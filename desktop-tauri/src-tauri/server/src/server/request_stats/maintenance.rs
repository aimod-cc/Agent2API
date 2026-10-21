//! 请求统计的**维护动作**：两模式清理、清理预览与数据库压缩。
//!
//! ── 为什么单独一层 ──────────────────────────────────────────
//! 这批方法（`clear_where` / `clear_raw_where` / `clear` / `clear_preview` /
//! `db_bytes` / `compact`）与「记账 / 查询 / 报表」是两类生命周期完全不同的
//! 操作：前者由管理 API 显式触发、低频且重（整表删除、重建库文件），后者
//! 高频且必须轻（每个请求的收尾路径都在调）。拆开后 `request_stats.rs` 专注
//! 于记账与读侧，本文件集中「删数据 / 量磁盘 / 压缩」这组互相支撑的语义
//! —— 预览（`clear_preview`）与执行（两个 clear）、压缩（`compact`）与
//! 压缩状态（`VacuumStatus`）必须读同一段代码才能确认口径一致，放一起才
//! 拆不散。方法仍是 `impl RequestStats` 的一部分（Rust 允许 impl 块分文件），
//! 对外 API 形状与拆分前逐字相同。
//!
//! ── request_raw 的同步清理约定 ──────────────────────────────
//! 原始正文（`request_raw` 表）与明细同 id 关联：**每一次删明细都必须同步
//! 删正文**（先删正文再删明细，顺序理由见 `sql::delete_raw_matching`），
//! 否则明细没了正文还在 —— 成为永远读不出来的孤儿行。本文件的两个
//! clear 与全量 clear 都遵守这条约定。明细与正文都没有自动淘汰
//! （不按天数、不按条数），删除只发生在两处：用户显式触发的这些清理动作，
//! 以及 schema v13 那次一次性清空（升级时回收旧的「始终采集」正文，
//! 见 `db/schema.rs` 的 `SCHEMA_VERSION`）。
//!
//! ── 并发模型（与 `request_stats.rs` 同一套）──────────────────
//! 所有方法取 `&self`，串行化靠 `RequestStats::guard` + `Db` 的连接锁：
//! 每个方法都在**一次** `with_conn` / `with_conn_mut` 里跑完（多语句用事务）。
//! **硬约束**：持锁期间绝不能调 `logging::log`（日志写同一个库，`Mutex`
//! 不可重入会死锁）—— `compact` 的完成日志因此在线程拿到结果、**释放锁之后**
//! 才打。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::server::logging;

use super::clock::now_ms;
use super::record::RequestQuery;
use super::{daily, sql, RequestStats};

impl RequestStats {
    /// 按筛选条件清空明细与对应原始正文（`DELETE mode=all` 带筛选时的执行体），
    /// 返回删除的明细条数。**不动按天聚合**。
    ///
    /// ── 为什么不再重算按天聚合 ──────────────────────────────────
    /// 改造前这里会把受影响日期的聚合行从剩余明细整行重算（让「报表请求数 =
    /// 剩余明细数」）。本次改为**不动日报**：日报是**全量聚合**（`fold_into_daily`
    /// 累计的是当天全部明细，含此前已被清理掉的部分），带筛选的部分删除后
    /// 它无法被部分重算 —— 手工从聚合行里减掉命中量要重写一遍「分组 / 建组 /
    /// 空组」的口径，而「按剩余明细整行重算」会抹掉早前清理掉的那部分历史贡献。
    /// 取舍：保留日报的历史读数（报表请求数可能大于当前明细能数出的量），
    /// 由 API 响应的 `note` 字段向用户说明（见 `stats_api::clear_stats_requests`）。
    ///
    /// 与全量 [`Self::clear`] 不同，这里**不**清调试模式的原始报文：命中的是
    /// 部分明细，调试报文可能还对应着留下的行（取舍见 `debug_traffic` 的说明）。
    ///
    /// 降级：库不可用时返回 0 ——「一条都没删」是唯一诚实的答案，
    /// 谎报删除条数会让前端提示「已清空 N 条」而库里什么都没变。
    pub fn clear_where(&self, filter: &RequestQuery) -> usize {
        let plan = sql::FilterPlan::of(filter);
        self.with_conn_mut(&self.guard(), |conn| {
            let tx = conn.transaction()?;
            // 先删命中的正文（明细删完就再也找不到要陪葬的正文 id 了）
            sql::delete_raw_matching(&tx, &plan)?;
            sql::delete_matching(&tx, &plan)
        })
        .unwrap_or(0)
    }

