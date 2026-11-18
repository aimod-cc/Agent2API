//! SIWC 错误码映射；原始响应仅经脱敏后随登录诊断返回。

use serde_json::Value;

use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::errors::GatewayError;

use super::USAGE_URL;

pub(super) fn code(body: &Value) -> String {
    let raw = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .or_else(|| body.pointer("/detail/code").and_then(Value::as_str))
        .or_else(|| body.get("code").and_then(Value::as_str))
        .or_else(|| body.pointer("/error/type").and_then(Value::as_str));
    if let Some(raw) = raw.filter(|raw| safe_code(raw)) {
        return raw.to_string();
    }
    // Magpie 也接收 detail 字符串内的 SIWC 业务码，只取固定词法，不透出整段错误。
    let detail = body.get("detail").and_then(Value::as_str).unwrap_or("");
    for prefix in ["subscription_sharing_", "chatpass_v2_"] {
        if let Some(start) = detail.find(prefix) {
            let value: String = detail[start..]
                .chars()
                .take_while(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || *ch == '_')
                .take(128)
                .collect();
            if safe_code(&value) {
                return value;
            }
        }
    }
    String::new()
}

fn safe_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_')
}

pub(super) fn requires_login(code: &str) -> bool {
    matches!(
        code,
        "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused"
            | "invalid_client"
            | "subscription_sharing_invalid_user"
    )
}

pub(super) fn temporary(code: &str) -> bool {
    matches!(
        code,
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable"
    )
}

pub(super) fn message(status: u16, code: &str) -> String {
    match code {
        "subscription_sharing_usage_limit_exceeded" => {
            format!("ChatGPT 套餐用量已达上限，请在 {USAGE_URL} 查看")
        }
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
            "OpenAI 暂时无法确认 ChatGPT 套餐用量，请稍后重试".to_string()
        }
        "subscription_sharing_user_not_eligible" => {
            "当前 ChatGPT 账号或套餐不具备此次授权的使用资格".to_string()
        }
        "subscription_sharing_unsupported_capability" => {
            "ChatGPT 套餐通道不支持本次请求中的部分能力".to_string()
        }
        "subscription_sharing_route_not_supported" => {
            "ChatGPT 套餐通道不支持这个 API 路径".to_string()
        }
        "unsupported_country_region_territory" | "unknown_country" => {
            "OpenAI 不支持当前请求来源的国家或地区，请核对软件的出网设置；浏览器授权与软件请求可能使用不同线路".to_string()
        }
        "chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context" => {
            "OpenAI 未接受本应用的套餐授权，请核对账号或工作区的使用权限".to_string()
        }
        "invalid_client" => "OpenAI 未接受本应用的注册信息，请重新发起登录".to_string(),
        "access_denied" => "ChatGPT 授权被取消或未获同意，请重新发起登录".to_string(),
        code if requires_login(code) => "ChatGPT 授权已失效，请重新登录".to_string(),
        _ if status == 401 => "ChatGPT 登录凭证未被接受，请刷新凭证或重新登录".to_string(),
        _ if status == 429 => "ChatGPT 请求暂时受限，请稍后重试".to_string(),
        _ if status >= 500 => "OpenAI 服务暂时不可用，请稍后重试".to_string(),
        _ => format!("ChatGPT 请求未被接受（HTTP {status}）"),
    }
}

pub(super) fn upstream(status: u16, body: &Value) -> GatewayError {
    let code = code(body);
    let mut error = GatewayError::with_status(i32::from(status), message(status, &code));
    if safe_code(&code) {
        error.code = Some(code);
    }
    error
}

pub(super) fn classify(status: u16, body: &Value) -> UpstreamErrorClass {
    let code = code(body);
    let status = if (200..300).contains(&status) {
        502
    } else {
        status
    };
    let message = message(status, &code);
    if temporary(&code) {
        return UpstreamErrorClass::Fatal {
            status: 503,
            message,
            upstream_code: None,
        };
    }
    // 套餐准入或能力拒绝不是临时限额，不能把所有 403 / 429 一律罚账号。
    if matches!(
        code.as_str(),
        "subscription_sharing_user_not_eligible"
            | "subscription_sharing_unsupported_capability"
            | "subscription_sharing_route_not_supported"
    ) {
        return UpstreamErrorClass::Fatal {
            status,
            message,
            upstream_code: None,
        };
    }
    if status == 401 || code == "subscription_sharing_invalid_user" {
        return UpstreamErrorClass::TokenExpired { message };
    }
    if code == "subscription_sharing_usage_limit_exceeded" || status == 429 {
        return UpstreamErrorClass::QuotaLimited {
            reset_at: None,
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
