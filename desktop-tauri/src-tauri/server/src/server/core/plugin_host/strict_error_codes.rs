//! 严格错误边界的有限代码集合；字符串形状/长度不足以证明它不是 token。
//! 分类只由真实 SDK classify RPC 决定，这里仅决定哪些机器码可以公开。

pub(super) fn qwen_number(value: i64) -> bool {
    matches!(value, 105 | 406 | 110 | 112 | 113 | 114 | 115 | 116 | 117 | 118 | 119 | 122
        | 10605 | 416 | 430 | 80411 | 80412 | 100400 | 100401 | 100403 | 47902 | 48716 | 10408 | 10500 | 500)
}

pub(super) fn credential_scope(value: &str) -> bool {
    matches!(value, "account" | "provider" | "local" | "source" | "session" | "subscription" | "oauth"
        | "api" | "credential" | "refresh" | "token" | "model" | "request" | "usage" | "none" | "unknown")
}

pub(super) fn known(provider: &str, code: &str) -> bool {
    let qwen = crate::server::core::providers::availability::is_qwenwork_family(provider);
    if qwen && code.parse::<i64>().ok().is_some_and(qwen_number) { return true; }
    if matches!(code, "400" | "401" | "402" | "403" | "404" | "408" | "409" | "413" | "429" | "499" | "500" | "502" | "503" | "504" | "556") { return true; }
    if matches!(code,
        "runtime_upstream_error" | "runtime_invalid_error_metadata" | "runtime_protocol_error"
        | "runtime_sdk_quota" | "runtime_sdk_token" | "runtime_sdk_content"
        | "runtime_stream_interrupted" | "runtime_classification_unavailable" | "runtime_transport_unavailable"
        | "builtin_runtime_error" | "builtin_upstream_error" | "builtin_invalid_error_code"
        | "bad_params" | "not_found" | "unsupported" | "no_method" | "internal" | "busy"
        | "aborted" | "abort_err" | "ABORT_ERR" | "cancelled" | "canceled" | "timeout"
        | "network_timeout" | "upstream_timeout" | "transport_closed" | "protocol_error"
        | "local_precondition" | "invalid_account" | "invalid_models" | "proxy_failed"
        | "invalid_local_reference" | "network_cleanup_unconfirmed" | "credential_cleanup_unconfirmed"
        | "local_cleanup_unconfirmed" | "login_cleanup_unconfirmed" | "cleanup_failed"
        | "login_cancelled" | "login_timeout" | "upstream_queue" | "requestshape"
        | "upstream_failed" | "request_failed" | "upstream_error" | "api_error"
        | "upstream_request_failed" | "upstream_stream_error" | "upstream_error_stream" | "upstream_refused"
        | "auth_changed" | "auth_conflict" | "identity_changed" | "local_generation_changed"
        | "permission_denied" | "permission_error" | "access_denied" | "invalid_request"
        | "invalid_request_error" | "invalid_argument" | "unavailable" | "overloaded_error" | "validation_required"
        | "content_filter" | "content_blocked" | "content_policy_violation" | "bio_policy" | "cyber_policy"
        | "refusal" | "safety" | "prohibited_content" | "blocklist" | "spii" | "image_safety" | "invalid_prompt"
        | "unauthenticated" | "token_expired" | "authentication_error" | "invalid_token" | "expired_token"
        | "invalid_grant" | "refresh_token_reused" | "sign_in_unrenewed" | "kimi_work_login_expired"
        | "resource_exhausted" | "quota_exceeded" | "insufficient_quota" | "usage_limit_reached"
        | "rate_limit_error" | "rate_limit_exceeded" | "throttlingexception"
        | "cursor_error" | "devin_error" | "qoder_error" | "copilot_request_error"
        | "invalid_client" | "invalid_scope" | "authorization_pending" | "slow_down"
        | "route_not_supported" | "model_not_available_for_account" | "model_not_available" | "model_policy_required") {
        return true;
    }
    match provider {
        "factory" => code == "content_policy_rejected",
        "grok" => matches!(code, "unsupported_external_web_access" | "grok_local_file_unavailable"
            | "grok_local_identity_changed" | "grok_local_signed_out" | "grok_local_token_rejected"
            | "grok_local_token_expired" | "grok_local_refresh_unsupported" | "grok_reference_changed"),
        "claude-subscription" => matches!(code, "not_implemented" | "local_protocol" | "cli_protocol_error"
            | "claude_runtime_error" | "run_expired" | "run_busy" | "run_pool_full" | "run_evicted"),
        "codex-subscription" | "chatgpt-subscription" => matches!(code,
            "invalid_refresh_token" | "refresh_token_expired" | "refresh_token_invalidated"
            | "refresh_unavailable" | "refresh_already_consumed" | "desktop_refresh_unavailable"
            | "desktop_token_expired" | "credit_balance_exhausted" | "organization_spend_limit_exceeded"
            | "project_spend_limit_exceeded" | "organization_usage_limit_exceeded")
            || provider == "chatgpt-subscription" && matches!(code,
                "subscription_sharing_usage_limit_exceeded" | "subscription_sharing_invalid_user"
                | "subscription_sharing_user_not_eligible" | "subscription_sharing_usage_unavailable"
                | "subscription_sharing_unsupported_capability" | "subscription_sharing_route_not_supported"
                | "subscription_sharing_user_unavailable" | "chatpass_v2_scope_not_authorized"
                | "chatpass_v2_invalid_authorization_context"),
        "copilot-subscription" => matches!(code, "authentication_required" | "bad_refresh_token"
            | "github_reauthentication_required" | "github_refresh_expired" | "unsupported_copilot_path"
            | "upstream_http_error" | "credential_operation_failed" | "upstream_malformed"),
        "gemini-subscription" => matches!(code, "RESOURCE_EXHAUSTED" | "UNAUTHENTICATED" | "PERMISSION_DENIED"
            | "INVALID_ARGUMENT" | "UNAVAILABLE" | "NOT_FOUND" | "credential_operation_failed" | "upstream_malformed"),
        _ => false,
    }
}