    /// 只清原始正文（`DELETE mode=raw` 的执行体）：按筛选条件命中的明细 id
    /// 删 `request_raw` 行，**不动**明细与日报 —— 报表的统计字段全部保留，
    /// 只抹掉「预览对话」用的正文。返回删除的正文行数。
    ///
    /// 明细行删除后正文会成为读不出来的孤儿（永远读不出来、白占空间，预览 404），
    /// 所以删正文必须挂在「删明细」的同一批 id 上；反过来只删正文、留明细
    /// 则是安全的（明细行还在，只是详情弹窗没有正文）。
    pub fn clear_raw_where(&self, filter: &RequestQuery) -> usize {
        let plan = sql::FilterPlan::of(filter);
        self.with_conn_mut(&self.guard(), |conn| sql::delete_raw_matching(conn, &plan))
            .unwrap_or(0)
    }

    /// 清空明细、按天聚合与原始正文（`DELETE mode=all` 不带筛选的执行体），
    /// 返回删除的明细条数。
    ///
    /// 为什么不做成「按筛选部分删除」的累计版本：一张表整清是唯一能让
    /// 「清空后立即读到 0 条」成立的语义 —— 部分清空只删命中的行，留下的
    /// 历史读数还在，用户看到的「已清空」与实际不符。这里直接删三张表
    /// 的全部行：清空后立即读到的就是 0 条，重开程序也不会把已删的数据载回来。
    /// **不改任何设置**：清数据与改配置是两件事。
    ///
    /// 降级：库不可用时返回 0 ——「一条都没删」是唯一诚实的答案，
    /// 谎报删除条数会让前端提示「已清空 N 条」而库里什么都没变。
    pub fn clear(&self) -> usize {
        self.with_conn_mut(&self.guard(), |conn| {
            let tx = conn.transaction()?;
            // 正文先于明细清（顺序理由见 sql::delete_raw_matching；
            // 全清时整表删，不需要按 id 找）
            sql::delete_all_raw(&tx)?;
            let removed = sql::delete_all_requests(&tx)?;
            daily::delete_all_daily(&tx)?;
            tx.commit()?;
            Ok(removed)
        })
        .unwrap_or(0)
    }

    /// 清理预览（弹窗的数据源）：当前筛选下**将删除**的明细数（`all`）、
    /// 其中仍带原始正文的条数（`raw`）、库文件磁盘占用（`dbBytes`），
    /// 以及压缩任务的运行状态（`vacuumRunning` / `lastVacuum`）。
    ///
    /// 三个读数与清理执行体共用同一份 `FilterPlan` —— 预览说删 N 条，
    /// 点确认删掉的就是 N 条。`dbBytes` 不按筛选计算（库文件没有「这一部分
    /// 属于这批筛选」的边界，VACUUM 后回收的空间也是整库的）。
    pub fn clear_preview(&self, filter: &RequestQuery) -> Value {
        let plan = sql::FilterPlan::of(filter);
        let loaded = self.with_conn(&self.guard(), |conn| {
            let all = sql::count_matching(conn, &plan)?;
            let raw = sql::count_raw_matching(conn, &plan)?;
            Ok((all, raw))
        });
        let (all, raw) = loaded.unwrap_or((0, 0));
        json!({
            "all": all,
            "raw": raw,
            "dbBytes": self.db_bytes(),
            "vacuumRunning": self.vacuum.is_running(),
            "lastVacuum": self.vacuum.last_finished(),
        })
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
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
                conn.execute_batch("VACUUM;")?;
                // 尾部 checkpoint：把 VACUUM 写进 WAL 的那份整库并回主库并截零。
                // 少了它，回收的空间只从主库搬到 `-wal`，总占用不降反升（#297）。
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
                Ok(())
            });
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
