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

pub(super) fn scope(body: &Value) -> CooldownScope {
    match body.get("credentialScope").or_else(|| body.get("credential_scope")).or_else(|| body.get("scope")).and_then(Value::as_str) {
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

pub(super) fn hard_stop(status: u16, code: &str) -> bool {
    matches!(status, 403 | 409 | 428 | 499)
        || code.ends_with("cleanup_unconfirmed") || code.starts_with("claude_session_")
        || matches!(code, "permission_denied" | "access_denied" | "permission_error" | "validation_required"
            | "tabbit_submission_unconfirmed" | "tabbit_account_changed" | "tabbit_state_conflict"
            | "claude_response_started" | "account_origin_authorization_required"
            | "plugin_busy" | "gateway_busy" | "account_busy" | "plugin_response_started" | "plugin_resource_limit"
            | "plugin_protocol_error" | "plugin_trap" | "forbidden" | "native_helper_not_authorized" | "native_helper_unavailable"
            | "request_replay_blocked" | "web_tools_invalid_request" | "gateway_capability_missing" | "aborted" | "cancelled"
            | "upstream_submission_unconfirmed" | "runtime_invalid_error_metadata" | "runtime_protocol_error" | "runtime_stream_interrupted"
            | "runtime_classification_unavailable" | "runtime_transport_unavailable" | "network_cleanup_unconfirmed"
            | "runtime_upstream_error" | "builtin_runtime_error" | "runtime_sdk_content")
}
