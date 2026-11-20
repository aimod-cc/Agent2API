use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Notify;

use super::cache;
use super::types::Entry;

/// 从官网 CDN 静态聚合目录拉取社区插件列表，默认每 6 小时刷新一次。
const CATALOG_TTL_MS: i64 = 6 * 3_600_000;
const MIN_RETRY_MS: i64 = 5 * 60_000;
const MAX_RETRY_MS: i64 = 60 * 60_000;
const MAX_CATALOG_BYTES: usize = 16 * 1024 * 1024;

struct Worker {
    wake: Notify,
    force: AtomicBool,
    started: AtomicBool,
    failures: AtomicU32,
}

fn worker() -> &'static Worker {
    static WORKER: OnceLock<Worker> = OnceLock::new();
    WORKER.get_or_init(|| Worker {
        wake: Notify::new(),
        force: AtomicBool::new(false),
        started: AtomicBool::new(false),
        failures: AtomicU32::new(0),
    })
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
    spawn();
    if force {
        worker().force.store(true, Ordering::Release);
    }
    worker().wake.notify_one();
}

pub(super) fn ensure_loaded() {
    let previous = cache::snapshot();
    let now = crate::server::logging::now_ms();
    let empty = previous.market.is_empty() && previous.github.is_empty();
    let stale = previous.market_at == 0 || now.saturating_sub(previous.market_at) >= CATALOG_TTL_MS;
    if (empty || stale) && (previous.market_retry <= now || empty && previous.market_retry == 0) {
        wake(false);
    }
}

fn spawn() {
    if worker().started.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::spawn_task(async {
        loop {
            let force = worker().force.swap(false, Ordering::AcqRel);
            refresh(force).await;
            cache::progress(false, 0, 0);
            tokio::select! {
                _ = worker().wake.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs((6 * 3600 + noise(600)) as u64)) => {},
            }
        }
    });
}

async fn refresh(force: bool) {
    let previous = cache::snapshot();
    let now = crate::server::logging::now_ms();
    if !force {
        if previous.market_retry > now {
            return;
        }
        if previous.market_retry == 0
            && previous.market_at > 0
            && now.saturating_sub(previous.market_at) < CATALOG_TTL_MS
        {
            return;
        }
    }

    cache::progress(true, 0, 0);
    let fetched = super::super::website::bytes_with_fallback(
        super::super::website::CATALOG,
        super::super::website::LEGACY_CATALOG,
        MAX_CATALOG_BYTES,
    )
    .await;

    let bytes = match fetched {
        Ok(bytes) => bytes,
        Err(error) => {
            let failures = worker().failures.fetch_add(1, Ordering::AcqRel).min(4);
            let wait = (MIN_RETRY_MS << failures).min(MAX_RETRY_MS);
            let retry_at = now + wait + noise(60_000);
            cache::update(|cache| {
                cache.market_error = Some(error);
                cache.market_retry = retry_at;
                cache.github_retry = retry_at;
            });
            cache::persist();
            return;
        }
    };

    let parsed = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if value.get("schemaVersion").and_then(Value::as_u64) == Some(1) => value,
        _ => {
            let retry_at = now + MIN_RETRY_MS + noise(60_000);
            cache::update(|cache| {
                cache.market_error = Some("官网社区插件目录格式无效".into());
                cache.market_retry = retry_at;
                cache.github_retry = retry_at;
            });
            cache::persist();
            return;
        }
    };

    let Some(raw_entries) = parsed.get("entries").and_then(Value::as_array) else {
        let retry_at = now + MIN_RETRY_MS + noise(60_000);
        cache::update(|cache| {
            cache.market_error = Some("官网社区插件目录缺少条目列表".into());
            cache.market_retry = retry_at;
        });
        cache::persist();
        return;
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

    cache::update(|cache| {
        cache.market = market_entries;
        cache.github = github_entries;
        cache.market_at = now;
        cache.github_at = now;
        cache.market_retry = 0;
        cache.github_retry = 0;
        cache.market_error = warning;
        cache.github_error = None;
        cache.github_missing = 0;
    });
    cache::persist();
    super::super::icons::wake(false);
}
