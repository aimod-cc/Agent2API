//! 参考 Magpie gateway/codex_backend.go/codexAccountOut 与 routing.go。
//! 401 与配额失败分开，权限/模型不存在不伪装成额度不足；窗口重置值只用上游读数。

use serde_json::Value;

use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{http, usage};

pub(super) fn classify(status: u16, body: &Value) -> UpstreamErrorClass {
    let status = if (200..300).contains(&status) {
        502
    } else {
        status
    };
    let detail = body
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| body.pointer("/error/message").and_then(Value::as_str))
        .or_else(|| body.pointer("/detail/message").and_then(Value::as_str))
        .or_else(|| body.get("detail").and_then(Value::as_str))
        .unwrap_or("上游拒绝了本次请求");
    let detail: String = detail
        .chars()
        .filter(|ch| !ch.is_control() || matches!(*ch, '\n' | '\t'))
        .take(2000)
        .collect();
    let message = format!("Codex 返回 {status}：{detail}");
    let code = http::code(body).unwrap_or_default().to_ascii_lowercase();
    if status == 401 {
        return UpstreamErrorClass::TokenExpired { message };
    }
    let quota = status == 429
        || status == 402
        || status == 403 && code == "usage_not_included"
        || matches!(
            code.as_str(),
            "usage_limit_reached"
                | "rate_limit_exceeded"
                | "rate_limit_reached"
                | "insufficient_quota"
        );
    if quota {
        let reset_at = usage::reset(body)
            .or_else(|| body.get("error").and_then(usage::reset))
            .filter(|at| *at > logging::now_ms());
        return UpstreamErrorClass::QuotaLimited {
            reset_at,
            message,
            upstream_code: None,
            status,
        };
    }
    UpstreamErrorClass::Fatal {
        status,
        message,
        upstream_code: None,
    }
}

pub(super) fn classify_conversation(error: &GatewayError) -> UpstreamErrorClass {
    let body = serde_json::json!({ "message": error.message, "code": error.code,
        "retry_after_seconds": error.retry_after_secs });
    classify(u16::try_from(error.status_code).unwrap_or(500), &body)
}
