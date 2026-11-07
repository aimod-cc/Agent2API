//! 自动同步：变更通知合并、业务内容去重、最小上传间隔与失败退避。
//! SQLite 回调只发非阻塞信号；kv 由同步范围内的写入点显式通知。
//! 等待期间只记「有待处理变更」，到期读取最新快照；启动时补查，避免重启丢待办。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use zeroize::Zeroizing;

use crate::server::core::crypto::{self, SecretScope};
use crate::server::db::Db;
use crate::server::logging;

use super::{settings, snapshot};

/// 第一个信号之后的等待窗口（合并窗口内到达的信号不再单独上传）
const DEBOUNCE_MS: u64 = 1_000;
/// 连续变更的合并上限，不替代两次上传之间的最小间隔。
const MAX_WAIT_MS: u64 = 10_000;
const RETRY_BASE_MS: i64 = 60_000;
const RETRY_MAX_MS: i64 = 3_600_000;

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
        let previous = SUPPRESS_DEPTH.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            Some(value.saturating_sub(1))
        });
        if previous == Ok(1) {
            // 抑制期间也可能有真正的本地写入，结束后补查一次，不能丢掉这些变更。
            notify_change();
        }
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
    notify_change();
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
        let mut retry = None;
        loop {
            if !wait_until_ready(&mut rx, retry.as_ref()).await {
                break;
            }
            match run_once(&db).await {
                RunOutcome::Complete => break,
                RunOutcome::Deferred => retry = None,
                RunOutcome::Retry(delay) => retry = Some(delay),
            }
        }
    }
}

struct RetryDelay {
    at: i64,
    target_id: String,
    last_sync_at: Option<i64>,
}

async fn wait_until_ready(rx: &mut Receiver<()>, retry: Option<&RetryDelay>) -> bool {
    loop {
        let settings = settings::load();
        if !settings.enabled || !settings.auto_sync {
            return false;
        }
        let now = logging::now_ms();
        let retry_floor = retry.filter(|delay| delay.target_id == settings.target_id()
            && delay.last_sync_at == settings.status.last_sync_at).map(|delay| delay.at).unwrap_or(0);
        let due = settings.auto_sync_due_at().max(retry_floor);
        if due <= now {
            return true;
        }
        let wait_ms = due.saturating_sub(now).clamp(1, 30_000) as u64;
        match tokio::time::timeout(Duration::from_millis(wait_ms), rx.recv()).await {
            Ok(Some(())) | Err(_) => {},
            Ok(None) => return false,
        }
    }
}

enum RunOutcome {
    Complete,
    Deferred,
    Retry(RetryDelay),
}

async fn run_once(db: &Db) -> RunOutcome {
    let _guard = super::sync_lock().lock().await;
    let settings = settings::load();
    if !settings.enabled || !settings.auto_sync {
        return RunOutcome::Complete;
    }
    if is_suppressed() || settings.auto_sync_due_at() > logging::now_ms() {
        return RunOutcome::Deferred;
    }
    match upload_if_changed(db, &settings).await {
        Ok(()) => RunOutcome::Complete,
        Err(error) => {
            let failures = settings.status.consecutive_failures.saturating_add(1);
            let backoff = RETRY_BASE_MS.saturating_mul(1i64 << failures.saturating_sub(1).min(6))
                .min(RETRY_MAX_MS)
                .max(i64::from(settings.auto_sync_interval_minutes) * 60_000);
            let now = logging::now_ms();
            let retry_at = now.saturating_add(backoff);
            let saved = settings::update_status_for(&settings, |status| {
                status.last_attempt_at = Some(now);
                status.retry_at = Some(retry_at);
                status.consecutive_failures = failures;
                status.last_error = Some(error.clone());
                status.last_error_source = Some("auto".to_string());
            });
            let detail = saved.err().map(|cause| format!("；同步状态未保存：{cause}")).unwrap_or_default();
            logging::log_with_level("[CloudSync]", &format!("⚠️ 云同步（自动）失败：{error}{detail}"), "warn");
            RunOutcome::Retry(RetryDelay {
                at: retry_at,
                target_id: settings.target_id(),
                last_sync_at: settings.status.last_sync_at,
            })
        }
    }
}

async fn upload_if_changed(db: &Db, settings: &settings::CloudSyncSettings) -> Result<(), String> {
    settings.validate()?;
    let snapshot = snapshot::build(db)?;
    let key = crypto::backup_key(SecretScope::Accounts)?.map(Zeroizing::new);
    let content_hash = snapshot.content_hash(key.as_deref())?;
    let key_fingerprint = key.as_deref().map(|bytes| crypto::fingerprint_of(bytes));
    let target_id = settings.target_id();
    let unchanged = settings.status.last_local_hash.as_deref() == Some(content_hash.as_str())
        && settings.status.last_key_fingerprint == key_fingerprint
        && settings.status.last_target_id.as_deref() == Some(target_id.as_str());
    if unchanged {
        if settings.status.retry_at.is_some() || settings.status.consecutive_failures != 0 {
            settings::update_status_for(settings, |status| {
                status.retry_at = None;
                status.consecutive_failures = 0;
                status.last_error = None;
                status.last_error_source = None;
            })?;
        }
        return Ok(());
    }
    super::upload_locked(settings, snapshot, content_hash, key).await?;
    logging::verbose("[CloudSync]", "自动同步：已上传本地变更");
    Ok(())
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
