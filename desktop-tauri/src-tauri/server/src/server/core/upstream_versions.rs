//! 插件声明的上游客户端版本号同步：定时从官网拉取并持久化，带手动触发与本地限流。

use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::Notify;

use crate::server::core::update::website;
use crate::server::db::Db;
use crate::server::errors::GatewayError;
use crate::server::{config, logging};

pub const CONFIG_KEY: &str = "upstreamVersionSync";
const CACHE_KV_KEY: &str = "upstreamVersionCache";
pub const DEFAULT_INTERVAL_HOURS: i64 = 6;
pub const MIN_INTERVAL_HOURS: i64 = 1;
pub const MAX_INTERVAL_HOURS: i64 = 24;
pub const MANUAL_COOLDOWN_MS: i64 = 60_000;
const TICK_SECS: u64 = 30;
const REQUEST_TIMEOUT_MS: u64 = 12_000;
const MAX_FEED_BYTES: usize = 256 * 1024;
const MIN_BACKOFF_MS: i64 = 5 * 60_000;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct CachedEntry {
    version: String,
    tag: String,
    repository: String,
    published_at: String,
    synced_at: i64,
    last_attempt_at: i64,
    last_manual_at: i64,
    next_run_at: i64,
    last_error: Option<String>,
    failures: u32,
}

#[derive(Default)]
struct State {
    db: Option<Db>,
    entries: BTreeMap<String, CachedEntry>,
    in_flight: HashSet<String>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn notifier() -> &'static Notify {
    static NOTIFY: OnceLock<Notify> = OnceLock::new();
    NOTIFY.get_or_init(Notify::new)
}

fn fetch_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub fn wake() {
    notifier().notify_one();
}

pub fn valid_version(raw: &str) -> bool {
    let value = raw.trim();
    !value.is_empty()
        && value.len() <= 32
        && value.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

pub fn valid_source_id(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 128
        && raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && !matches!(raw, "__proto__" | "prototype" | "constructor")
}

pub fn install(db: Option<Db>) {
    let loaded = db
        .as_ref()
        .and_then(|db| {
            db.with(|conn| {
                conn.query_row("SELECT value FROM kv WHERE key = ?1", [CACHE_KV_KEY], |row| {
                    row.get::<_, String>(0)
                })
                .optional()
                .ok()
                .flatten()
            })
        })
        .flatten()
        .filter(|text| text.len() <= 2 * 1024 * 1024)
        .and_then(|text| serde_json::from_str::<BTreeMap<String, CachedEntry>>(&text).ok())
        .unwrap_or_default();

    if let Ok(mut guard) = state().lock() {
        guard.db = db;
        guard.entries = loaded;
        guard.in_flight.clear();
    }
}

fn persist_entries(db: Option<Db>, entries: &BTreeMap<String, CachedEntry>) {
    let Some(db) = db else { return };
    let Ok(text) = serde_json::to_string(entries) else { return };
    let _ = db.with(move |conn| {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [CACHE_KV_KEY, &text],
        )
    });
}

async fn persist_async(db: Option<Db>, entries: BTreeMap<String, CachedEntry>) {
    let Some(db) = db else { return };
    let Ok(text) = serde_json::to_string(&entries) else { return };
    let _ = crate::server::core::blocking::run(move || {
        db.with(move |conn| {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [CACHE_KV_KEY, &text],
            )
        })
    })
    .await;
}

pub fn configured_interval_from_raw(raw: &Map<String, Value>, source_id: &str) -> Option<i64> {
    raw.get(CONFIG_KEY)?
        .get(source_id)?
        .get("intervalHours")?
        .as_i64()
        .filter(|hours| (MIN_INTERVAL_HOURS..=MAX_INTERVAL_HOURS).contains(hours))
        .filter(|hours| *hours != DEFAULT_INTERVAL_HOURS)
}

pub fn effective_interval_hours(source_id: &str) -> i64 {
    config::with_raw(|raw| configured_interval_from_raw(raw, source_id))
        .flatten()
        .unwrap_or(DEFAULT_INTERVAL_HOURS)
}

