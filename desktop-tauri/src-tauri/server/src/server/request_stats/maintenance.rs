//! 请求统计的维护动作：两侧报文清理、清理预览与数据库压缩。
//!
//! 上游调试报文与下游正文共用请求 ID。筛选清理按命中的 ID 处理，无筛选
//! 时清空两张报文表；删除明细前必须先清报文，且全部动作在同一事务中提交。
//! 持数据库锁期间不能调用 logging::log，它也会写同一个库。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::server::logging;

use super::clock::now_ms;
use super::record::RequestQuery;
use super::{daily, sql, RequestStats};

#[derive(Clone, Copy, Default)]
pub struct ClearResult {
    pub deleted: usize,
    pub raw: usize,
    pub downstream_raw: usize,
    pub upstream_raw: usize,
}

impl RequestQuery {
    pub fn has_filters(&self) -> bool {
        !sql::FilterPlan::of(self).is_empty()
    }
}

impl RequestStats {
    /// 只清两侧报文，或同时清理请求明细；无筛选删除明细时重置日报。
    pub fn clear_requests(&self, filter: &RequestQuery, raw_only: bool) -> Option<ClearResult> {
        let plan = sql::FilterPlan::of(filter);
        self.with_conn_mut(&self.guard(), |conn| {
            let tx = conn.transaction()?;
            let raw = sql::delete_raw_matching(&tx, &plan)?;
            let deleted = if raw_only {
                raw.requests
            } else if plan.is_empty() {
                let removed = sql::delete_all_requests(&tx)?;
                daily::delete_all_daily(&tx)?;
                removed
            } else {
                sql::delete_matching(&tx, &plan)?
            };
            tx.commit()?;
            Ok(ClearResult {
                deleted,
                raw: raw.requests,
                downstream_raw: raw.downstream,
                upstream_raw: raw.upstream,
            })
        })
    }

    pub fn clear_preview(&self, filter: &RequestQuery) -> Option<Value> {
        let plan = sql::FilterPlan::of(filter);
        let (all, raw) = self.with_conn(&self.guard(), |conn| {
            let all = sql::count_matching(conn, &plan)?;
            let raw = sql::count_raw_matching(conn, &plan)?;
            Ok((all, raw))
        })?;
        Some(json!({
            "all": all,
            "raw": raw.requests,
            "downstreamRaw": raw.downstream,
            "upstreamRaw": raw.upstream,
            "dbBytes": self.db_bytes(),
            "vacuumRunning": self.vacuum.is_running(),
            "lastVacuum": self.vacuum.last_finished(),
        }))
    }

    /// 库文件的磁盘占用（主库 + WAL，字节）。
    ///
    /// WAL 是已提交数据的一部分（checkpoint 前它才是「新」副本），只报主库
    /// 会把占用算小 —— 报「磁盘还剩多少可回收」的场景两个文件都要算。
    /// 读不到（文件不存在 / 无权限）按 0：设置页显示 0 字节比整个接口 500 好。
    pub fn db_bytes(&self) -> u64 {
        let file = self.file();
        file_size(&file) + file_size(&PathBuf::from(format!("{}-wal", file.to_string_lossy())))
    }

