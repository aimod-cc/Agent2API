use serde_json::Value;

use crate::server::config::error_policy::{CooldownScope, ErrorKind, PluginErrorClassification};
use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::errors::GatewayError;

pub(super) fn candidate_codes(body: &Value) -> Vec<String> {
    let mut codes = Vec::new();
    for key in ["providerCode", "provider_code", "code", "upstreamCode", "upstream_code", "type"] {
        for value in [body, &body["error"], &body["detail"]] {
            if let Some(text) = value.get(key).and_then(Value::as_str)
                .filter(|text| !text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control))
            {
                if !codes.iter().any(|existing| existing == text) {
                    codes.push(text.to_string());
                }
            }
            if let Some(number) = value.get(key).and_then(Value::as_i64) {
                let text = number.to_string();
                if !codes.iter().any(|existing| existing == &text) {
                    codes.push(text);
                }
            }
        }
    }
    codes
}

pub(super) fn machine_code(body: &Value) -> String {
    candidate_codes(body).into_iter().next().unwrap_or_default()
}

pub(super) fn match_classification<'a>(
    classifications: &'a [PluginErrorClassification],
    status: u16,
    codes: &[String],
) -> Option<&'a PluginErrorClassification> {
    let mut best: Option<(&PluginErrorClassification, (bool, bool))> = None;
    for item in classifications {
        let code_ok = item.codes.is_empty() || item.codes.iter().any(|c| codes.iter().any(|cand| cand == c));
        let status_ok = item.statuses.is_empty() || item.statuses.contains(&status);
        if !code_ok || !status_ok { continue; }
        let score = (!item.codes.is_empty(), !item.statuses.is_empty());
        if best.is_none_or(|(_, prev)| score > prev) {
            best = Some((item, score));
        }
    }
    best.map(|(item, _)| item)
}

