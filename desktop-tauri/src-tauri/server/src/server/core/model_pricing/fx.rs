//! 美元兑人民币（USD → CNY）实时汇率获取与本地缓存（12 小时 TTL，离线回落 7.2）。

use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::core::egress;
use crate::server::db::Db;
use crate::server::logging;

const FX_URL: &str = "https://open.er-api.com/v6/latest/USD";
pub const FX_CACHE_KV_KEY: &str = "fxRateCache";
pub const FX_TTL_MS: i64 = 12 * 3600 * 1000;
const RETRY_AFTER_MS: i64 = 10 * 60 * 1000;
pub const FX_FALLBACK: f64 = 7.2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FxRateSnapshot {
    pub rate: f64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

impl FxRateSnapshot {
    pub fn is_stale(&self, now_ms: i64) -> bool {
        self.updated_at <= 0 || now_ms.saturating_sub(self.updated_at) >= FX_TTL_MS
    }

    pub fn to_json(&self, now_ms: i64) -> Value {
        serde_json::json!({
            "rate": self.rate,
            "updatedAt": self.updated_at,
            "stale": self.is_stale(now_ms),
            "fallback": self.updated_at <= 0,
        })
    }
}

#[derive(Default)]
struct FxState {
    cached: Option<FxRateSnapshot>,
    db_loaded: bool,
    refreshing: bool,
    failed_at: i64,
}

static DB: OnceLock<Option<Db>> = OnceLock::new();
static STATE: OnceLock<Mutex<FxState>> = OnceLock::new();

pub fn install(db: Option<Db>) {
    let loaded = db.as_ref().and_then(read_db_cache_from);
    let _ = DB.set(db);
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    guard.db_loaded = true;
    if let Some(snapshot) = loaded {
        guard.cached = Some(snapshot);
    }
}

fn state() -> &'static Mutex<FxState> {
    STATE.get_or_init(|| Mutex::new(FxState::default()))
}

fn read_db_cache_from(db: &Db) -> Option<FxRateSnapshot> {
    let raw: Option<String> = db.with(|conn: &Connection| {
        conn.query_row(
            "SELECT value FROM kv WHERE key = ?1",
            params![FX_CACHE_KV_KEY],
            |row| row.get(0),
        )
        .ok()
    })?;
    let parsed: FxRateSnapshot = serde_json::from_str(&raw?).ok()?;
    (parsed.rate.is_finite() && parsed.rate > 0.0).then_some(parsed)
}

fn write_db_cache(snapshot: &FxRateSnapshot) {
    let Some(Some(db)) = DB.get() else { return };
    let Ok(encoded) = serde_json::to_string(snapshot) else { return };
    let _ = db.with(|conn: &Connection| {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![FX_CACHE_KV_KEY, encoded],
        )
        .is_ok()
    });
}

/// 立即返回当前可用的汇率，并在过期且未处于失败冷却期时后台异步刷新。
///
/// 不在读取路径上访问 `Db`（缓存已在 `install` 时装入），避免调用方持有数据库锁时重入死锁。
pub fn current_rate_soon() -> FxRateSnapshot {
    let now = logging::now_ms();
    let (snapshot, should_refresh) = {
        let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
        guard.db_loaded = true;
        let current = guard.cached.clone().unwrap_or(FxRateSnapshot {
            rate: FX_FALLBACK,
            updated_at: 0,
        });
        let stale = current.is_stale(now);
        let cooling = guard.failed_at > 0 && now.saturating_sub(guard.failed_at) < RETRY_AFTER_MS;
        let can_spawn = tokio::runtime::Handle::try_current().is_ok();
        let trigger = stale && can_spawn && !guard.refreshing && !cooling;
        if trigger {
            guard.refreshing = true;
        }
        (current, trigger)
    };
    if should_refresh {
        crate::spawn_task(async move {
            let _ = refresh_live().await;
        });
    }
    snapshot
}

/// 主动向汇率接口拉取最新 USD → CNY 汇率。
pub async fn refresh_live() -> Result<FxRateSnapshot, String> {
    {
        let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
        guard.refreshing = true;
    }
    let result = fetch_from_network().await;
    let now = logging::now_ms();
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    guard.refreshing = false;
    match result {
        Ok(rate) => {
            let snap = FxRateSnapshot {
                rate,
                updated_at: now,
            };
            guard.cached = Some(snap.clone());
            guard.failed_at = 0;
            drop(guard);
            write_db_cache(&snap);
            Ok(snap)
        }
        Err(err) => {
            guard.failed_at = now;
            Err(err)
        }
    }
}

async fn fetch_from_network() -> Result<f64, String> {
    let client = egress::client_for_system_proxy();
    let response = client
        .get(FX_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "Agent2API")
        .timeout(std::time::Duration::from_secs(12))
        .send()
        .await
        .map_err(|err| format!("获取汇率失败: {}", egress::describe_error_detail(&err)))?;
    if !response.status().is_success() {
        return Err(format!("汇率接口返回 HTTP {}", response.status()));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|err| format!("解析汇率响应失败: {err}"))?;
    let rate = body
        .pointer("/rates/CNY")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| "汇率响应缺少有效 CNY 汇率".to_string())?;
    Ok((rate * 10_000.0).round() / 10_000.0)
}
