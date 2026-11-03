//! Independent, opt-in egress and backpressure for account maintenance RPCs.
//! No account.proxy writes, no process/global proxy changes, no model calls.
use super::proxy_pool;
use crate::server::{config, errors::GatewayError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

const KEY: &str = "maintenancePolicy";
pub const CAPABILITIES_METHOD: &str = "maintenance.capabilities";
const OPERATIONS: &[&str] = &["usage.query", "checkin.claim", "claim", "claim.status"];

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Direct,
    Auto,
    Fixed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub mode: Mode,
    pub proxy_id: String,
    pub providers: Vec<String>,
    pub balance: bool,
    pub checkin: bool,
    pub rewards: bool,
    pub max_concurrent: usize,
    pub per_proxy_concurrent: usize,
    pub min_interval_ms: u64,
    pub max_interval_ms: u64,
    pub max_pending: usize,
    pub queue_timeout_seconds: u64,
    pub usage_cache_seconds: u64,
    pub failure_cooldown_seconds: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: Mode::Direct,
            proxy_id: String::new(),
            providers: vec!["raccoon".into(), "loomy".into()],
            balance: true,
            checkin: true,
            rewards: true,
            max_concurrent: 2,
            per_proxy_concurrent: 1,
            min_interval_ms: 500,
            max_interval_ms: 1500,
            max_pending: 128,
            queue_timeout_seconds: 120,
            usage_cache_seconds: 60,
            failure_cooldown_seconds: 30,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=16).contains(&self.max_concurrent)
            || !(1..=16).contains(&self.per_proxy_concurrent)
            || !(1..=4096).contains(&self.max_pending)
            || !(1..=600).contains(&self.queue_timeout_seconds)
            || self.min_interval_ms > self.max_interval_ms
            || self.max_interval_ms > 60_000
            || self.usage_cache_seconds > 3600
            || self.failure_cooldown_seconds > 600
        {
            return Err("Invalid maintenance concurrency, interval, cache or queue limits".into());
        }
        if self.providers.is_empty()
            || self.providers.len() > 64
            || self.providers.iter().any(|p| {
                p.is_empty()
                    || p.len() > 80
                    || !p
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            })
        {
            return Err("Select valid maintenance providers".into());
        }
        if self.mode == Mode::Fixed && (self.proxy_id.is_empty() || self.proxy_id.len() > 100) {
            return Err("Fixed mode requires a proxy pool entry".into());
        }
        Ok(())
    }
    fn applies(&self, provider: &str, method: &str) -> bool {
        self.enabled
            && self.providers.iter().any(|p| p == provider)
            && match method {
                "usage.query" => self.balance,
                "checkin.claim" => self.checkin,
                "claim" | "claim.status" => self.rewards,
                _ => false,
            }
    }
}
pub fn current() -> Settings {
    config::with_raw(|raw| raw.get(KEY).cloned())
        .flatten()
        .and_then(|v| serde_json::from_value::<Settings>(v).ok())
        .unwrap_or_default()
}
pub fn for_call(provider: &str, method: &str) -> Option<Settings> {
    let s = current();
    s.applies(provider, method).then_some(s)
}
pub fn configure(value: Value) -> Result<Settings, String> {
    let settings: Settings =
        serde_json::from_value(value).map_err(|_| "Invalid maintenance settings")?;
    settings.validate()?;
    if settings.enabled && settings.mode == Mode::Fixed {
        proxy_pool::resolve_reference(&settings.proxy_id)
            .map_err(|_| "Selected maintenance proxy is unavailable")?;
    }
    let value =
        serde_json::to_value(&settings).map_err(|_| "Cannot serialize maintenance settings")?;
    if !config::update_raw_field(KEY, value) {
        return Err("Cannot persist maintenance settings".into());
    }
    Ok(settings)
}

