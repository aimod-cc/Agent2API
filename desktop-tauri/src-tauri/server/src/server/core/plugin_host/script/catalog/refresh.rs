use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use super::cache;
use super::types::Entry;

/// 从官网 CDN 静态聚合目录拉取社区插件列表，默认每 6 小时由自动维护调度刷新一次。
const CATALOG_TTL_MS: i64 = 6 * 3_600_000;
const MIN_RETRY_MS: i64 = 5 * 60_000;
const MAX_RETRY_MS: i64 = 60 * 60_000;
const MAX_CATALOG_BYTES: usize = 16 * 1024 * 1024;

struct Worker {
    started: AtomicBool,
    failures: AtomicU32,
}

fn worker() -> &'static Worker {
    static WORKER: OnceLock<Worker> = OnceLock::new();
    WORKER.get_or_init(|| Worker {
        started: AtomicBool::new(false),
        failures: AtomicU32::new(0),
    })
}

fn refresh_lock() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

struct ProgressGuard;

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        cache::progress(false, 0, 0);
    }
}

fn noise(max: i64) -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEED: OnceLock<i64> = OnceLock::new();
    let seed = *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos() as i64)
            .unwrap_or(0)
    });
    if max <= 0 {
        0
    } else {
        seed.wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407)
            .rotate_left(17)
            .rem_euclid(max)
    }
}

pub(super) fn wake(force: bool) {
    if worker().started.swap(true, Ordering::AcqRel) && !force {
        return;
    }
    crate::spawn_task(async move {
        let _ = refresh_now(force).await;
        worker().started.store(false, Ordering::Release);
    });
}

pub(super) fn ensure_loaded() {
    if !crate::server::config::scheduled_settings().magpie_plugin_check.enabled {
        return;
    }
    let previous = cache::snapshot();
    let now = crate::server::logging::now_ms();
    let empty = previous.market.is_empty() && previous.github.is_empty();
    let stale = previous.market_at == 0 || now.saturating_sub(previous.market_at) >= CATALOG_TTL_MS;
    if (empty || stale) && (previous.market_retry <= now || empty && previous.market_retry == 0) {
        wake(false);
    }
}

pub(crate) async fn refresh_now(force: bool) -> Result<(usize, usize, Option<String>), String> {
    let _guard = refresh_lock().lock().await;
    refresh_inner(force).await
}

async fn refresh_inner(force: bool) -> Result<(usize, usize, Option<String>), String> {
    let previous = cache::snapshot();
    let now = crate::server::logging::now_ms();
    if !force {
        if previous.market_retry > now {
            return Err(previous.market_error.unwrap_or_else(|| "插件清单请求处于冷却期".into()));
        }
        if previous.market_retry == 0
            && previous.market_at > 0
            && now.saturating_sub(previous.market_at) < CATALOG_TTL_MS
        {
            return Ok((previous.market.len(), previous.github.len(), previous.market_error));
        }
    }

    cache::progress(true, 0, 0);
    let _progress = ProgressGuard;
    let fetched = tokio::time::timeout(
        Duration::from_secs(45),
        super::super::website::bytes_with_fallback(
            super::super::website::CATALOG,
            super::super::website::LEGACY_CATALOG,
            MAX_CATALOG_BYTES,
        ),
    )
    .await
    .map_err(|_| "拉取 Magpie 插件清单超时".to_string())
    .and_then(|result| result);

    let bytes = match fetched {
        Ok(bytes) => bytes,
        Err(error) => {
            let failures = worker().failures.fetch_add(1, Ordering::AcqRel).min(4);
            let wait = (MIN_RETRY_MS << failures).min(MAX_RETRY_MS);
            let retry_at = now + wait + noise(60_000);
            cache::update(|cache| {
                cache.market_error = Some(error.clone());
                cache.market_retry = retry_at;
                cache.github_retry = retry_at;
            });
            cache::persist();
            return Err(error);
        }
    };

    let parsed = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if value.get("schemaVersion").and_then(Value::as_u64) == Some(1) => value,
        _ => {
            let retry_at = now + MIN_RETRY_MS + noise(60_000);
            let message = "官网社区插件目录格式无效".to_string();
            cache::update(|cache| {
                cache.market_error = Some(message.clone());
                cache.market_retry = retry_at;
                cache.github_retry = retry_at;
            });
            cache::persist();
            return Err(message);
        }
    };

    let Some(raw_entries) = parsed.get("entries").and_then(Value::as_array) else {
        let retry_at = now + MIN_RETRY_MS + noise(60_000);
        let message = "官网社区插件目录缺少条目列表".to_string();
        cache::update(|cache| {
            cache.market_error = Some(message.clone());
            cache.market_retry = retry_at;
        });
        cache::persist();
        return Err(message);
    };

    let mut market_entries = Vec::new();
    let mut github_entries = Vec::new();
    for raw in raw_entries {
        let Ok(entry) = serde_json::from_value::<Entry>(raw.clone()) else {
            continue;
        };
        if !entry.is_provider() {
            continue;
        }
        if entry.publisher == "third-party" {
            github_entries.push(entry);
        } else {
            market_entries.push(entry);
        }
    }

    worker().failures.store(0, Ordering::Release);
    let warning = parsed
        .get("warning")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);

    let market_count = market_entries.len();
    let github_count = github_entries.len();
    cache::update(|cache| {
        cache.market = market_entries;
        cache.github = github_entries;
        cache.market_at = now;
        cache.github_at = now;
        cache.market_retry = 0;
        cache.github_retry = 0;
        cache.market_error = warning.clone();
        cache.github_error = None;
        cache.github_missing = 0;
    });
    cache::persist();
    super::super::icons::wake(false);
    Ok((market_count, github_count, warning))
}
