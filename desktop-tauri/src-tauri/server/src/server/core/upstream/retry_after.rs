//! 标准限流等待时间在响应被消费前读取，同时用于冷却与客户端提示。

use axum::http::HeaderMap;

use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::errors::GatewayError;
use crate::server::logging;

pub(super) fn reset_at(headers: &HeaderMap) -> Option<i64> {
    let now = logging::now_ms();
    let value = |name: &str| headers.get(name)?.to_str().ok().map(str::trim);
    if let Some(milliseconds) = value("retry-after-ms").and_then(|text| text.parse::<f64>().ok()) {
        if let Some(reset) = after(now, milliseconds) {
            return Some(reset);
        }
    }
    let text = value("retry-after")?;
    if let Ok(seconds) = text.parse::<f64>() {
        return after(now, seconds * 1000.0);
    }
    chrono::DateTime::parse_from_rfc2822(text)
        .ok()
        .map(|date| date.timestamp_millis())
        .filter(|reset| *reset > now)
}

fn after(now: i64, milliseconds: f64) -> Option<i64> {
    if !milliseconds.is_finite() || milliseconds <= 0.0 || milliseconds >= i64::MAX as f64 {
        return None;
    }
    now.checked_add(milliseconds.ceil() as i64)
}

pub(super) fn apply(class: &mut UpstreamErrorClass, header_reset: Option<i64>) {
    if let (
        UpstreamErrorClass::QuotaLimited {
            status: 429,
            reset_at,
            ..
        },
        Some(reset),
    ) = (class, header_reset)
    {
        *reset_at = Some(reset_at.unwrap_or(reset).max(reset));
    }
}

pub(super) fn with_reset(error: GatewayError, reset_at: Option<i64>) -> GatewayError {
    if error.status_code != 429 {
        return error;
    }
    let remaining = reset_at.unwrap_or(0).saturating_sub(logging::now_ms());
    if remaining <= 0 {
        return error;
    }
    let seconds = (remaining as u64).saturating_add(999) / 1000;
    error.with_retry_after_seconds(seconds)
}
