use serde_json::Value;

use crate::server::config::error_policy::{CooldownScope, ErrorKind};
use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::errors::GatewayError;

pub(super) fn machine_code(body: &Value) -> String {
    for key in ["providerCode", "provider_code", "code", "upstreamCode", "upstream_code", "type"] {
        for value in [body, &body["error"], &body["detail"]] {
            if let Some(text) = value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control)) { return text.to_string(); }
            if let Some(number) = value.get(key).and_then(Value::as_i64) { return number.to_string(); }
        }
    }
    String::new()
}

pub(super) fn kind(provider: &str, class: &UpstreamErrorClass, status: u16, code: &str) -> ErrorKind {
    if matches!(class, UpstreamErrorClass::ContentBlocked { .. }) { return ErrorKind::Content; }
    if matches!(class, UpstreamErrorClass::TokenExpired { .. }) { return ErrorKind::Auth; }
    let upstream = match class {
        UpstreamErrorClass::QuotaLimited { upstream_code, .. } | UpstreamErrorClass::Fatal { upstream_code, .. } => *upstream_code,
        _ => None,
    }.or_else(|| code.parse::<i64>().ok());
    if matches!(provider, "workbuddy" | "workbuddy-intl") {
        if matches!(upstream, Some(14003 | 6001 | 6002 | 6005 | 6006)) { return ErrorKind::Transient; }
        if matches!(upstream, Some(14001 | 14012 | 14013 | 14014 | 14018 | 14019)) { return ErrorKind::Quota; }
        if matches!(upstream, Some(6003 | 6004)) { return ErrorKind::RateLimit; }
    }
    if matches!(provider, "zcode" | "zcode-intl") {
        if matches!(upstream, Some(1005 | 1308 | 1310)) { return ErrorKind::Quota; }
        if upstream == Some(1113) { return ErrorKind::Balance; }
    }
    if status == 402 || matches!(code, "insufficient_balance" | "balance_insufficient" | "billing_hard_limit_reached"
        | "payment_required" | "credit_balance_exhausted" | "credit_balance_too_low") { return ErrorKind::Balance; }
    if matches!(code, "quota_exhausted" | "quota_exceeded" | "insufficient_quota" | "usage_limit_reached" | "monthly_request_count"
        | "quota_limit_reached" | "organization_spend_limit_exceeded" | "project_spend_limit_exceeded"
        | "organization_usage_limit_exceeded" | "subscription_sharing_usage_limit_exceeded") { return ErrorKind::Quota; }
    if matches!(code, "account_model_unavailable" | "account_model_permissions_unknown" | "model_not_available_for_account"
        | "model_not_available" | "model_not_found" | "unsupported" | "no_method" | "not_implemented" | "route_not_supported"
        | "subscription_sharing_route_not_supported" | "subscription_sharing_unsupported_capability") || status == 501 { return ErrorKind::Unsupported; }
    if matches!(code, "content_policy_violation" | "content_filter" | "content_blocked" | "content_policy_rejected"
        | "workbuddy_content_blocked" | "prohibited_content" | "refusal" | "safety" | "runtime_sdk_content") { return ErrorKind::Content; }
    if matches!(status, 400 | 413 | 422) || matches!(code, "context_length_exceeded" | "payload_too_large" | "invalid_request_error"
        | "invalid_request" | "context_window_exceeded" | "model_context_window_exceeded") { return ErrorKind::BadRequest; }
    if status == 429 || matches!(class, UpstreamErrorClass::QuotaLimited { .. }) { return ErrorKind::RateLimit; }
    if matches!(status, 408 | 500 | 502 | 503 | 504) { return ErrorKind::Transient; }
    if status == 401 { return ErrorKind::Auth; }
    ErrorKind::Unknown
}

/// 「这份 body 本身就发不出去」的本地业务码 —— 换账号无用，得换一家。
/// 这些码由本机在**发送前**生成（网页工具的字符预算、视觉兼容的前置校验），
/// 它们的结论是「提供商 × 模型能力吃不下这份请求」，与具体账号无关。
/// 判据按码前缀匹配：新增同类能力检查时只要沿用同一前缀，就自动获得
/// 「跳过这一家 + 落模型冷却」的处置。
pub(super) fn provider_local(code: &str) -> bool {
    ["web_tools_", "vision_", "native_payload_"].iter().any(|prefix| code.starts_with(prefix))
}

/// 429/402 里包着的「账号资源已经用完」语义，按**文案**还原成真正的错误类型。
///
/// 聚合类上游常把余额/套餐问题套在 429 里返回，例如：
/// `上游返回 429: 上游返回 402: 积分余额不足，请获取积分后再试`、
/// `ZCode 套餐池当前没有有效套餐（1005）`。只按状态码归成「请求限流」会让
/// 处置方向错掉 —— 限流是「等一会儿还能用」，余额/套餐是「这个账号暂时
/// 不可能答」，后者必须按**账号级**冷却并把该账号的其余候选一起排掉，
/// 否则路由组会拿同一个账号换着模型再撞一次（429 连着两条就是这么来的）。
///
/// 返回 `None` 表示文案看不出资源问题（例如真正的频率限制），保持原分类。
pub(super) fn resource_kind(message: &str) -> Option<ErrorKind> {
    let text = message.to_lowercase();
    // 明确的频率限制优先排除：文案里常同时出现「额度」「quota」这类词。
    const RATE: [&str; 5] = ["rate limit", "rate_limit", "too many requests", "限流", "频率限制"];
    if RATE.iter().any(|word| text.contains(word)) { return None; }
    const BALANCE: [&str; 8] = ["积分余额不足", "积分不足", "余额不足", "欠费", "insufficient balance", "insufficient credit",
        "credit balance", "payment required"];
    if BALANCE.iter().any(|word| text.contains(word)) { return Some(ErrorKind::Balance); }
    const QUOTA: [&str; 9] = ["套餐池", "没有有效套餐", "套餐到期", "套餐已到期", "额度已用尽", "quota", "usage limit",
        "配额不足", "quota exceeded"];
    if QUOTA.iter().any(|word| text.contains(word)) { return Some(ErrorKind::Quota); }
    None
}

