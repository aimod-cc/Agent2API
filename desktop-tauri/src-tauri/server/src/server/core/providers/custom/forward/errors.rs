//! 自定义出口的有限错误证据；不把整份上游正文放入账号或重试记录。

use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::core::upstream::request::UpstreamErrorDetail;
use crate::server::errors::{GatewayError, PluginErrorMetadata};
use crate::server::logging;
use serde_json::Value;

pub(super) fn from_detail(status: u16, detail: &UpstreamErrorDetail, headers: &axum::http::HeaderMap) -> GatewayError {
    let body = detail.to_value();
    let code = machine_code(&body);
    let reset = reset_at(&body, headers).or_else(|| {
        let at = crate::server::errors::parse_quota_reset_at(&detail.message);
        (at > logging::now_ms()).then_some(at)
    });
    let mut error = GatewayError::with_status(i32::from(status), if matches!(status, 401 | 403) {
        format!("上游凭证被拒绝（HTTP {status}）: {}", detail.message)
    } else { format!("上游返回 {status}: {}", detail.message) }).with_optional_code(detail.code);
    error.code = code.clone();
    let scope = [&body, &body["error"]].into_iter().find_map(|value| {
        value.get("credential_scope").or_else(|| value.get("credentialScope")).and_then(Value::as_str)
            .filter(|value| matches!(*value, "model" | "account" | "provider" | "credential" | "subscription" | "oauth" | "token"))
            .map(str::to_string)
    });
    error.plugin_metadata = Some(PluginErrorMetadata { reset_at: reset, credential_scope: scope, provider_code: code });
    if let Some(at) = reset { error.retry_after_secs = Some(at.saturating_sub(logging::now_ms()).max(0) as u64 / 1000 + 1); }
    error.conversation_class = Some(if status == 429 {
        UpstreamErrorClass::QuotaLimited { status, message: error.message.clone(), upstream_code: detail.code, reset_at: reset }
    } else if status == 401 {
        UpstreamErrorClass::TokenExpired { message: error.message.clone() }
    } else if matches!(error.code.as_deref(), Some("content_policy_violation" | "content_filter" | "content_blocked" | "content_policy_rejected")) {
        UpstreamErrorClass::ContentBlocked { status, message: error.message.clone(), upstream_code: detail.code }
    } else { UpstreamErrorClass::Fatal { status, message: error.message.clone(), upstream_code: detail.code } });
    error
}

fn machine_code(body: &Value) -> Option<String> {
    for key in ["code", "upstreamCode", "upstream_code", "type"] {
        for value in [&body["error"], body] {
            if let Some(code) = value.get(key).and_then(Value::as_str).filter(|value| !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)) {
                return Some(code.to_string());
            }
            if let Some(code) = value.get(key).and_then(Value::as_i64) { return Some(code.to_string()); }
        }
    }
    None
}

fn reset_at(body: &Value, headers: &axum::http::HeaderMap) -> Option<i64> {
    let now = logging::now_ms();
    let mut at = crate::server::core::upstream::retry_after::reset_at(headers);
    for value in [body, &body["error"]] {
        for key in ["resetAt", "reset_at", "resets_at"] {
            let time = value.get(key).and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok())
                .or_else(|| chrono::DateTime::parse_from_rfc3339(value.as_str()?).ok().map(|date| date.timestamp_millis())))
                .and_then(|time| if (1..100_000_000_000).contains(&time) { time.checked_mul(1000) } else { Some(time) });
            if let Some(time) = time.filter(|time| *time > now && *time <= (1_i64 << 53) - 1 && chrono::DateTime::from_timestamp_millis(*time).is_some()) {
                at = Some(at.unwrap_or(time).max(time));
            }
        }
    }
    at
}
