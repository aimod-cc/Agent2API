//! 插件控制面错误的 HTTP 状态与业务码兼容解码。

pub(super) mod workbuddy_stream;

use agent2api_provider_protocol as proto;
use serde::Deserialize;

use crate::server::errors::GatewayError;

#[path = "strict_errors.rs"]
pub(super) mod strict;

const HTTP_CODE_PREFIX: &str = "provider_http:";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorMetadata {
    status: i32,
    #[serde(default)]
    code: Option<String>,
    #[serde(default, alias = "upstream_code")]
    upstream_code: Option<i64>,
    #[serde(default, alias = "reset_at")]
    reset_at: Option<i64>,
    #[serde(default, alias = "credential_scope")]
    credential_scope: Option<String>,
    #[serde(default, alias = "retry_after_secs", alias = "retryAfter")]
    retry_after_secs: Option<u64>,
}

pub(super) fn control_error(error: &proto::WireError) -> Option<GatewayError> {
    let payload = error.code.as_deref()?.strip_prefix(HTTP_CODE_PREFIX)?;
    let metadata: ErrorMetadata = serde_json::from_str(payload).ok()?;
    if !(400..600).contains(&metadata.status) {
        return None;
    }
    let mut failure = GatewayError::with_status(metadata.status, error.message.clone())
        .with_optional_code(metadata.upstream_code);
    failure.code = metadata.code;
    let now = crate::server::logging::now_ms();
    let reset_at = metadata.reset_at.filter(|reset| *reset >= 0 && *reset <= (1_i64 << 53) - 1
        && chrono::DateTime::from_timestamp_millis(*reset).is_some());
    let credential_scope = metadata.credential_scope.filter(|scope| credential_scope_allowed(scope));
    let seconds_until = |reset: i64| u64::try_from(reset.saturating_sub(now).max(0)).ok().map(|ms| ms.div_ceil(1000));
    failure.retry_after_secs = reset_at.and_then(seconds_until).or_else(|| metadata.retry_after_secs.filter(|seconds| {
        i64::try_from(*seconds).ok().and_then(|seconds| seconds.checked_mul(1000))
            .and_then(|ms| now.checked_add(ms)).is_some_and(|reset| reset <= (1_i64 << 53) - 1
                && chrono::DateTime::from_timestamp_millis(reset).is_some())
    }));
    if reset_at.is_some() || credential_scope.is_some() {
        failure.plugin_metadata = Some(crate::server::errors::PluginErrorMetadata { reset_at, credential_scope, provider_code: None });
    }
    Some(failure)
}

pub(super) fn credential_scope_allowed(scope: &str) -> bool {
    matches!(scope, "account" | "provider" | "local" | "source" | "session" | "subscription" | "oauth"
        | "api" | "credential" | "refresh" | "token" | "model" | "request" | "usage" | "none" | "unknown")
}

pub(super) fn operation_failure(method: &str, detail: &str) -> String {
    use proto::methods::gateway as methods;

    let operation = match method {
        methods::USAGE_QUERY => "余额查询",
        methods::TOKEN_ENSURE => "获取可用凭证",
        methods::TOKEN_REFRESH => "凭证续期",
        methods::TOKEN_EXPIRING => "检查凭证有效期",
        methods::MODELS_LIST => "获取模型列表",
        methods::MODELS_REFRESH => "刷新模型列表",
        methods::CHECKIN_CLAIM => "签到",
        methods::CLAIM => "福利领取",
        methods::CLAIM_STATUS => "查询福利状态",
        methods::ACCOUNT_NORMALIZE => "账号导入",
        methods::ACCOUNT_VIEW => "获取账号信息",
        methods::ACCOUNT_IDENTITY => "识别账号",
        methods::LOGIN_START => "发起登录",
        methods::LOGIN_POLL => "查询登录状态",
        methods::LOGIN_CALLBACK => "登录授权",
        methods::LOGIN_CANCEL => "取消登录",
        methods::CHAT => "构造对话请求",
        crate::server::core::providers::images::GENERATE_METHOD => "图片生成",
        crate::server::core::providers::images::EDIT_METHOD => "图片编辑",
        _ => "提供商操作",
    };
    format!("{operation}失败：{detail}")
}