pub fn set_interval_in_raw(
    raw: &mut Map<String, Value>,
    source_id: &str,
    hours: Option<i64>,
) -> Result<(), String> {
    if !valid_source_id(source_id) {
        return Err("上游版本源标识无效".into());
    }
    if let Some(hours) = hours {
        if !(MIN_INTERVAL_HOURS..=MAX_INTERVAL_HOURS).contains(&hours) {
            return Err(format!(
                "定时拉取间隔必须是 {MIN_INTERVAL_HOURS}–{MAX_INTERVAL_HOURS} 小时的整数"
            ));
        }
    }
    let normalized = hours.filter(|value| *value != DEFAULT_INTERVAL_HOURS);
    match normalized {
        Some(value) => {
            let root = raw.entry(CONFIG_KEY).or_insert_with(|| json!({}));
            if !root.is_object() {
                *root = json!({});
            }
            if let Some(map) = root.as_object_mut() {
                map.insert(source_id.to_string(), json!({ "intervalHours": value }));
            }
        }
        None => {
            if let Some(map) = raw.get_mut(CONFIG_KEY).and_then(Value::as_object_mut) {
                map.remove(source_id);
                if map.is_empty() {
                    raw.remove(CONFIG_KEY);
                }
            }
        }
    }
    Ok(())
}

pub fn reschedule(source_id: &str, hours: i64) {
    let clamped = hours.clamp(MIN_INTERVAL_HOURS, MAX_INTERVAL_HOURS);
    let snapshot = {
        let Ok(mut guard) = state().lock() else { return };
        let entry = guard.entries.entry(source_id.to_string()).or_default();
        entry.next_run_at = if entry.last_attempt_at > 0 {
            entry
                .last_attempt_at
                .saturating_add(clamped.saturating_mul(3_600_000))
        } else {
            0
        };
        (guard.db.clone(), guard.entries.clone())
    };
    persist_entries(snapshot.0, &snapshot.1);
    wake();
}

pub fn cached_version(source_id: &str) -> Option<String> {
    let guard = state().lock().ok()?;
    let entry = guard.entries.get(source_id)?;
    let trimmed = entry.version.trim();
    valid_version(trimmed).then(|| trimmed.to_string())
}

fn cooldown_remaining_secs(entry: &CachedEntry, now: i64) -> i64 {
    if entry.last_manual_at <= 0 || entry.last_manual_at > now {
        return 0;
    }
    let elapsed = now.saturating_sub(entry.last_manual_at);
    if elapsed >= MANUAL_COOLDOWN_MS {
        return 0;
    }
    ((MANUAL_COOLDOWN_MS - elapsed + 999) / 1000).max(1)
}

pub fn status_json(source_id: &str) -> Value {
    let now = logging::now_ms();
    let interval_hours = effective_interval_hours(source_id);
    let (entry, syncing) = state()
        .lock()
        .map(|guard| {
            (
                guard.entries.get(source_id).cloned().unwrap_or_default(),
                guard.in_flight.contains(source_id),
            )
        })
        .unwrap_or_default();
    let version = if valid_version(entry.version.trim()) {
        json!(entry.version.trim())
    } else {
        Value::Null
    };
    json!({
        "sourceId": source_id,
        "version": version,
        "tag": entry.tag,
        "repository": entry.repository,
        "publishedAt": entry.published_at,
        "syncedAt": entry.synced_at,
        "lastAttemptAt": entry.last_attempt_at,
        "lastManualAt": entry.last_manual_at,
        "nextRunAt": entry.next_run_at,
        "cooldownRemainingSeconds": cooldown_remaining_secs(&entry, now),
        "intervalHours": interval_hours,
        "minHours": MIN_INTERVAL_HOURS,
        "maxHours": MAX_INTERVAL_HOURS,
        "defaultHours": DEFAULT_INTERVAL_HOURS,
        "syncing": syncing,
        "lastError": entry.last_error,
    })
}

struct InFlightGuard {
    sources: Vec<String>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = state().lock() {
            for source_id in &self.sources {
                guard.in_flight.remove(source_id);
            }
        }
    }
}

