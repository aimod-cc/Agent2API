//! 自动同步：本地数据变了 → 防抖合并 → 自动上传。
//!
//! ── 触发源为什么分成两路 ────────────────────────────────────
//! 业务数据落在两类地方：
//!   · **三张表**（`accounts` / `account_usage_records` / `checkin_records`）——
//!     用 SQLite 的 `update_hook` 监听。这是最可靠的一路：不管谁写的（账号
//!     增删改、token 刷新、签到记账、余额查询），钩子都会响。
//!   · **kv 键**（配置、Agent 配置、代理池 / 订阅）—— 走**显式通知**，不用钩子。
//!
//! ── kv 为什么不能用 update_hook ─────────────────────────────
//! `kv` 表同时承载 `logsNextId` 这类**每秒都在变**的本机状态，钩子必须能分辨
//! 「改的是哪个键」。而 `update_hook` 的回调是在 SQLite 执行语句**期间**被调用
//! 的（此时连接锁已被 `Db::with` 持有），在回调里再查一次库属于重入 —— 轻则
//! 拖慢每一次日志写入，重则死锁。所以 kv 这一路改为在**写入点**显式调用
//! [`notify_change`]：`config::update`（配置项，逐键比对同步范围）、
//! `agents::save_client`、`proxy_pool::write_items`、`subscriptions::write_items`、
//! `proxy_settings::write_raw` 五处。代价是将来新增一个「同步范围内的 kv 写入点」
//! 时要记得加一行通知 —— 这条约束写在 `snapshot::is_synced_kv_key` 的注释里。
//!
//! ── 为什么要防抖 + 上限 ────────────────────────────────────
//! 一次「添加账号」会连着写好几行（账号 + 用量记录），token 刷新也可能在几秒内
//! 连写两三次。每个信号都上传一次等于把同一个快照重复传 N 遍。策略与 cc-switch
//! 一致：**收到第一个信号后等 1 秒**，期间的新信号合并；若信号连续不断，最多
//! 等 10 秒就必须传一次（否则高频写入会让同步永远不触发）。
//!
//! ── 为什么上传前还要比内容哈希 ──────────────────────────────
//! 防抖只能合并「短时间内的多次写」。真正的问题是：`update_hook` 会在**内容
//! 没变**的写入上响（例如 token 刷新写回同样的值）。每次上传都是几十 KB 到
//! 几 MB 的外网请求，长期跑下来既费流量又可能撞上云盘限流。所以上传前先构建
//! 快照、算哈希：与 `lastLocalHash` 相同**且**密钥指纹也没变 → 直接跳过，一个
//! 请求都不发。这条判断也让「刚下载完」不会立刻回传（下载后写库会触发钩子，
//! 但内容与远端一致 → 跳过）。抑制守卫因此是第二道保险，不是唯一依赖。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{channel, Receiver, Sender};

use crate::server::core::crypto::{self, SecretScope};
use crate::server::db::Db;
use crate::server::logging;

use super::{settings, snapshot};

/// 第一个信号之后的等待窗口（合并窗口内到达的信号不再单独上传）
const DEBOUNCE_MS: u64 = 1_000;
/// 连续信号下的最长等待：到点必须传一次
const MAX_WAIT_MS: u64 = 10_000;

static CHANGE_TX: OnceLock<Sender<()>> = OnceLock::new();
static SUPPRESS_DEPTH: AtomicUsize = AtomicUsize::new(0);

/// 抑制守卫：作用域内不触发自动上传（下载应用期间用）。
///
/// 用计数而不是布尔：将来若有嵌套场景，布尔会让内层的 `drop` 提前解除抑制。
pub(crate) struct SuppressionGuard;

impl SuppressionGuard {
    pub(crate) fn new() -> Self {
        SUPPRESS_DEPTH.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for SuppressionGuard {
    fn drop(&mut self) {
        let _ = SUPPRESS_DEPTH.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            Some(value.saturating_sub(1))
        });
    }
}

fn is_suppressed() -> bool {
    SUPPRESS_DEPTH.load(Ordering::SeqCst) > 0
}

/// 这个表名要不要触发自动同步（唯一判定，钩子与测试共用）
pub(crate) fn should_trigger_for_table(table: &str) -> bool {
    snapshot::SYNCED_TABLES.contains(&table)
}

