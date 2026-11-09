use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde_json::{json, Value};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::config::{self, QueryConfig};
use crate::server::core::account_store::AccountStore;
use crate::server::logging;

const RETAIN_SECONDS: u64 = 300;

#[derive(Clone)]
pub(super) struct Job {
    pub id: String,
    pub request_id: String,
    pub account: Value,
    pub config: QueryConfig,
    pub cancel: CancellationToken,
    pub started_at: i64,
    pub timeout_seconds: u64,
    pub deadline: tokio::time::Instant,
    pub result: watch::Sender<Option<Value>>,
}

#[derive(Default)]
struct Registry {
    sequence: u64,
    jobs: HashMap<String, Arc<Job>>,
}

fn lock() -> MutexGuard<'static, Registry> {
    static STATE: OnceLock<Mutex<Registry>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Registry::default())).lock().unwrap_or_else(|error| error.into_inner())
}

impl Job {
    pub fn pending(&self) -> bool { self.result.borrow().is_none() }

    pub fn state(&self) -> Value {
        let result = self.result.borrow();
        let status = result.as_ref().and_then(|row| row.get("queryStatus")).and_then(Value::as_str).unwrap_or("running");
        json!({ "id": self.id, "requestId": self.request_id, "status": status,
            "error": result.as_ref().and_then(|row| row.get("error")),
            "startedAt": self.started_at, "deadlineAt": self.started_at + self.timeout_seconds as i64 * 1000 })
    }

    fn stop(&self, status: &str) {
        if !self.pending() { return; }
        self.cancel.cancel();
        self.result.send_replace(Some(json!({
            "id": self.id, "requestId": self.request_id, "queryStatus": status,
            "usage": Value::Null, "error": Value::Null, "at": logging::now_ms(),
        })));
    }
}

pub(super) fn begin(store: &AccountStore, id: &str, replace: bool) -> Option<(Arc<Job>, bool)> {
    let guard = store.guard();
    let _config_guard = config::lock();
    let record = store.record_by_id(&guard, id)?;
    let config = config::of(&record);
    let account = store.public_account(&record);
    if config.fingerprint.is_empty() || config.reading_key.is_empty() { return None; }
    let mut registry = lock();
    let now = logging::now_ms();
    registry.jobs.retain(|_, job| job.pending() || now - job.started_at < RETAIN_SECONDS as i64 * 1000);
    if let Some(previous) = registry.jobs.get(id) {
        if previous.pending() && !replace && previous.config == config { return Some((previous.clone(), false)); }
        previous.stop(if previous.config == config { "superseded" } else { "config_changed" });
    }
    registry.sequence = registry.sequence.wrapping_add(1);
    let (result, _) = watch::channel(None);
    let timeout_seconds = crate::server::core::usage_records::query_timeout_of(record.get("usageQuery"));
    let job = Arc::new(Job { id: id.to_string(), request_id: format!("{now}-{}", registry.sequence),
        account, config, cancel: CancellationToken::new(), started_at: now, timeout_seconds,
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_seconds), result });
    registry.jobs.insert(id.to_string(), job.clone());
    crate::server::core::usage_records::mark_attempt(id);
    Some((job, true))
}

pub(super) fn finish(job: &Arc<Job>, action: impl FnOnce() -> Option<Value>) -> bool {
    let registry = lock();
    if job.cancel.is_cancelled() || !registry.jobs.get(&job.id).is_some_and(|current| Arc::ptr_eq(current, job)) { return false; }
    let Some(mut row) = action() else { job.stop("config_changed"); return false; };
    row["requestId"] = json!(job.request_id);
    row["queryStatus"] = json!(if row.get("code").and_then(Value::as_str) == Some("usage_timeout") {
        "timeout"
    } else if row.get("error").and_then(Value::as_str).is_some() { "failed" } else { "success" });
    job.result.send_replace(Some(row));
    true
}

pub fn cancel(id: &str, request_id: &str) -> bool {
    let registry = lock();
    let Some(job) = registry.jobs.get(id).filter(|job| job.request_id == request_id) else { return false; };
    let pending = job.pending();
    job.stop("cancelled");
    pending
}

pub(crate) fn invalidate_changed(id: &str, fingerprint: &str) {
    if let Some(job) = lock().jobs.get(id) {
        if job.config.fingerprint != fingerprint { job.stop("config_changed"); }
    }
}

pub fn invalidate_account(id: &str) {
    if let Some(job) = lock().jobs.get(id) { job.stop("config_changed"); }
}

pub(super) fn reconcile_one(store: &AccountStore, job: &Arc<Job>) {
    let guard = store.guard();
    let _config_guard = config::lock();
    let current = store.record_by_id(&guard, &job.id).map(|record| config::of(&record));
    if current.as_ref() != Some(&job.config) {
        let registry = lock();
        if registry.jobs.get(&job.id).is_some_and(|current| Arc::ptr_eq(current, job)) { job.stop("config_changed"); }
    }
}

pub(super) fn reconcile(store: &AccountStore) {
    let jobs: Vec<_> = lock().jobs.values().filter(|job| job.pending()).cloned().collect();
    for job in jobs { reconcile_one(store, &job); }
}

pub fn states(store: &AccountStore) -> Vec<Value> {
    reconcile(store);
    let mut registry = lock();
    let now = logging::now_ms();
    registry.jobs.retain(|_, job| job.pending() || now - job.started_at < RETAIN_SECONDS as i64 * 1000);
    registry.jobs.values().map(|job| job.state()).collect()
}

pub(super) async fn wait(job: Arc<Job>) -> Value {
    let mut result = job.result.subscribe();
    loop {
        if let Some(row) = result.borrow().clone() { return row; }
        if result.changed().await.is_err() {
            return json!({ "id": job.id, "requestId": job.request_id, "queryStatus": "cancelled" });
        }
    }
}
