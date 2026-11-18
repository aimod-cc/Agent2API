//! 仅WorkBuddy国内WASM流的有界错误投影；数字业务码与文本语义码不混用。

use agent2api_provider_protocol as proto;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::errors::{GatewayError, PluginErrorMetadata};

const MAX_ERROR_BYTES: usize = 16 * 1024;
const MAX_MESSAGE_BYTES: usize = 8 * 1024;
const MAX_WINDOW: i64 = 8 * 86_400_000;

#[derive(Deserialize)]
struct FrameError {
    message: String,
    #[serde(alias = "statusCode")]
    status: u16,
    #[serde(default, alias = "upstreamCode", alias = "upstream_code")]
    code: Option<i64>,
    #[serde(default, rename = "providerCode", alias = "provider_code")]
    provider_code: Option<String>,
    #[serde(default, rename = "resetAt", alias = "reset_at")]
    reset_at: Option<i64>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

pub(in crate::server::core::plugin_host) fn bounded_reset(at: i64, now: i64) -> bool {
    now > 0 && at > now && at <= now.saturating_add(MAX_WINDOW)
        && at <= (1_i64 << 53) - 1 && chrono::DateTime::from_timestamp_millis(at).is_some()
}

fn bounded_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 4096 && !code.chars().any(char::is_control)
}

pub(in crate::server::core::plugin_host) fn frame(value: &Value) -> GatewayError {
    let decoded = (value.to_string().len() <= MAX_ERROR_BYTES)
        .then(|| serde_json::from_value::<FrameError>(value.clone()).ok()).flatten();
    let Some(frame) = decoded.filter(|frame| (400..600).contains(&frame.status)
        && !frame.message.trim().is_empty() && frame.message.len() <= MAX_MESSAGE_BYTES
        && frame.provider_code.as_deref().is_none_or(bounded_code)
        && frame.kind.as_deref().is_none_or(|kind| kind == "upstream_error")) else {
        return invalid();
    };
    let mut error = GatewayError::with_status(i32::from(frame.status), frame.message);
    error.code = frame.provider_code;
    error.upstream_code = frame.code;
    project(error, frame.reset_at)
}

fn project(mut error: GatewayError, reset: Option<i64>) -> GatewayError {
    if error.message.len() > MAX_MESSAGE_BYTES || error.code.as_deref().is_some_and(|code| !bounded_code(code)) {
        return invalid();
    }
    let now = crate::server::logging::now_ms();
    let reset_at = reset.filter(|at| bounded_reset(*at, now));
    error.retry_after_secs = reset_at.and_then(|at| u64::try_from(at.saturating_sub(now)).ok()).map(|ms| ms.div_ceil(1000));
    error.plugin_metadata = Some(PluginErrorMetadata {
        reset_at, credential_scope: None, provider_code: error.code.clone(),
    });
    error
}

pub(in crate::server::core::plugin_host) fn control(error: proto::WireError) -> GatewayError {
    if error.message.len() > MAX_MESSAGE_BYTES || error.code.as_ref().is_some_and(|code| code.len() > MAX_ERROR_BYTES) {
        return invalid();
    }
    let Some(failure) = super::control_error(&error) else {
        // 本地运行错误不是提供商业务码，不写入provider_code。
        return super::super::rpc::rpc_error("workbuddy", error, proto::methods::gateway::CONVERSATION);
    };
    let reset = failure.plugin_metadata.as_ref().and_then(|metadata| metadata.reset_at);
    project(failure, reset)
}

/// 兼容已发出失败head的标准入口；只投影本次错误，不保留任意JSON。
pub(in crate::server::core::plugin_host) fn response(status: u16, value: &Value) -> GatewayError {
    let value = value.get("error").filter(|error| error.is_object()).unwrap_or(value);
    let message = value.get("message").or_else(|| value.get("msg")).and_then(Value::as_str)
        .or_else(|| value.as_str()).unwrap_or("WorkBuddy返回失败响应");
    let code = value.get("upstreamCode").or_else(|| value.get("upstream_code"))
        .or_else(|| value.get("code")).and_then(|code| code.as_i64().or_else(|| code.as_str()?.parse().ok()));
    let provider_code = value.get("providerCode").or_else(|| value.get("provider_code"))
        .and_then(Value::as_str).or_else(|| value.get("code").and_then(Value::as_str).filter(|code| code.parse::<i64>().is_err()));
    if !(400..600).contains(&status) || message.len() > MAX_MESSAGE_BYTES
        || provider_code.is_some_and(|code| !bounded_code(code)) { return invalid(); }
    let mut error = GatewayError::with_status(i32::from(status), message);
    error.code = provider_code.map(str::to_string);
    error.upstream_code = code;
    project(error, value.get("resetAt").or_else(|| value.get("reset_at")).and_then(Value::as_i64))
}

