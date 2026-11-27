//! 本机直连 Agent 采集器：读取不经过网关的本地 Agent 历史请求记录并合并进日志与报表。
//! 对话原文不落库，由详情接口在预览时按索引路径实时读取。

pub mod antigravity;
pub mod antigravity_account;
pub mod antigravity_backfill;
pub mod preview;
pub mod protobuf;
pub mod registry;
pub mod settings;
pub mod sqlite_reader;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::server::core::account_store::AccountStore;
use crate::server::logging;
use crate::server::request_stats::{LocalDirectIngestReport, RequestStats};

#[derive(Clone, Debug, Default)]
struct RuntimeSyncState {
    syncing: bool,
    last_sync_at: Option<i64>,
    last_inserted: usize,
    last_deduped_existing: usize,
    last_deduped_gateway: usize,
    last_error: Option<String>,
}

static RUNTIME_STATE: OnceLock<Mutex<RuntimeSyncState>> = OnceLock::new();
static SYNC_MUTEX: OnceLock<AsyncMutex<()>> = OnceLock::new();
static WAKE_NOTIFY: OnceLock<Arc<Notify>> = OnceLock::new();
static REASONING_BACKFILL_DONE: AtomicBool = AtomicBool::new(false);

fn runtime_state() -> &'static Mutex<RuntimeSyncState> {
    RUNTIME_STATE.get_or_init(|| Mutex::new(RuntimeSyncState::default()))
}

fn sync_mutex() -> &'static AsyncMutex<()> {
    SYNC_MUTEX.get_or_init(|| AsyncMutex::new(()))
}

fn wake_notify() -> &'static Arc<Notify> {
    WAKE_NOTIFY.get_or_init(|| Arc::new(Notify::new()))
}

pub fn notify_settings_changed() {
    wake_notify().notify_one();
}

pub fn status_json(stats: &RequestStats) -> Value {
    let cfg = settings::load();
    let agents = registry::agent_cards_json(&cfg, stats);
    let rt = runtime_state()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone();
    json!({
        "enabled": cfg.enabled,
        "pollIntervalSecs": cfg.poll_interval_secs,
        "syncing": rt.syncing,
        "lastSyncAt": rt.last_sync_at,
        "lastInserted": rt.last_inserted,
        "lastDedupedExisting": rt.last_deduped_existing,
        "lastDedupedGateway": rt.last_deduped_gateway,
        "lastError": rt.last_error,
        "agents": agents,
    })
}

pub fn scan_and_ingest_blocking(
    stats: &RequestStats,
    accounts: Option<&AccountStore>,
    force_rescan: bool,
    only_agent: Option<&str>,
) -> LocalDirectIngestReport {
    let cfg = settings::load();
    let mut total = LocalDirectIngestReport::default();

    let antigravity_selected = only_agent.is_none_or(|id| id == "antigravity");
    if antigravity_selected && (cfg.agent_enabled("antigravity") || (force_rescan && only_agent == Some("antigravity"))) {
        if force_rescan || !REASONING_BACKFILL_DONE.swap(true, Ordering::AcqRel) {
            let backfilled = antigravity_backfill::backfill_antigravity_reasoning(stats);
            if backfilled > 0 {
                let msg = format!("已为 {backfilled} 条历史本机直连请求补全思考等级");
                logging::console_line("[LocalDirect]", &msg);
                logging::log_event(
                    "info",
                    "server",
                    &format!("[LocalDirect] {msg}"),
                    Some(json!({
                        "routine": true,
                        "component": "LocalDirect",
                        "backfilledReasoning": backfilled,
                    })),
                );
            }
        }
        if force_rescan {
            stats.clear_local_direct_file_states(Some("antigravity"));
        }
        let custom_paths = cfg.custom_paths_for("antigravity");
        let file_states = if force_rescan {
            Default::default()
        } else {
            stats.local_direct_file_states("antigravity")
        };
        let account_names = accounts
            .map(AccountStore::antigravity_email_names)
            .unwrap_or_default();
        let batch = antigravity::scan_all(custom_paths, &file_states, force_rescan, &account_names);
        if let Some(report) = stats.ingest_local_direct(batch.items, &batch.file_updates) {
            total.inserted += report.inserted;
            total.deduped_existing += report.deduped_existing;
            total.deduped_gateway += report.deduped_gateway;
        }
    }

    total
}

pub async fn sync_now(
    stats: Arc<RequestStats>,
    accounts: Option<AccountStore>,
    force_rescan: bool,
    only_agent: Option<String>,
) -> Result<Value, String> {
    let _lock = sync_mutex().lock().await;
    {
        let mut rt = runtime_state().lock().unwrap_or_else(|err| err.into_inner());
        rt.syncing = true;
        rt.last_error = None;
    }

    let stats_for_task = stats.clone();
    let agent_filter = only_agent.clone();
    let join_res = tokio::task::spawn_blocking(move || {
        scan_and_ingest_blocking(
            &stats_for_task,
            accounts.as_ref(),
            force_rescan,
            agent_filter.as_deref(),
        )
    })
    .await;

    let now = logging::now_ms();
    match join_res {
        Ok(report) => {
            {
                let mut rt = runtime_state().lock().unwrap_or_else(|err| err.into_inner());
                rt.syncing = false;
                rt.last_sync_at = Some(now);
                rt.last_inserted = report.inserted;
                rt.last_deduped_existing = report.deduped_existing;
                rt.last_deduped_gateway = report.deduped_gateway;
                rt.last_error = None;
            }
            if report.inserted > 0 {
                let msg = format!(
                    "已导入 {} 条本机直连请求（去重网关同源 {} 条）",
                    report.inserted, report.deduped_gateway
                );
                logging::console_line("[LocalDirect]", &msg);
                logging::log_event(
                    "info",
                    "server",
                    &format!("[LocalDirect] {msg}"),
                    Some(json!({
                        "routine": true,
                        "component": "LocalDirect",
                        "inserted": report.inserted,
                        "dedupedGateway": report.deduped_gateway,
                    })),
                );
            }
            Ok(json!({
                "success": true,
                "report": report,
                "status": status_json(&stats),
            }))
        }
        Err(err) => {
            let msg = format!("本机直连扫描任务异常: {err}");
            logging::log_with_level("[LocalDirect]", &msg, "error");
            {
                let mut rt = runtime_state().lock().unwrap_or_else(|err| err.into_inner());
                rt.syncing = false;
                rt.last_sync_at = Some(now);
                rt.last_error = Some(msg.clone());
            }
            Err(msg)
        }
    }
}

pub fn trigger_background_sync(
    stats: Arc<RequestStats>,
    accounts: Option<AccountStore>,
    force_rescan: bool,
    only_agent: Option<String>,
) {
    crate::spawn_task(async move {
        let _ = sync_now(stats, accounts, force_rescan, only_agent).await;
    });
}

pub fn spawn(stats: Arc<RequestStats>, accounts: Option<AccountStore>) {
    let notify = wake_notify().clone();
    crate::spawn_task(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        loop {
            let cfg = settings::load();
            if cfg.enabled {
                let _ = sync_now(stats.clone(), accounts.clone(), false, None).await;
            }
            let wait = Duration::from_secs(cfg.poll_interval_secs.clamp(
                settings::MIN_POLL_INTERVAL_SECS,
                settings::MAX_POLL_INTERVAL_SECS,
            ));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = notify.notified() => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });
}