#[derive(Clone, Debug)]
struct FetchedVersion {
    version: String,
    tag: String,
    repository: String,
    published_at: String,
}

async fn fetch_remote_versions() -> Result<BTreeMap<String, FetchedVersion>, String> {
    let response = website::fetch_json_limit(
        website::CDN_UPSTREAM_VERSIONS_URL,
        REQUEST_TIMEOUT_MS,
        MAX_FEED_BYTES,
    )
    .await;
    let payload = match response {
        Ok(res) if res.status.is_success() => res.value,
        _ => {
            let fallback = website::fetch_json_limit(
                website::API_UPSTREAM_VERSIONS_URL,
                REQUEST_TIMEOUT_MS,
                MAX_FEED_BYTES,
            )
            .await
            .map_err(|error| error.message)?;
            if !fallback.status.is_success() {
                return Err(format!("官网版本接口返回 HTTP {}", fallback.status.as_u16()));
            }
            fallback.value
        }
    };
    if payload["schemaVersion"] != 1 {
        return Err("官网上游版本清单格式不受支持".into());
    }
    let Some(map) = payload["versions"].as_object() else {
        return Err("官网上游版本清单缺少 versions 对象".into());
    };
    let mut parsed = BTreeMap::new();
    for (id, item) in map.iter().take(128) {
        if !valid_source_id(id) {
            continue;
        }
        let Some(version) = item["version"].as_str().map(str::trim).filter(|v| valid_version(v)) else {
            continue;
        };
        parsed.insert(
            id.clone(),
            FetchedVersion {
                version: version.to_string(),
                tag: item["tag"].as_str().unwrap_or("").trim().chars().take(100).collect(),
                repository: item["repository"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(200)
                    .collect(),
                published_at: item["publishedAt"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(100)
                    .collect(),
            },
        );
    }
    Ok(parsed)
}

fn backoff_ms(failures: u32, interval_ms: i64) -> i64 {
    let factor = 1_i64.checked_shl(failures.saturating_sub(1).min(6)).unwrap_or(64);
    MIN_BACKOFF_MS.saturating_mul(factor).min(interval_ms.max(MIN_BACKOFF_MS))
}

fn apply_fetch_result(
    targets: &[String],
    fetched: &Result<BTreeMap<String, FetchedVersion>, String>,
    now: i64,
) -> (Option<Db>, BTreeMap<String, CachedEntry>, Vec<(String, String)>, Option<String>) {
    let mut changes = Vec::new();
    let mut primary_error = None;
    let Ok(mut guard) = state().lock() else {
        return (None, BTreeMap::new(), changes, Some("上游版本状态锁不可用".into()));
    };
    for source_id in targets {
        let interval_ms = effective_interval_hours(source_id).saturating_mul(3_600_000);
        let entry = guard.entries.entry(source_id.clone()).or_default();
        entry.last_attempt_at = now;
        match fetched {
            Ok(map) => {
                if let Some(item) = map.get(source_id) {
                    if entry.version != item.version {
                        changes.push((source_id.clone(), item.version.clone()));
                    }
                    entry.version = item.version.clone();
                    entry.tag = item.tag.clone();
                    entry.repository = item.repository.clone();
                    entry.published_at = item.published_at.clone();
                    entry.synced_at = now;
                    entry.next_run_at = now.saturating_add(interval_ms);
                    entry.failures = 0;
                    entry.last_error = None;
                } else {
                    let message = "官网尚未同步到该上游的有效正式版本号".to_string();
                    entry.failures = entry.failures.saturating_add(1);
                    entry.next_run_at = now.saturating_add(backoff_ms(entry.failures, interval_ms));
                    entry.last_error = Some(message.clone());
                    if primary_error.is_none() {
                        primary_error = Some(message);
                    }
                }
            }
            Err(message) => {
                entry.failures = entry.failures.saturating_add(1);
                entry.next_run_at = now.saturating_add(backoff_ms(entry.failures, interval_ms));
                entry.last_error = Some(message.clone());
                if primary_error.is_none() {
                    primary_error = Some(message.clone());
                }
            }
        }
    }
    (guard.db.clone(), guard.entries.clone(), changes, primary_error)
}

pub async fn sync_manual(source_id: &str) -> Result<Value, GatewayError> {
    if !valid_source_id(source_id)
        || !crate::server::core::plugin_host::provider_settings::has_upstream_source(source_id)
    {
        return Err(GatewayError::with_status(404, "当前没有已启用的插件使用该上游版本源"));
    }

    let now = logging::now_ms();
    let pre_snapshot = {
        let mut guard = state()
            .lock()
            .map_err(|_| GatewayError::new("上游版本状态不可用"))?;
        if guard.in_flight.contains(source_id) {
            return Err(GatewayError::with_status(429, "正在同步最新版本，请稍候"));
        }
        if let Some(entry) = guard.entries.get(source_id) {
            let wait = cooldown_remaining_secs(entry, now);
            if wait > 0 {
                return Err(GatewayError::with_status(
                    429,
                    format!("操作过于频繁，请 {wait} 秒后再试"),
                )
                .with_retry_after_seconds(wait as u64));
            }
        }
        guard.in_flight.insert(source_id.to_string());
        let entry = guard.entries.entry(source_id.to_string()).or_default();
        entry.last_manual_at = now;
        entry.last_attempt_at = now;
        (guard.db.clone(), guard.entries.clone())
    };
    let _in_flight = InFlightGuard {
        sources: vec![source_id.to_string()],
    };
    persist_async(pre_snapshot.0, pre_snapshot.1).await;

    let _fetch_guard = fetch_lock().lock().await;
    let fetched = fetch_remote_versions().await;
    let finished = logging::now_ms();
    let targets = vec![source_id.to_string()];
    let (db, entries, changes, error) = apply_fetch_result(&targets, &fetched, finished);
    persist_async(db, entries).await;
    drop(_in_flight);

    for (id, version) in changes {
        logging::log(
            "[UpstreamVersion]",
            &format!("已从官网同步 {id} 最新版本：{version}"),
        );
    }
    if let Some(message) = error {
        logging::log_with_level(
            "[UpstreamVersion]",
            &format!("手动同步 {source_id} 最新版本失败：{message}"),
            "warn",
        );
        return Err(GatewayError::with_status(502, message));
    }

    let status = status_json(source_id);
    Ok(json!({
        "status": status,
        "version": cached_version(source_id),
    }))
}

async fn run_scheduled_round() {
    let sources = crate::server::core::plugin_host::provider_settings::declared_upstream_sources();
    if sources.is_empty() {
        return;
    }
    let now = logging::now_ms();
    let due: Vec<String> = {
        let Ok(mut guard) = state().lock() else { return };
        let mut selected = Vec::new();
        for source_id in sources {
            if guard.in_flight.contains(&source_id) {
                continue;
            }
            let due_now = guard
                .entries
                .get(&source_id)
                .is_none_or(|entry| entry.next_run_at <= 0 || now >= entry.next_run_at);
            if due_now {
                guard.in_flight.insert(source_id.clone());
                selected.push(source_id);
            }
        }
        selected
    };
    if due.is_empty() {
        return;
    }
    let _in_flight = InFlightGuard {
        sources: due.clone(),
    };
    let _fetch_guard = fetch_lock().lock().await;
    let fetched = fetch_remote_versions().await;
    let finished = logging::now_ms();
    let (db, entries, changes, error) = apply_fetch_result(&due, &fetched, finished);
    persist_async(db, entries).await;
    drop(_in_flight);

    for (id, version) in changes {
        logging::log(
            "[UpstreamVersion]",
            &format!("已自动更新 {id} 上游版本号：{version}"),
        );
    }
    if let Some(message) = error {
        logging::verbose(
            "[UpstreamVersion]",
            &format!("定时同步上游版本号未完成：{message}"),
        );
    }
}

pub fn spawn() {
    crate::spawn_task(async move {
        loop {
            run_scheduled_round().await;
            let _ = tokio::time::timeout(
                Duration::from_secs(TICK_SECS),
                notifier().notified(),
            )
            .await;
        }
    });
}