pub(in crate::server::core::plugin_host) fn stopped(error: &GatewayError) -> bool {
    matches!(error.status_code, 499 | 504)
        || error.message == crate::server::core::upstream::cancellation::MANUAL_TERMINATED
        || error.code.as_deref().is_some_and(|code| matches!(code,
            "aborted" | "cancelled" | "canceled" | "timeout" | "deadline_exceeded"
            | "runtime_stream_interrupted" | "plugin_trap" | "plugin_resource_limit"
            | "plugin_unavailable" | "plugin_protocol_error") || code.contains("cleanup"))
}

fn provider_code(error: &GatewayError) -> Option<&str> {
    error.plugin_metadata.as_ref().and_then(|metadata| metadata.provider_code.as_deref())
        .or_else(|| error.code.as_deref().filter(|code| *code != "plugin_response_started"))
}

pub(in crate::server::core::plugin_host) fn same(left: &GatewayError, right: &GatewayError) -> bool {
    left.status_code == right.status_code && left.message == right.message
        && left.upstream_code.zip(right.upstream_code).is_none_or(|(left, right)| left == right)
        && provider_code(left).zip(provider_code(right)).is_none_or(|(left, right)| left == right)
        && left.plugin_metadata.as_ref().and_then(|metadata| metadata.reset_at)
            .zip(right.plugin_metadata.as_ref().and_then(|metadata| metadata.reset_at))
            .is_none_or(|(left, right)| left == right)
}

/// 首因保留；中断不被业务码覆盖。仅同一状态/说明且不冲突的错误可补字段。
pub(in crate::server::core::plugin_host) fn merge(previous: &mut GatewayError, next: GatewayError) {
    if stopped(previous) { return; }
    if stopped(&next) { *previous = next; return; }
    if !same(previous, &next) { return; }
    if previous.code.is_none() { previous.code = next.code.clone(); }
    if previous.upstream_code.is_none() { previous.upstream_code = next.upstream_code; }
    let next_code = provider_code(&next).filter(|code| bounded_code(code)).map(str::to_string);
    let previous_meta = previous.plugin_metadata.get_or_insert_with(|| PluginErrorMetadata {
        reset_at: None, credential_scope: None, provider_code: None,
    });
    if previous_meta.reset_at.is_none() { previous_meta.reset_at = next.plugin_metadata.as_ref().and_then(|metadata| metadata.reset_at); }
    if previous_meta.provider_code.is_none() { previous_meta.provider_code = next_code; }
    if previous.retry_after_secs.is_none() { previous.retry_after_secs = next.retry_after_secs; }
}

pub(in crate::server::core::plugin_host) fn detail(error: &GatewayError) -> Value {
    json!({ "message": error.message, "code": error.upstream_code,
        "upstreamCode": error.upstream_code, "providerCode": provider_code(error),
        "resetAt": error.plugin_metadata.as_ref().and_then(|metadata| metadata.reset_at) })
}

pub(in crate::server::core::plugin_host) fn stream_frame(error: &GatewayError) -> bytes::Bytes {
    crate::server::core::upstream::sse::sse_frame(&json!({ "error": {
        "message": error.message, "status": error.status_code, "type": "upstream_error",
        "code": error.upstream_code, "providerCode": provider_code(error),
        "resetAt": error.plugin_metadata.as_ref().and_then(|metadata| metadata.reset_at),
    } }))
}

pub(in crate::server::core::plugin_host) fn started(mut error: GatewayError) -> GatewayError {
    // provider_code只由原失败投影填充；本地取消/超时不补造提供商业务码。
    error.code = Some("plugin_response_started".into());
    error
}

pub(in crate::server::core::plugin_host) fn invalid() -> GatewayError {
    GatewayError::with_status(502, "WorkBuddy流错误元数据无效，未据此标记冷却")
        .with_code("plugin_protocol_error")
}
