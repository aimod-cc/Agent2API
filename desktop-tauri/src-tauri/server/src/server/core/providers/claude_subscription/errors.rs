//! Magpie 的 claude_auth / rate_limit_event 判据；诊断不回显原始 CLI 输出。
use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::errors::GatewayError;
use crate::server::logging;
use serde_json::Value;

pub(super) fn local(code: &str, message: &str) -> GatewayError {
    GatewayError::with_status(503, message).with_code(code)
}

pub(super) fn protocol(message: &str) -> GatewayError {
    GatewayError::with_status(502, message).with_code("claude_cli_protocol")
}

pub(super) fn session(message: &str) -> GatewayError {
    GatewayError::with_status(409, message).with_code("claude_session_lost")
}

pub(super) fn classify(status: u16, message: &str, reset: Option<i64>) -> UpstreamErrorClass {
    match status {
        429 => UpstreamErrorClass::QuotaLimited {
            reset_at: reset.filter(|at| *at > logging::now_ms()),
            message: message.into(),
            upstream_code: None,
            status,
        },
        401 => UpstreamErrorClass::TokenExpired { message: message.into() },
        _ => UpstreamErrorClass::Fatal { message: message.into(), status, upstream_code: None },
    }
}

pub(super) fn upstream(envelope: &Value) -> GatewayError {
    let text = envelope
        .get("result")
        .and_then(Value::as_str)
        .or_else(|| envelope.pointer("/error/message").and_then(Value::as_str))
        .or_else(|| envelope.get("error").and_then(Value::as_str))
        .unwrap_or("");
    let all = format!("{} {}", text, envelope.get("errors").unwrap_or(&Value::Null)).to_ascii_lowercase();
    let kind = envelope.get("error").and_then(Value::as_str).unwrap_or("");
    let reported = envelope.get("api_error_status").and_then(Value::as_u64).unwrap_or(0) as u16;
    let auth = [
        "oauth session expired and could not be refreshed",
        "oauth token revoked",
        "oauth access token has been revoked",
        "not logged in",
        "not signed in",
        "authentication_failed",
        "invalid authentication",
        "authentication required",
    ]
    .iter()
    .any(|part| all.contains(part));
    let quota = kind == "rate_limit"
        || envelope.pointer("/rate_limit_info/status").and_then(Value::as_str) == Some("rejected")
        || ["hit your limit", "usage limit", "rate limit", "rate_limit_error"].iter().any(|part| all.contains(part));
    let model = ["model not found", "model_not_found", "not have access to model", "not available for your"]
        .iter()
        .any(|part| all.contains(part));
    let (status, code, message) = if auth || reported == 401 {
        (401, "claude_auth_required", "Claude Code 登录已失效，请在官方 CLI 中重新登录后关联账号")
    } else if quota || reported == 429 {
        (429, "claude_quota_limited", "Claude Code 报告订阅额度或速率受限，请等待恢复或使用其他已授权账号")
    } else if model {
        (400, "account_model_unavailable", "此 Claude 账号不能使用所选模型，请刷新该账号的模型清单")
    } else if reported == 403 {
        (403, "claude_access_denied", "Claude Code 拒绝此次请求，请在官方客户端检查账号权限")
    } else {
        (
            if (400..600).contains(&reported) { reported } else { 502 },
            "claude_upstream_error",
            "Claude Code 未完成请求；原始诊断可能含本机路径或凭证，未回显到客户端",
        )
    };
    let reset = envelope.pointer("/rate_limit_info/resetsAt").and_then(timestamp);
    let mut error = GatewayError::with_status(i32::from(status), message).with_code(code);
    error.conversation_class = Some(classify(status, message, reset));
    if let Some(at) = reset.filter(|at| *at > logging::now_ms()) {
        error.retry_after_secs = Some(((at - logging::now_ms()).max(0) as u64).div_ceil(1000));
    }
    error
}

pub(super) fn timestamp(value: &Value) -> Option<i64> {
    let n = value.as_i64().or_else(|| value.as_str()?.parse().ok())?;
    if n <= 0 {
        return None;
    }
    let ms = if n < 100_000_000_000 { n.checked_mul(1000)? } else { n };
    chrono::DateTime::from_timestamp_millis(ms).map(|_| ms)
}