#[derive(Clone)]
pub struct Candidate {
    id: String,
    proxy: Value,
}
fn candidates(settings: &Settings) -> Result<Vec<Candidate>, GatewayError> {
    if settings.mode == Mode::Direct {
        return Ok(vec![Candidate {
            id: "direct".into(),
            proxy: Value::Null,
        }]);
    }
    let ids = if settings.mode == Mode::Fixed {
        vec![settings.proxy_id.clone()]
    } else {
        proxy_pool::list()
            .iter()
            .filter(|v| v.get("enabled").and_then(Value::as_bool) == Some(true))
            .filter_map(|v| v.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect()
    };
    let result: Vec<_> = ids
        .into_iter()
        .filter_map(|id| {
            let p = proxy_pool::resolve_reference(&id).ok()?;
            // Credentials stay in memory and the private stdio RPC; API exposes IDs only.
            let host = if p.host.contains(':') && !p.host.starts_with('[') {
                format!("[{}]", p.host)
            } else {
                p.host
            };
            Some(Candidate {
                id,
                proxy: json!({"protocol":p.protocol,"host":host,"port":p.port,
            "username":p.username,"password":p.password}),
            })
        })
        .collect();
    if result.is_empty() {
        return Err(error(
            503,
            "No enabled maintenance proxy is available; direct fallback is disabled",
        ));
    }
    Ok(result)
}
fn error(status: i32, message: &str) -> GatewayError {
    GatewayError::with_status(status, message)
}
fn jitter(min: u64, max: u64) -> u64 {
    let mut bytes = [0_u8; 8];
    if getrandom::getrandom(&mut bytes).is_err() {
        return max;
    }
    min + u64::from_le_bytes(bytes) % (max - min + 1)
}
type AccountKey = (String, String);
struct Cache {
    at: Instant,
    value: Value,
    settings: Settings,
}
#[derive(Default)]
struct State {
    active: usize,
    pending: usize,
    accounts: HashSet<AccountKey>,
    per_proxy: HashMap<String, usize>,
    next_start: Option<Instant>,
    cursor: usize,
    cooldown: HashMap<String, Instant>,
    cache: HashMap<AccountKey, Cache>,
}
#[derive(Default)]
struct Coordinator {
    state: Mutex<State>,
    notify: Notify,
}
fn coordinator() -> Arc<Coordinator> {
    static INSTANCE: OnceLock<Arc<Coordinator>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| Arc::new(Coordinator::default()))
        .clone()
}
pub fn snapshot() -> Value {
    let c = coordinator();
    let value = match c.state.lock() {
        Ok(s) => json!({"active":s.active,"pending":s.pending}),
        Err(_) => json!({"unavailable":true}),
    };
    value
}
struct Waiting {
    coordinator: Arc<Coordinator>,
    admitted: bool,
}
impl Drop for Waiting {
    fn drop(&mut self) {
        if !self.admitted {
            if let Ok(mut s) = self.coordinator.state.lock() {
                s.pending = s.pending.saturating_sub(1);
            }
        }
        self.coordinator.notify.notify_waiters();
    }
}
pub enum Admission {
    Cached(Value),
    Run(Permit),
}
pub struct Permit {
    coordinator: Arc<Coordinator>,
    key: AccountKey,
    method: String,
    settings: Settings,
    candidate: Candidate,
}
impl Permit {
    pub fn apply(&self, params: &mut Value) {
        params["maintenanceEgress"] = json!({"version":1,"proxy":self.candidate.proxy});
    }
    pub fn finish(&self, result: &Result<Value, agent2api_provider_protocol::WireError>) {
        if let Ok(mut s) = self.coordinator.state.lock() {
            if matches!(self.method.as_str(), "claim" | "checkin.claim") {
                // A reward may be committed even if the response is lost.
                s.cache.remove(&self.key);
            }
            if self.method == "usage.query" {
                if let Ok(value) = result {
                    // A hard bound prevents dormant/imported account IDs growing memory forever.
                    if s.cache.len() >= 4096 {
                        s.cache.clear();
                    }
                    s.cache.insert(
                        self.key.clone(),
                        Cache {
                            at: Instant::now(),
                            value: value.clone(),
                            settings: self.settings.clone(),
                        },
                    );
                }
            }
            // Never replay a possibly committed check-in/reward. Timeouts cool down this
            // entrance for FUTURE auto-mode tasks; auth/HTTP business errors do not. Typed transport errors also cool down.
            if self.settings.mode == Mode::Auto
                && matches!(
                    result.as_ref().err().and_then(|e| e.code.as_deref()),
                    Some("timeout" | "maintenance_transport")
                )
            {
                s.cooldown.insert(
                    self.candidate.id.clone(),
                    Instant::now() + Duration::from_secs(self.settings.failure_cooldown_seconds),
                );
            }
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut s) = self.coordinator.state.lock() {
            s.active = s.active.saturating_sub(1);
            s.accounts.remove(&self.key);
            if let Some(n) = s.per_proxy.get_mut(&self.candidate.id) {
                *n = n.saturating_sub(1);
            }
            s.per_proxy.retain(|_, n| *n > 0);
        }
        self.coordinator.notify.notify_waiters();
    }
}
pub async fn acquire(
    settings: Settings,
    provider: &str,
    method: &str,
    account: &str,
) -> Result<Admission, GatewayError> {
    acquire_with(
        coordinator(),
        settings,
        provider,
        method,
        account,
        candidates,
    )
    .await
}
async fn acquire_with<F>(
    c: Arc<Coordinator>,
    settings: Settings,
    provider: &str,
    method: &str,
    account: &str,
    resolve: F,
) -> Result<Admission, GatewayError>
where
    F: Fn(&Settings) -> Result<Vec<Candidate>, GatewayError>,
{
    settings
        .validate()
        .map_err(|_| error(503, "Invalid persisted maintenance policy"))?;
    if !OPERATIONS.contains(&method) {
        return Err(error(400, "Not a maintenance operation"));
    }
    let key = (provider.to_string(), account.to_string());
    {
        let mut s = c
            .state
            .lock()
            .map_err(|_| error(503, "Maintenance scheduler unavailable"))?;
        if s.pending >= settings.max_pending {
            return Err(error(429, "Maintenance queue is full; try later"));
        }
        s.pending += 1;
    }
    let mut waiting = Waiting {
        coordinator: c.clone(),
        admitted: false,
    };
    let deadline = Instant::now() + Duration::from_secs(settings.queue_timeout_seconds);
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(error(429, "Maintenance queue wait timed out"));
        }
        let notified = c.notify.notified();
        let mut wake_after = Duration::from_secs(1);
        {
            let mut s = c
                .state
                .lock()
                .map_err(|_| error(503, "Maintenance scheduler unavailable"))?;
            s.cooldown.retain(|_, until| *until > now);
            if let Some(next) = s.next_start.filter(|t| *t > now) {
                wake_after = wake_after.min(next.duration_since(now));
            }
            if let Some(next) = s.cooldown.values().min() {
                wake_after = wake_after.min(next.saturating_duration_since(now));
            }
            if method == "usage.query" && settings.usage_cache_seconds > 0 {
                if let Some(cached) = s.cache.get(&key) {
                    if cached.settings == settings
                        && now.duration_since(cached.at)
                            < Duration::from_secs(settings.usage_cache_seconds)
                    {
                        return Ok(Admission::Cached(cached.value.clone()));
                    }
                }
            }
            if s.active < settings.max_concurrent
                && !s.accounts.contains(&key)
                && s.next_start.map(|t| now >= t).unwrap_or(true)
            {
                // Resolve only when a task can start, not once per queued waiter/tick.
                // Pool updates are observed at admission; no stale endpoint snapshots.
                let options = resolve(&settings)?;
                let len = options.len();
                let chosen = (0..len).map(|i| (s.cursor + i) % len).find(|&i| {
                    let p = &options[i];
                    s.per_proxy.get(&p.id).copied().unwrap_or(0) < settings.per_proxy_concurrent
                        && (settings.mode != Mode::Auto || !s.cooldown.contains_key(&p.id))
                });
                if let Some(i) = chosen {
                    let candidate = options[i].clone();
                    s.cursor = (i + 1) % len;
                    s.pending = s.pending.saturating_sub(1);
                    s.active += 1;
                    s.accounts.insert(key.clone());
                    *s.per_proxy.entry(candidate.id.clone()).or_default() += 1;
                    s.next_start = Some(
                        now + Duration::from_millis(jitter(
                            settings.min_interval_ms,
                            settings.max_interval_ms,
                        )),
                    );
                    waiting.admitted = true;
                    return Ok(Admission::Run(Permit {
                        coordinator: c.clone(),
                        key,
                        method: method.to_string(),
                        settings,
                        candidate,
                    }));
                }
            }
        }
        // Bounded polling also observes settings in the external native proxy pool.
        tokio::select! { _=notified=>{}, _=tokio::time::sleep(wake_after.min(deadline.saturating_duration_since(now)))=>{} }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> Settings {
        Settings {
            enabled: true,
            min_interval_ms: 0,
            max_interval_ms: 0,
            ..Settings::default()
        }
    }
    fn direct(_: &Settings) -> Result<Vec<Candidate>, GatewayError> {
        Ok(vec![Candidate {
            id: "direct".into(),
            proxy: Value::Null,
        }])
    }
    fn pool(_: &Settings) -> Result<Vec<Candidate>, GatewayError> {
        Ok(vec![
            Candidate {
                id: "a".into(),
                proxy: json!({}),
            },
            Candidate {
                id: "b".into(),
                proxy: json!({}),
            },
        ])
    }
    async fn permit(c: Arc<Coordinator>, s: Settings, a: &str) -> Permit {
        match acquire_with(c, s, "raccoon", "usage.query", a, direct)
            .await
            .unwrap()
        {
            Admission::Run(p) => p,
            _ => panic!("unexpected cache"),
        }
    }
    #[test]
    fn validates_modes_limits_and_scope() {
        let mut s = settings();
        assert!(s.validate().is_ok());
        for m in ["chat", "conversation", "models.refresh", "token.refresh"] {
            assert!(!s.applies("raccoon", m));
        }
        assert!(s.applies("raccoon", "checkin.claim"));
        assert!(!s.applies("other", "usage.query"));
        s.mode = Mode::Fixed;
        assert!(s.validate().is_err());
        s.proxy_id = "test-pool".into();
        assert!(s.validate().is_ok());
        s.min_interval_ms = 2;
        s.max_interval_ms = 1;
        assert!(s.validate().is_err());
        assert!(serde_json::from_value::<Settings>(json!({"unknown":true})).is_err());
        for _ in 0..100 {
            let n = jitter(20, 25);
            assert!((20..=25).contains(&n));
        }
    }
    #[tokio::test]
    async fn concurrency_cancellation_and_queue_bound() {
        let c = Arc::new(Coordinator::default());
        let mut s = settings();
        s.max_concurrent = 1;
        s.max_pending = 1;
        let first = permit(c.clone(), s.clone(), "one").await;
        let c2 = c.clone();
        let s2 = s.clone();
        let waiting = tokio::spawn(async move { permit(c2, s2, "two").await });
        tokio::task::yield_now().await;
        assert!(
            acquire_with(c.clone(), s.clone(), "raccoon", "claim", "three", direct)
                .await
                .is_err()
        );
        waiting.abort();
        let _ = waiting.await;
        assert_eq!(c.state.lock().unwrap().pending, 0);
        drop(first);
        let next = permit(c.clone(), s, "new-account").await;
        drop(next);
        let state = c.state.lock().unwrap();
        assert_eq!(state.active, 0);
        assert!(state.accounts.is_empty());
    }
    #[tokio::test]
    async fn auto_rotation_and_per_proxy_limit() {
        let c = Arc::new(Coordinator::default());
        let mut s = settings();
        s.mode = Mode::Auto;
        let a = acquire_with(c.clone(), s.clone(), "raccoon", "claim", "one", pool)
            .await
            .unwrap();
        let b = acquire_with(c.clone(), s.clone(), "loomy", "claim", "two", pool)
            .await
            .unwrap();
        if let (Admission::Run(a), Admission::Run(b)) = (a, b) {
            assert_ne!(a.candidate.id, b.candidate.id);
            assert_eq!(c.state.lock().unwrap().active, 2);
        }
        assert_eq!(c.state.lock().unwrap().active, 0);
    }
    #[tokio::test]
    async fn successful_usage_cached_but_rewards_never_cached() {
        let c = Arc::new(Coordinator::default());
        let s = settings();
        let p = permit(c.clone(), s.clone(), "one").await;
        p.finish(&Ok(json!({"available":123})));
        drop(p);
        assert!(matches!(
            acquire_with(
                c.clone(),
                s.clone(),
                "raccoon",
                "usage.query",
                "one",
                direct
            )
            .await
            .unwrap(),
            Admission::Cached(_)
        ));
        assert!(matches!(
            acquire_with(c, s, "raccoon", "claim", "one", direct)
                .await
                .unwrap(),
            Admission::Run(_)
        ));
    }
    #[tokio::test]
    async fn starts_are_spaced_and_account_is_serialized() {
        let c = Arc::new(Coordinator::default());
        let mut s = settings();
        s.min_interval_ms = 35;
        s.max_interval_ms = 35;
        s.per_proxy_concurrent = 2;
        s.usage_cache_seconds = 0;
        let p = permit(c.clone(), s.clone(), "one").await;
        let start = Instant::now();
        let q = permit(c.clone(), s.clone(), "two").await;
        assert!(start.elapsed() >= Duration::from_millis(35));
        drop(q);
        let c2 = c.clone();
        let task = tokio::spawn(async move { permit(c2, s, "one").await });
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!task.is_finished());
        drop(p);
        drop(task.await.unwrap());
    }
    #[tokio::test]
    async fn no_direct_fallback_and_no_slot_leak_on_resolution_error() {
        let c = Arc::new(Coordinator::default());
        let s = settings();
        let result = acquire_with(c.clone(), s, "raccoon", "claim", "one", |_| {
            Err(error(503, "Unavailable"))
        })
        .await;
        assert!(result.is_err());
        let state = c.state.lock().unwrap();
        assert_eq!(state.pending, 0);
        assert_eq!(state.active, 0);
    }
    #[tokio::test]
    async fn transport_cooldown_rotates_without_replaying_mutation() {
        let c = Arc::new(Coordinator::default());
        let mut s = settings();
        s.mode = Mode::Auto;
        let first = acquire_with(
            c.clone(),
            s.clone(),
            "raccoon",
            "checkin.claim",
            "one",
            pool,
        )
        .await
        .unwrap();
        if let Admission::Run(p) = first {
            assert_eq!(p.candidate.id, "a");
            p.finish(&Err(agent2api_provider_protocol::WireError::with_code(
                "fixture transport failure",
                "maintenance_transport",
            )));
        }
        c.state.lock().unwrap().cursor = 0;
        let next = acquire_with(c.clone(), s, "raccoon", "checkin.claim", "two", pool)
            .await
            .unwrap();
        if let Admission::Run(p) = next {
            assert_eq!(p.candidate.id, "b");
        }
        assert_eq!(c.state.lock().unwrap().active, 0);
    }
    #[tokio::test]
    async fn mutation_invalidates_usage_even_when_response_is_lost() {
        let c = Arc::new(Coordinator::default());
        let s = settings();
        let p = permit(c.clone(), s.clone(), "one").await;
        p.finish(&Ok(json!({"available":1})));
        drop(p);
        let mutation = acquire_with(c.clone(), s.clone(), "raccoon", "claim", "one", direct)
            .await
            .unwrap();
        if let Admission::Run(p) = mutation {
            p.finish(&Err(agent2api_provider_protocol::WireError::with_code(
                "fixture", "timeout",
            )));
        }
        assert!(matches!(
            acquire_with(c, s, "raccoon", "usage.query", "one", direct)
                .await
                .unwrap(),
            Admission::Run(_)
        ));
    }
}
