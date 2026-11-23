use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::server::{errors::GatewayError, logging};

pub(super) fn start() -> Result<OwnedSemaphorePermit, GatewayError> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(64))).clone().try_acquire_owned()
        .map_err(|_| GatewayError::with_status(429, "正在等待的短信登录过多，请稍后重试"))
}

struct Entry { retry_at: i64, window_until: i64, attempts: u8 }

fn table() -> &'static Mutex<HashMap<String, Entry>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(provider: &str, country: &str, phone: &str) -> String {
    format!("{:x}", Sha256::digest(format!("{provider}\0{country}\0{phone}").as_bytes()))
}

pub(super) fn reserve(provider: &str, country: &str, phone: &str, cooldown: u64) -> Result<(bool, i64), GatewayError> {
    let now = logging::now_ms();
    let mut table = table().lock().map_err(|_| GatewayError::with_status(503, "短信发送限制暂不可用"))?;
    table.retain(|_, entry| entry.window_until > now || entry.retry_at > now);
    let key = key(provider, country, phone);
    if table.len() >= 4096 && !table.contains_key(&key) {
        return Err(GatewayError::with_status(429, "短信发送请求过多，请稍后重试"));
    }
    let entry = table.entry(key).or_insert(Entry { retry_at: 0, window_until: now + 900_000, attempts: 0 });
    if entry.retry_at > now { return Ok((false, entry.retry_at)); }
    if entry.window_until <= now { entry.window_until = now + 900_000; entry.attempts = 0; }
    if entry.attempts >= 5 { return Ok((false, entry.window_until)); }
    entry.attempts += 1;
    entry.retry_at = now + cooldown as i64 * 1000;
    Ok((true, entry.retry_at))
}

pub(super) fn challenge_ready(provider: &str, country: &str, phone: &str, reserved_until: i64) -> Result<i64, GatewayError> {
    let now = logging::now_ms();
    let mut table = table().lock().map_err(|_| GatewayError::with_status(503, "短信发送限制暂不可用"))?;
    let Some(entry) = table.get_mut(&key(provider, country, phone)) else { return Ok(reserved_until); };
    // 只释放本次尚未发码的预约；挑战仍计入尝试次数，不能清掉其他请求追加的冷却。
    if entry.retry_at == reserved_until { entry.retry_at = now; }
    Ok(if entry.attempts >= 5 && entry.window_until > now { entry.retry_at.max(entry.window_until) }
        else { entry.retry_at })
}

pub(super) fn extend(provider: &str, country: &str, phone: &str, retry_at: i64) {
    if let Ok(mut table) = table().lock() {
        if let Some(entry) = table.get_mut(&key(provider, country, phone)) {
            entry.retry_at = entry.retry_at.max(retry_at);
        }
    }
}

pub(super) fn reset_retry(provider: &str, country: &str, phone: &str) {
    let now = crate::server::logging::now_ms();
    if let Ok(mut table) = table().lock() {
        if let Some(entry) = table.get_mut(&key(provider, country, phone)) {
            entry.retry_at = now;
        }
    }
}