pub(super) fn scope(body: &Value) -> CooldownScope {    match body.get("credentialScope").or_else(|| body.get("credential_scope")).or_else(|| body.get("scope")).and_then(Value::as_str) {
        Some("provider") => CooldownScope::Provider,
        Some("account" | "credential" | "subscription" | "oauth" | "token") => CooldownScope::Account,
        _ => CooldownScope::Model,
    }
}

pub(super) fn reset(body: &Value, now: i64) -> Option<i64> {
    let mut latest = None;
    for value in [body, &body["error"], &body["detail"]] {
        for key in ["resetAt", "reset_at", "resetsAt", "resets_at"] {
            let at = value.get(key).and_then(|raw| raw.as_i64().or_else(|| raw.as_str()?.parse().ok())
                .or_else(|| chrono::DateTime::parse_from_rfc3339(raw.as_str()?).ok().map(|time| time.timestamp_millis()))).and_then(|at| {
                if (1..100_000_000_000).contains(&at) { at.checked_mul(1000) } else { Some(at) }
            });
            if let Some(at) = at.filter(|at| valid_reset(*at, now)) { latest = Some(latest.unwrap_or(at).max(at)); }
        }
        for key in ["retryAfterSecs", "retry_after_secs", "resets_in_seconds", "retryAfter"] {
            if let Some(at) = value.get(key).and_then(Value::as_u64).and_then(|seconds| i64::try_from(seconds).ok())
                .and_then(|seconds| seconds.checked_mul(1000)).and_then(|ms| now.checked_add(ms)).filter(|at| valid_reset(*at, now)) {
                latest = Some(latest.unwrap_or(at).max(at));
            }
        }
    }
    latest
}

fn valid_reset(at: i64, now: i64) -> bool {
    at > now && at <= (1_i64 << 53) - 1 && chrono::DateTime::from_timestamp_millis(at).is_some()
}

pub(in crate::server::core::upstream) fn detail(error: &GatewayError) -> Value {
    let mut body = serde_json::json!({ "code": error.code, "message": error.message, "upstreamCode": error.upstream_code });
    if let Some(metadata) = &error.plugin_metadata {
        if let Some(reset) = metadata.reset_at { body["resetAt"] = reset.into(); }
        if let Some(scope) = &metadata.credential_scope { body["credentialScope"] = scope.clone().into(); }
        if let Some(code) = &metadata.provider_code { body["providerCode"] = code.clone().into(); }
    }
    if let Some(seconds) = error.retry_after_secs { body["retryAfterSecs"] = seconds.into(); }
    body
}

pub(in crate::server::core::upstream) fn gateway_class(error: &GatewayError) -> UpstreamErrorClass {
    error.conversation_class.clone().unwrap_or_else(|| {
        if error.is_quota_limit() {
            UpstreamErrorClass::QuotaLimited { status: error.status_code as u16, message: error.message.clone(),
                upstream_code: error.upstream_code, reset_at: reset(&detail(error), crate::server::logging::now_ms()) }
        } else { UpstreamErrorClass::Fatal { status: error.status_code as u16, message: error.message.clone(), upstream_code: error.upstream_code } }
    })
}

/// 「不要再换号、直接把错误交回下游」的硬终止判据。
///
/// ── 为什么 403 / 权限类不再在这里 ─────────────────────────
/// 上游 403（账号被风控、凭证对该模型无权、订阅级禁止）是**账号级**结论：
/// 换下一个账号或另一家仍可能成功，属于「应该顺延」而不是「整条请求没救」。
/// #644 之后它们走默认的换号规则（并落账号级冷却/熔断）。这里只保留
/// 「再发一次可能造成重复执行/重复计费/状态错乱」与「用户已取消」两类。
pub(super) fn hard_stop(status: u16, code: &str) -> bool {
    matches!(status, 409 | 428 | 499)
        || code.ends_with("cleanup_unconfirmed") || code.starts_with("claude_session_")
        || matches!(code, "validation_required"
            | "tabbit_submission_unconfirmed" | "tabbit_account_changed" | "tabbit_state_conflict"
            | "claude_response_started" | "account_origin_authorization_required"
            | "plugin_busy" | "gateway_busy" | "account_busy" | "plugin_response_started" | "plugin_resource_limit"
            | "plugin_protocol_error" | "plugin_trap" | "native_helper_not_authorized" | "native_helper_unavailable"
            | "request_replay_blocked" | "gateway_capability_missing" | "aborted" | "cancelled"
            | "upstream_submission_unconfirmed" | "runtime_invalid_error_metadata" | "runtime_protocol_error" | "runtime_stream_interrupted"
            | "runtime_classification_unavailable" | "runtime_transport_unavailable" | "network_cleanup_unconfirmed"
            | "runtime_upstream_error" | "builtin_runtime_error" | "runtime_sdk_content")
}