/// 发一个「本地数据变了」的信号。非阻塞：通道满（已有待处理信号）时直接丢弃 ——
/// 我们要的是「脏了」这件事，不是每一次变化。
///
/// **可以在持有数据库连接锁的钩子里调用**：不做 IO、不打日志、不取锁。
pub(crate) fn notify_change() {
    if is_suppressed() {
        return;
    }
    let Some(tx) = CHANGE_TX.get() else {
        return;
    };
    match tx.try_send(()) {
        Ok(()) => {}
        // 缓冲 1：满了说明已有一个待处理信号，本次合并进去即可
        Err(TrySendError::Full(_)) | Err(TrySendError::Closed(_)) => {}
    }
}

/// SQLite `update_hook` 的回调入口。
///
/// ── 硬约束：这里必须极快 ────────────────────────────────────
/// 钩子在语句执行期间被调用（连接锁已被持有）。因此只做一次数组查找 + 一次
/// 非阻塞 send，**绝不**查库、不打日志（`logging::log` 要写同一个库 → 死锁）。
pub(crate) fn on_table_change(table: &str) {
    if should_trigger_for_table(table) {
        notify_change();
    }
}

/// 启动自动同步工作线程（`bootstrap` 里调一次）。
///
/// ── 为什么线程里现读设置 ────────────────────────────────────
/// 用户在设置页改开关后要立刻生效，不必重启。工作线程每轮都从
/// `settings::load()` 读最新值（一次 kv 查询，频率是「有信号时」）。
pub(crate) fn spawn_worker(db: Db) {
    if CHANGE_TX.get().is_some() {
        return;
    }
    let (tx, rx) = channel::<()>(1);
    if CHANGE_TX.set(tx).is_err() {
        return;
    }
    crate::spawn_task(async move {
        run_worker(db, rx).await;
    });
}

async fn run_worker(db: Db, mut rx: Receiver<()>) {
    while rx.recv().await.is_some() {
        let started = Instant::now();
        // 防抖：等到「安静 1 秒」或「总共 10 秒」，以先到者为准
        loop {
            let elapsed = started.elapsed();
            if elapsed >= Duration::from_millis(MAX_WAIT_MS) {
                break;
            }
            let remaining = Duration::from_millis(MAX_WAIT_MS) - elapsed;
            let wait = Duration::from_millis(DEBOUNCE_MS).min(remaining);
            match tokio::time::timeout(wait, rx.recv()).await {
                Ok(Some(())) => continue,
                // 通道关闭 = 进程在退出
                Ok(None) => return,
                // 安静窗口到了
                Err(_) => break,
            }
        }
        if let Err(error) = run_once(&db).await {
            super::record_failure(&error, "auto");
        }
    }
}

/// 跑一轮自动上传。`Ok(false)` = 内容没变 / 未启用，跳过。
async fn run_once(db: &Db) -> Result<bool, String> {
    let settings = settings::load();
    if !settings.enabled || !settings.auto_sync {
        return Ok(false);
    }
    if is_suppressed() {
        return Ok(false);
    }
    settings.validate()?;

    // 内容没变就跳过：先构建快照算哈希（本地操作，不发网络请求）
    let snapshot = snapshot::build(db)?;
    let content_hash = snapshot.content_hash();
    let key_fingerprint = crypto::key_fingerprint(SecretScope::Accounts);
    let unchanged = settings.status.last_local_hash.as_deref() == Some(content_hash.as_str())
        && settings.status.last_key_fingerprint == key_fingerprint;
    if unchanged {
        return Ok(false);
    }

    let _guard = super::sync_lock().lock().await;
    // 拿到锁之后再读一次设置：排队期间用户可能关了开关
    let settings = settings::load();
    if !settings.enabled || !settings.auto_sync {
        return Ok(false);
    }
    super::upload_locked(db, &settings).await?;
    logging::verbose("[CloudSync]", "自动同步：已上传本地变更");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_business_tables_trigger() {
        assert!(should_trigger_for_table("accounts"));
        assert!(should_trigger_for_table("checkin_records"));
        assert!(should_trigger_for_table("account_usage_records"));
        assert!(!should_trigger_for_table("logs"));
        assert!(!should_trigger_for_table("requests"));
        assert!(!should_trigger_for_table("debug_traffic"));
        assert!(!should_trigger_for_table("kv"));
    }

    #[test]
    fn suppression_guard_is_scoped() {
        assert!(!is_suppressed());
        {
            let _guard = SuppressionGuard::new();
            assert!(is_suppressed());
            {
                let _nested = SuppressionGuard::new();
                assert!(is_suppressed());
            }
            assert!(is_suppressed(), "内层释放后外层仍应抑制");
        }
        assert!(!is_suppressed());
    }
}