    /// 压缩数据库：`checkpoint(TRUNCATE)` → `VACUUM` → 再 `checkpoint(TRUNCATE)`
    /// —— 首尾两次 checkpoint 缺一不可（理由见下），中间那次 VACUUM 重建整库
    /// 文件回收空闲页。**后台线程**执行，返回启动结果。
    ///
    /// ── 为什么 VACUUM 之后还要再 checkpoint 一次（issue #297）────
    /// WAL 模式下 `VACUUM` 是**一个原子事务**，而自动 checkpoint 只在事务
    /// **之间**触发 —— 撑不住它的体量。于是重建后的整库会被原样写进 `-wal`，
    /// VACUUM 收尾那次被动 checkpoint 把内容并回主库却**不截断 WAL 文件**：
    /// 主库确实缩了，`-wal` 里却留着与整库等大的一份已分配空间。
    ///
    /// 只做首尾两次中的前一次（本函数在 #297 之前的形态）会稳定复现该 issue：
    /// 实测「主库 126MB + WAL 0MB」的库走一遍变成「110MB + 111MB」，总占用
    /// 从 126MB 涨到 221MB（1.76 倍）—— 与 #297 报的 624MB → 1.09GB（1.747 倍）
    /// 同一形态。补上尾部 checkpoint 后同一个库变成「110MB + 0MB」，真正下降。
    ///
    /// 触发条件是「首参之前 `-wal` 已经被截零」（= 刚重启过、或上次压缩收过尾）：
    /// 那时起点没有可回收的 WAL 余量，用户看到的就只剩「涨上去的那部分」。
    /// 若起点 WAL 本身是肥的（跑了一阵没重启），首参先把它收回主库，账面仍是
    /// 下降 —— 这正是同一个 bug 被一部分用户报成「正常变小」的原因。
    ///
    /// ── 为什么必须拿那把连接锁 ──────────────────────────────────
    /// 本项目存储层是「同步 rusqlite 单连接 + `Mutex`」：VACUUM 不允许在事务
    /// 里跑，且必须与其它读写互斥 —— 唯一正确的入口就是 `Db::with`（拿锁）。
    /// 持锁期间其余数据库操作（记账、日志写入、管理 API）会排队：桌面单文件
    /// 库通常几十 MB，VACUUM 秒级完成，这个停顿可以接受 —— 这也正是压缩做成
    /// 「后台线程 + 用户显式触发」而不是记账路径顺带做的理由。
    ///
    /// ── 为什么是 std::thread 而不是 tokio::spawn ────────────────
    /// `db.with` 是**阻塞**调用（拿不住锁就一直等），放进异步任务会占死一个
    /// tokio worker 线程；专用系统线程把阻塞留在运行时之外，跑完即回收。
    ///
    /// ── 状态与并发 ──────────────────────────────────────────────
    /// 运行标记在**启动前**原子置位（`VacuumStatus::try_start`），重复触发由
    /// 调用方给 409；完成时刻无论成败都记录（失败也是一次终局）。
    pub fn compact(&self) -> CompactStart {
        if !self.vacuum.try_start() {
            return CompactStart::AlreadyRunning;
        }
        let Some(db) = self.db.clone() else {
            // 库不可用：复位标记（否则 vacuumRunning 永远 true），如实上报
            self.vacuum.mark_finished();
            return CompactStart::Unavailable;
        };
        let vacuum = Arc::clone(&self.vacuum);
        std::thread::spawn(move || {
            let outcome = db.with(|conn| -> rusqlite::Result<()> {
                db.invalidate_accounts();
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
                conn.execute_batch("VACUUM;")?;
                // 尾部 checkpoint：把 VACUUM 写进 WAL 的那份整库并回主库并截零。
                // 少了它，回收的空间只从主库搬到 `-wal`，总占用不降反升（#297）。
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
                db.invalidate_accounts();
                Ok(())
            });
            db.invalidate_accounts();
            let ok = matches!(outcome, Some(Ok(())));
            // 先复位状态再打日志：日志要写同一个库（拿锁），若先打日志，
            // 状态会多卡一会儿；复位后的并发压缩请求与日志写入互不阻塞
            vacuum.mark_finished();
            if ok {
                logging::log("[Stats]", "数据库压缩完成（checkpoint + VACUUM + checkpoint）");
            } else {
                logging::log("[Stats]", "⚠️ 数据库压缩未完成：数据库不可用或压缩失败");
            }
        });
        CompactStart::Started
    }
}

/// [`RequestStats::compact`] 的启动结果（三种去向各有各的响应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactStart {
    /// 已在后台线程启动
    Started,
    /// 已有压缩任务在跑（调用方给 409）
    AlreadyRunning,
    /// 库不可用，无从压缩（调用方如实报错而不是谎报「已启动」）
    Unavailable,
}

/// [`RequestStats::compact`] 的运行状态。
///
/// 为什么挂在 `RequestStats` 上而不是模块级 `static`：`RequestStats` 本来就是
/// 进程级单例（`ServerState` 里那一份 `Arc`），状态挂在实例上让「有多个实例
/// 的测试」各自独立；OmniProxy 用模块级 let 变量是因为它的 router 是文件级
/// 单例，两边语义等价，这里选与本项目结构一致的挂法。
pub(super) struct VacuumStatus {
    /// 是否有压缩任务在跑（`swap` 保证「判定 + 置位」原子，防并发双开）
    running: AtomicBool,
    /// 最近一次压缩**完成**的时刻（毫秒 Unix；None = 本次运行还没完成过）。
    /// 无论成败都记录 —— 「完成」包括失败（失败也是一次终局，前端据此停止轮询）。
    last_finished_at: Mutex<Option<i64>>,
}

impl VacuumStatus {
    pub(super) fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            last_finished_at: Mutex::new(None),
        }
    }

    /// 尝试占用「正在运行」标记：返回 false 表示已有任务在跑。
    ///
    /// 用 `swap` 而不是「先 load 再 store」：判定与置位在同一条原子操作里，
    /// 两个并发请求只有一个能拿到 `false` 的旧值 —— 不需要额外加锁。
    fn try_start(&self) -> bool {
        !self.running.swap(true, Ordering::AcqRel)
    }

    fn mark_finished(&self) {
        if let Ok(mut slot) = self.last_finished_at.lock() {
            *slot = Some(now_ms());
        }
        self.running.store(false, Ordering::Release);
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn last_finished(&self) -> Option<i64> {
        self.last_finished_at.lock().ok().and_then(|slot| *slot)
    }
}

/// 文件大小（读不到按 0；`db_bytes` 的辅助）
fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}
