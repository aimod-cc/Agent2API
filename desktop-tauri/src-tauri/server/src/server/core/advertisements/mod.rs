use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::{db::Db, logging};

mod network;
mod types;
use types::{image_url, Feed, MAX_FEED_BYTES, MAX_TTL_MS, WEBSITE_ORIGIN};

const CACHE_KEY: &str = "advertisementCache";
const RETRY_SECONDS: u64 = 30;

#[derive(Clone)]
pub struct AdvertisementCache {
    inner: Arc<Inner>,
}

struct Inner {
    db: Option<Db>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    cached: Option<Stored>,
    refresh_at: Option<Instant>,
    valid_until: Option<Instant>,
    observed_at: Option<Instant>,
    server_observed: i64,
    refreshing: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    feed: Feed,
    received_at: i64,
    server_received_at: i64,
    expires_at: i64,
}

impl AdvertisementCache {
    pub fn new(db: Option<Db>) -> Self {
        let cached = db
            .as_ref()
            .and_then(|db| {
                db.with(|conn| {
                    conn.query_row("SELECT value FROM kv WHERE key = ?1", [CACHE_KEY], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()
                    .ok()
                    .flatten()
                })
            })
            .flatten()
            .filter(|text| text.len() <= MAX_FEED_BYTES)
            .and_then(|text| serde_json::from_str::<Stored>(&text).ok())
            .filter(|stored| {
                stored.feed.validate().is_ok()
                    && stored.received_at <= logging::now_ms()
                    && stored.server_received_at >= stored.feed.server_time
                    && stored.server_received_at <= stored.feed.expires_at
                    && stored.expires_at > logging::now_ms()
                    && stored.expires_at.saturating_sub(stored.received_at) <= MAX_TTL_MS
            });
        let wall_now = logging::now_ms();
        let monotonic_now = Instant::now();
        let valid_until = cached.as_ref().map(|stored| {
            monotonic_now
                + Duration::from_millis(
                    stored
                        .expires_at
                        .saturating_sub(wall_now)
                        .clamp(0, MAX_TTL_MS) as u64,
                )
        });
        let server_observed = cached
            .as_ref()
            .map(|stored| {
                stored
                    .server_received_at
                    .saturating_add(wall_now.saturating_sub(stored.received_at))
            })
            .unwrap_or(0);
        Self {
            inner: Arc::new(Inner {
                db,
                state: Mutex::new(State {
                    cached,
                    valid_until,
                    observed_at: Some(monotonic_now),
                    server_observed,
                    ..State::default()
                }),
            }),
        }
    }

    pub fn snapshot(&self) -> Value {
        self.refresh_if_due();
        let Ok(state) = self.inner.state.lock() else {
            return empty();
        };
        let instant = Instant::now();
        let Some(stored) = state.cached.as_ref() else {
            return empty();
        };
        let Some(deadline) = state.valid_until.filter(|deadline| *deadline > instant) else {
            return empty();
        };
        let server_now = state.server_observed.saturating_add(
            state
                .observed_at
                .map(|at| {
                    instant
                        .saturating_duration_since(at)
                        .as_millis()
                        .min(i64::MAX as u128) as i64
                })
                .unwrap_or(0),
        );
        let items: Vec<Value> = if stored.feed.enabled {
            stored
                .feed
                .items
                .iter()
                .filter(|ad| {
                    ad.starts_at.is_none_or(|at| at <= server_now)
                        && ad.ends_at.is_none_or(|at| at > server_now)
                })
                .map(|ad| {
                    let mut value = serde_json::to_value(ad).unwrap_or(Value::Null);
                    value["imageUrl"] = json!(image_url(&ad.image_url));
                    value["imageDarkUrl"] = json!(image_url(&ad.image_dark_url));
                    value
                })
                .collect()
        } else {
            Vec::new()
        };
        json!({ "schemaVersion": 1, "revision": stored.feed.revision, "enabled": stored.feed.enabled,
            "serverTime": server_now, "expiresInSeconds": deadline.saturating_duration_since(instant).as_secs(), "items": items })
    }

    fn refresh_if_due(&self) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        if state.refreshing || state.refresh_at.is_some_and(|at| at > Instant::now()) {
            return;
        }
        state.refreshing = true;
        drop(state);
        let cache = self.clone();
        crate::spawn_task(async move {
            let started = Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(12), fetch_feed())
                .await
                .map_err(|_| "官网广告配置请求超时".to_string())
                .and_then(|result| result);
            let mut accepted = None;
            if let Ok(mut state) = cache.inner.state.lock() {
                match result {
                    Ok(feed) => {
                        let now = logging::now_ms();
                        let elapsed = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
                        let lifetime = feed
                            .expires_at
                            .saturating_sub(feed.server_time)
                            .min(MAX_TTL_MS);
                        let ttl = lifetime.saturating_sub(elapsed).max(0);
                        let refresh = feed.refresh_after_seconds;
                        let server_observed = feed.server_time.saturating_add(elapsed);
                        let stored = Stored {
                            feed,
                            received_at: now,
                            server_received_at: server_observed,
                            expires_at: now.saturating_add(ttl),
                        };
                        let observed = Instant::now();
                        state.valid_until = Some(observed + Duration::from_millis(ttl as u64));
                        state.observed_at = Some(observed);
                        state.server_observed = server_observed;
                        state.cached = Some(stored.clone());
                        state.refresh_at = Some(observed + Duration::from_secs(refresh));
                        accepted = Some(stored);
                    }
                    Err(error) => {
                        state.refresh_at =
                            Some(Instant::now() + Duration::from_secs(RETRY_SECONDS));
                        logging::verbose("[Advertisements]", &error);
                    }
                }
                state.refreshing = false;
            }
            if let (Some(stored), Some(db)) = (accepted, cache.inner.db.clone()) {
                if let Ok(text) = serde_json::to_string(&stored) {
                    let _ = tokio::task::spawn_blocking(move || db.with(|conn| {
                        conn.execute("INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [CACHE_KEY, &text])
                    })).await;
                }
            }
        });
    }
}

fn empty() -> Value {
    json!({ "schemaVersion": 1, "revision": 0, "enabled": false, "expiresInSeconds": 0, "items": [] })
}

async fn fetch_feed() -> Result<Feed, String> {
    let url = format!("{WEBSITE_ORIGIN}/api/advertisements");
    let mut response = network::request(&url).await?;
    if !response.status().is_success()
        || response.url().origin().ascii_serialization() != WEBSITE_ORIGIN
    {
        return Err("官网广告服务不可用".into());
    }
    if !response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
    {
        return Err("官网广告响应不是 JSON".into());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_FEED_BYTES as u64)
    {
        return Err("官网广告配置过大".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "读取官网广告配置失败")? {
        if bytes.len().saturating_add(chunk.len()) > MAX_FEED_BYTES {
            return Err("官网广告配置过大".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let envelope: Value = serde_json::from_slice(&bytes).map_err(|_| "官网广告配置格式无效")?;
    if envelope["success"] != true {
        return Err("官网广告服务返回失败".into());
    }
    let feed: Feed =
        serde_json::from_value(envelope["data"].clone()).map_err(|_| "官网广告配置格式无效")?;
    feed.validate()?;
    Ok(feed)
}