pub(super) fn kind(
    provider: &str,
    class: &UpstreamErrorClass,
    status: u16,
    code: &str,
    codes: &[String],
    classifications: &[PluginErrorClassification],
) -> ErrorKind {
    if let Some(declared) = match_classification(classifications, status, codes) {
        return declared.kind;
    }
    if matches!(class, UpstreamErrorClass::ContentBlocked { .. }) { return ErrorKind::Content; }
    if matches!(class, UpstreamErrorClass::TokenExpired { .. }) { return ErrorKind::Auth; }
    let upstream = match class {
        UpstreamErrorClass::QuotaLimited { upstream_code, .. } | UpstreamErrorClass::Fatal { upstream_code, .. } => *upstream_code,
        _ => None,
    }.or_else(|| codes.iter().find_map(|cand| cand.parse::<i64>().ok()));
    if classifications.is_empty() && matches!(provider, "workbuddy" | "workbuddy-intl") {
        if matches!(upstream, Some(14003 | 6001 | 6002 | 6005 | 6006)) { return ErrorKind::Transient; }
        if matches!(upstream, Some(14001 | 14012 | 14013 | 14014 | 14018 | 14019)) { return ErrorKind::Quota; }
        if matches!(upstream, Some(6003 | 6004)) { return ErrorKind::RateLimit; }
    }
    if classifications.is_empty() && matches!(provider, "zcode" | "zcode-intl") {
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

/// 识别「429 外壳嵌套 402」或正文明确指出的余额/积分不足信号。
/// 先排除否定措辞（「不是余额不足」等）与限流措辞，防止将权限/限流错误误归为余额不足。
pub(super) fn balance_message_signal(body: &Value, class: &UpstreamErrorClass) -> bool {
    let mut parts: Vec<&str> = Vec::new();
    for candidate in [
        body.get("message").and_then(Value::as_str),
        body.pointer("/error/message").and_then(Value::as_str),
        body.pointer("/detail/message").and_then(Value::as_str),
    ] {
        if let Some(text) = candidate.map(str::trim).filter(|text| !text.is_empty()) {
            parts.push(text);
        }
    }
    let class_message = match class {
        UpstreamErrorClass::QuotaLimited { message, .. }
        | UpstreamErrorClass::ContentBlocked { message, .. }
        | UpstreamErrorClass::Fatal { message, .. }
        | UpstreamErrorClass::TokenExpired { message } => message.trim(),
    };
    if !class_message.is_empty() && !parts.contains(&class_message) {
        parts.push(class_message);
    }
    if parts.is_empty() {
        return false;
    }
    let combined: String = parts.join(" ").chars().take(4096).collect();
    let lower = combined.to_lowercase();
    if [
        "不是余额不足",
        "非余额不足",
        "并非余额不足",
        "不是积分不足",
        "不是积分余额不足",
        "不是账号额度不足",
        "不是额度不足",
        "not insufficient",
        "not a balance",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        return false;
    }
    if [
        "rate limit",
        "ratelimit",
        "too many requests",
        "too many",
        "限流",
        "过于频繁",
        "频率",
        "rpm",
        "tpm",
        "qps",
        "并发",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        return false;
    }
    [
        "上游返回 402",
        "上游返回 http 402",
        "http 402",
        "status 402",
        "status: 402",
        "\"status\":402",
        "payment required",
        "payment_required",
        "积分余额不足",
        "余额不足",
        "积分不足",
        "积分已用完",
        "积分耗尽",
        "没有可用积分",
        "可用积分为 0",
        "欠费",
        "余额已用完",
        "余额耗尽",
        "insufficient balance",
        "insufficient_balance",
        "insufficient credit",
        "insufficient_credit",
        "credit balance",
        "credits exhausted",
        "credit exhausted",
        "out of credit",
        "no credit",
        "billing hard limit",
    ]
    .iter()
    .any(|word| lower.contains(word))
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

pub(super) fn hard_stop(status: u16, code: &str, codes: &[String]) -> bool {
    let matches_stop_code = |item: &str| {
        item.ends_with("cleanup_unconfirmed")
            || item.starts_with("claude_session_")
            || matches!(
                item,
                "validation_required"
                    | "tabbit_submission_unconfirmed"
                    | "tabbit_account_changed"
                    | "tabbit_state_conflict"
                    | "claude_response_started"
                    | "account_origin_authorization_required"
                    | "plugin_busy"
                    | "gateway_busy"
                    | "account_busy"
                    | "plugin_response_started"
                    | "plugin_resource_limit"
                    | "plugin_protocol_error"
                    | "plugin_trap"
                    | "native_helper_not_authorized"
                    | "native_helper_unavailable"
                    | "request_replay_blocked"
                    | "web_tools_invalid_request"
                    | "gateway_capability_missing"
                    | "aborted"
                    | "cancelled"
                    | "upstream_submission_unconfirmed"
                    | "runtime_invalid_error_metadata"
                    | "runtime_protocol_error"
                    | "runtime_stream_interrupted"
                    | "runtime_classification_unavailable"
                    | "runtime_transport_unavailable"
                    | "network_cleanup_unconfirmed"
                    | "builtin_runtime_error"
                    | "runtime_sdk_content"
            )
    };
    status == 499 || (!code.is_empty() && matches_stop_code(code)) || codes.iter().any(|item| matches_stop_code(item))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::config::error_policy::{
        CooldownScope, DeclaredErrorPolicy, ErrorPolicySnapshot, PluginErrorClassification,
    };
    use crate::server::core::upstream::error_policy::decide_with_declared;
    use serde_json::json;

    #[test]
    fn nested_402_in_429_is_classified_as_account_balance() {
        let snapshot = ErrorPolicySnapshot::default();
        let class = UpstreamErrorClass::QuotaLimited {
            status: 429,
            message: "上游返回 429: 上游返回 402: 积分余额不足，请获取积分后再试".into(),
            upstream_code: None,
            reset_at: None,
        };
        let body = json!({
            "code": "loomy_quota",
            "message": "上游返回 429: 上游返回 402: 积分余额不足，请获取积分后再试"
        });
        let decision = decide_with_declared(&snapshot, &DeclaredErrorPolicy::default(), "loomy", "gpt-5", &class, 429, &body, false, false);
        assert_eq!(decision.kind, ErrorKind::Balance);
        assert_eq!(decision.scope, CooldownScope::Account);
        assert!(decision.rule.exponential);
        assert_eq!(decision.rule.fallback_cap_seconds, 21_600);
    }

    #[test]
    fn negated_and_rate_limit_messages_are_not_misclassified_as_balance() {
        let snapshot = ErrorPolicySnapshot::default();
        let declared = DeclaredErrorPolicy {
            classifications: vec![PluginErrorClassification {
                kind: ErrorKind::Unsupported,
                statuses: vec![],
                codes: vec!["1311".into()],
                scope: None,
                hard_stop: false,
            }],
            rules: vec![],
        };
        let zcode = UpstreamErrorClass::Fatal {
            status: 403,
            message: "ZCode 当前订阅未开放所选模型权限（1311），不是余额不足".into(),
            upstream_code: Some(1311),
        };
        let zcode_body = json!({ "code": 1311, "message": "ZCode 当前订阅未开放所选模型权限（1311），不是余额不足" });
        let zcode_decision = decide_with_declared(&snapshot, &declared, "zcode", "claude", &zcode, 403, &zcode_body, false, false);
        assert_eq!(zcode_decision.kind, ErrorKind::Unsupported);

        let rate = UpstreamErrorClass::QuotaLimited {
            status: 429,
            message: "AStudio 当前积分或额度不足，或请求过于频繁".into(),
            upstream_code: None,
            reset_at: None,
        };
        let rate_body = json!({ "message": "AStudio 当前积分或额度不足，或请求过于频繁" });
        let rate_decision = decide_with_declared(&snapshot, &DeclaredErrorPolicy::default(), "astudio", "m", &rate, 429, &rate_body, false, false);
        assert_eq!(rate_decision.kind, ErrorKind::RateLimit);
    }
}
