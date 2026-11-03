//! 仅 13 个 scoped runtime 与 5 个 builtin ID 的宿主错误投影。
//! raw 仅借给同一次 SDK 分类 RPC；公开消息、代码、错误槽不保存上游自由文本。
//! 旧插件继续使用父模块 control_error / operation_failure。

#[path = "strict_error_codes.rs"]
mod codes;

use std::collections::BTreeMap;

use agent2api_provider_protocol as proto;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::errors::GatewayError;
use crate::server::logging;

const MAX_METADATA: usize = 8192;
const MAX_PRIVATE_MESSAGE: usize = 16_384;
const MAX_SAFE_INTEGER: i64 = (1_i64 << 53) - 1;
const UNKNOWN: &str = "runtime_upstream_error";
const INVALID: &str = "runtime_invalid_error_metadata";

pub(in crate::server::core::plugin_host) fn applies(provider: &str) -> bool {
    crate::server::core::providers::is_scoped_runtime_provider(provider)
}

fn bounded_number(value: i64) -> Option<i64> {
    (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value).then_some(value)
}

fn safe_status(status: u16) -> u16 {
    if (400..600).contains(&status) { status } else { 502 }
}

fn public_code(provider: &str, code: &str) -> String {
    if code.is_empty() { return UNKNOWN.into(); }
    if code.len() > 256 { return INVALID.into(); }
    if numeric_known_code(provider, code.parse().ok()) {
        return code.parse::<i64>().map(|value| value.to_string()).unwrap_or_else(|_| INVALID.into());
    }
    if codes::known(provider, code) { return code.to_string(); }
    // 未知 cleanup 不公开任意码，但仍是不可重试的 Fatal。
    if code.to_ascii_lowercase().contains("cleanup") { return "network_cleanup_unconfirmed".into(); }
    UNKNOWN.into()
}

fn nested_error(body: &Value) -> &Value {
    body.get("error").filter(|value| value.is_object())
        .or_else(|| body.get("detail").filter(|value| value.is_object())).unwrap_or(body)
}

fn body_code(provider: &str, body: &Value) -> String {
    let nested = nested_error(body);
    // 显式错误边界的纯文本仅供 SDK 私有分类；它不是一个可公开的机器码。
    if let Some(text) = body.as_str() { return if text.trim().is_empty() { UNKNOWN.into() } else { String::new() }; }
    if !body.is_object() { return UNKNOWN.into(); }
    let mut http = None;
    for key in ["code", "type", "status"] {
        for value in [nested, body] {
            let Some(raw) = value.get(key).filter(|value| !value.is_null()) else { continue; };
            if let Some(code) = raw.as_str() {
                if code.is_empty() { continue; }
                return if code.len() <= 256 { code.to_string() } else { INVALID.into() };
            }
            if let Some(number) = raw.as_i64() {
                if matches!(key, "code" | "status") && (400..600).contains(&number)
                    && !numeric_known_code(provider, Some(number)) {
                    http.get_or_insert(number);
                    continue;
                }
                return bounded_number(number).map(|value| value.to_string()).unwrap_or_else(|| INVALID.into());
            }
            return INVALID.into();
        }
    }
    if let Some(error) = body.get("error").filter(|error| !error.is_null() && !error.is_object()) {
        return error.as_str().filter(|code| codes::known(provider, code)).map(str::to_string)
            .unwrap_or_else(|| UNKNOWN.into());
    }
    http.map(|value| value.to_string()).unwrap_or_default()
}

fn numeric_code(provider: &str, body: &Value) -> Option<i64> {
    let nested = nested_error(body);
    ["upstreamCode", "upstream_code", "code"].iter().find_map(|key| [nested, body].iter()
        .find_map(|value| value.get(*key).and_then(|raw| raw.as_i64().or_else(|| {
            // 只有已证 Qwen 数字命名空间可从业务码字符串恢复为数值。
            if *key != "code" { return None; }
            let number = raw.as_str()?.parse::<i64>().ok()?;
            numeric_known_code(provider, Some(number)).then_some(number)
        }))))
        .and_then(bounded_number)
}

fn interruption(status: u16, code: &str) -> Option<u16> {
    let code = code.to_ascii_lowercase();
    if matches!(status, 499 | 504) { return Some(status); }
    if matches!(code.as_str(), "499" | "504") { return code.parse().ok(); }
    if code.contains("cleanup") { return Some(safe_status(status)); }
    if matches!(code.as_str(), "aborted" | "abort_err" | "cancelled" | "canceled" | "login_cancelled") {
        return Some(499);
    }
    if matches!(code.as_str(), "timeout" | "network_timeout" | "upstream_timeout" | "login_timeout") {
        return Some(504);
    }
    if status == 556 || matches!(code.as_str(), "556" | proto::WireError::LOCAL_PRECONDITION) { return Some(556); }
    if matches!(code.as_str(), "runtime_protocol_error" | "runtime_stream_interrupted"
        | "runtime_classification_unavailable" | "runtime_transport_unavailable") { return Some(safe_status(status)); }
    None
}

pub(in crate::server::core::plugin_host) fn stopped(error: &GatewayError) -> bool {
    matches!(error.status_code, 499 | 504 | 556)
        || error.code.as_deref().is_some_and(|code| interruption(502, code).is_some())
}

fn message(provider: &str, status: u16, code: &str, class: &UpstreamErrorClass) -> &'static str {
    if status == 499 { return "请求已结束或取消"; }
    if status == 504 { return "提供商请求等待超时"; }
    if code.to_ascii_lowercase().contains("cleanup") { return "请求资源关闭未确认，未继续重试"; }
    if status == 556 || code == proto::WireError::LOCAL_PRECONDITION {
        return match provider {
            "claude-subscription" => "Claude 本机运行前提未满足，请检查原生 CLI 2.1.259+、claude.ai 订阅登录与已关联账号；未复制客户端凭证",
            "grok" => "Grok 本机运行前提未满足，请检查 Grok Build CLI 安装、原客户端登录与已关联账号",
            _ => "本机运行、所选账号或凭证前提未满足，请检查原客户端与已授权账号",
        };
    }
    match class {
        UpstreamErrorClass::QuotaLimited { .. } => "上游账号额度或请求速率已达上限",
        UpstreamErrorClass::TokenExpired { .. } => "提供商凭证不可用，请重新登录或在原客户端续期",
        UpstreamErrorClass::ContentBlocked { .. } => "上游内容策略拒绝了本次请求",
        UpstreamErrorClass::Fatal { .. } => match code {
            "unsupported_external_web_access" => "此提供商不支持禁止外部联网搜索，请移除搜索工具或选择支持此限制的提供商",
            "subscription_sharing_route_not_supported" | "route_not_supported" => "此订阅路线不支持所请求的接口",
            "subscription_sharing_user_not_eligible" => "所选用户、工作区或策略不允许使用此订阅路线",
            "subscription_sharing_unsupported_capability" => "此订阅路线不支持本次输入、工具或模型能力",
            "unsupported" | "no_method" | "not_implemented" => "此提供商不支持该操作",
            "runtime_invalid_error_metadata" => "提供商返回了无效的错误元数据，未采取账号恢复或冷却动作",
            _ => match status {
                400 => "上游未接受本次请求参数",
                401 => "上游认证请求失败，请检查所选账号与授权范围",
                403 => "上游拒绝了本次访问，请检查权限、策略或服务地区",
                404 => "请求资源不存在",
                409 => "请求或完整账号凭证已变化，请重新操作",
                413 => "请求超过大小上限",
                503 => "提供商运行时或上游暂时不可用",
                _ => "提供商请求失败，原始诊断已隐藏",
            },
        },
    }
}

/// class 参数只允许同一次真实 SDK classify RPC 的结果，不读取 raw.class/kind hint。
pub(in crate::server::core::plugin_host) fn sdk_class(provider: &str, status: u16, class: UpstreamErrorClass, body: &Value) -> UpstreamErrorClass {
    let decoded = body.as_str().and_then(|text| serde_json::from_str::<Value>(text).ok());
    let body = decoded.as_ref().unwrap_or(body);
    let status = safe_status(status);
    let code = body_code(provider, body);
    let upstream = numeric_code(provider, body).or_else(|| class_upstream(&class).and_then(bounded_number));
    // runtime_sdk_* 是 Rust 生成的凭证标记，外来同名 code 不得授权处置。
    let class = if !numeric_known_code(provider, upstream)
        && matches!(code.as_str(), "runtime_sdk_quota" | "runtime_sdk_token" | "runtime_sdk_content") {
        UpstreamErrorClass::Fatal { status, message: String::new(), upstream_code: upstream }
    } else { class };
    let class = sanitize_class(provider, status, class, &code, upstream);
    let mut class = if evidenced_action(provider, status, &code, &class, body) { class }
        else { sanitize_class(provider, status, UpstreamErrorClass::Fatal {
            status, message: String::new(), upstream_code: upstream,
        }, &code, upstream) };
    // 已投影 class 的毫秒 reset 是绝对时刻；只在缺失时读取本次 body 的明确单位字段。
    if let UpstreamErrorClass::QuotaLimited { reset_at, .. } = &mut class {
        *reset_at = reset_at.filter(|time| valid_reset(*time)).or_else(|| reset_fields(provider, body));
    }
    class
}

pub(in crate::server::core::plugin_host) fn conversation(
    provider: &str, status: u16, class: UpstreamErrorClass, body: &Value,
    headers: Option<&BTreeMap<String, String>>,
) -> GatewayError {
    let decoded = body.as_str().and_then(|text| serde_json::from_str::<Value>(text).ok());
    let body = decoded.as_ref().unwrap_or(body);
    let code = body_code(provider, body);
    let upstream = numeric_code(provider, body).or_else(|| class_upstream(&class).and_then(bounded_number));
    let status = safe_status(status);
    let class = sanitize_class(provider, status, class, &code, upstream);
    let class_status = class_status(&class);
    let code = if interruption(status, &code).is_some() {
        public_code(provider, &code)
    } else if crate::server::core::providers::availability::is_qwenwork_family(provider) && upstream.is_some()
        && !codes::known(provider, &code) {
        if numeric_known_code(provider, upstream) { upstream.map(|value| value.to_string()).unwrap_or_else(|| UNKNOWN.into()) }
        else { UNKNOWN.into() }
    } else if code.is_empty() {
        match &class {
            UpstreamErrorClass::QuotaLimited { .. } => "runtime_sdk_quota".into(),
            UpstreamErrorClass::TokenExpired { .. } => "runtime_sdk_token".into(),
            UpstreamErrorClass::ContentBlocked { .. } => "runtime_sdk_content".into(),
            UpstreamErrorClass::Fatal { .. } => UNKNOWN.into(),
        }
    } else { public_code(provider, &code) };
    let mut error = GatewayError::with_status(class_status.into(), message(provider, class_status, &code, &class));
    error.code = Some(code);
    error.upstream_code = class_upstream(&class).or(upstream);
    let mut class = class;
    if let UpstreamErrorClass::QuotaLimited { reset_at, .. } = &mut class {
        *reset_at = reset_at.filter(|time| valid_reset(*time))
            .or_else(|| reset_fields(provider, body)).or_else(|| retry_header(headers));
        error.retry_after_secs = reset_at.and_then(seconds_until);
    }
    error.conversation_class = Some(class);
    error
}

fn numeric_known_code(provider: &str, upstream: Option<i64>) -> bool {
    crate::server::core::providers::availability::is_qwenwork_family(provider) && upstream.is_some_and(codes::qwen_number)
}

fn class_status(class: &UpstreamErrorClass) -> u16 {
    match class {
        UpstreamErrorClass::TokenExpired { .. } => 401,
        UpstreamErrorClass::QuotaLimited { status, .. }
        | UpstreamErrorClass::ContentBlocked { status, .. }
        | UpstreamErrorClass::Fatal { status, .. } => *status,
    }
}

fn class_upstream(class: &UpstreamErrorClass) -> Option<i64> {
    match class {
        UpstreamErrorClass::TokenExpired { .. } => None,
        UpstreamErrorClass::QuotaLimited { upstream_code, .. }
        | UpstreamErrorClass::ContentBlocked { upstream_code, .. }
        | UpstreamErrorClass::Fatal { upstream_code, .. } => *upstream_code,
    }
}

fn sanitize_class(provider: &str, status: u16, class: UpstreamErrorClass, code: &str, upstream: Option<i64>) -> UpstreamErrorClass {
    let status = safe_status(status);
    let upstream = upstream.and_then(bounded_number);
    let class_upstream = class_upstream(&class).and_then(bounded_number);
    let stopped = if matches!(status, 499 | 504) { Some(status) }
        else if matches!(class_status(&class), 499 | 504) { Some(class_status(&class)) }
        else { interruption(status, code).or_else(|| interruption(class_status(&class), code)) };
    let numeric_known = crate::server::core::providers::availability::is_qwenwork_family(provider)
        && upstream.or(class_upstream).is_some_and(codes::qwen_number);
    let unknown = (!code.is_empty() && !codes::known(provider, code)
        || matches!(code, "runtime_upstream_error" | "runtime_invalid_error_metadata" | "builtin_upstream_error"
            | "builtin_invalid_error_code" | "builtin_runtime_error" | "runtime_protocol_error"
            | "runtime_stream_interrupted" | "runtime_classification_unavailable" | "runtime_transport_unavailable")) && !numeric_known;
    let mut class = if let Some(status) = stopped {
        UpstreamErrorClass::Fatal { status, message: String::new(), upstream_code: upstream }
    } else if unknown {
        UpstreamErrorClass::Fatal { status, message: String::new(), upstream_code: upstream.or(class_upstream) }
    } else { class };
    if stopped.is_some() || unknown {
        if let UpstreamErrorClass::Fatal { upstream_code, .. } = &mut class {
            *upstream_code = upstream.or(class_upstream);
        }
    }
    match &mut class {
        UpstreamErrorClass::TokenExpired { .. } => {}
        UpstreamErrorClass::QuotaLimited { status, upstream_code, reset_at, .. } => {
            *status = if (400..600).contains(status) { *status } else { 429 };
            *upstream_code = upstream_code.and_then(bounded_number).or(upstream);
            *reset_at = reset_at.filter(|time| valid_reset(*time));
        }
        UpstreamErrorClass::ContentBlocked { status, upstream_code, .. }
        | UpstreamErrorClass::Fatal { status, upstream_code, .. } => {
            *status = safe_status(*status);
            *upstream_code = upstream_code.and_then(bounded_number).or(upstream);
        }
    }
    let safe = message(provider, class_status(&class), code, &class).to_string();
    match &mut class {
        UpstreamErrorClass::TokenExpired { message }
        | UpstreamErrorClass::QuotaLimited { message, .. }
        | UpstreamErrorClass::ContentBlocked { message, .. }
        | UpstreamErrorClass::Fatal { message, .. } => *message = safe,
    }
    class
}

fn evidenced_action(provider: &str, status: u16, code: &str, class: &UpstreamErrorClass, body: &Value) -> bool {
    // SDK 结果是必要凭证；没有正式机器判据的 403/429/5xx 还须具备已确认的私有文案判据。
    let transport = code.is_empty() || code.parse::<u16>().ok() == Some(status)
        || matches!(code, "api_error" | "upstream_error" | "upstream_request_failed" | "upstream_stream_error"
            | "upstream_error_stream" | "upstream_refused" | "copilot_request_error"
            | "cursor_error" | "devin_error" | "qoder_error");
    if !transport || matches!(class, UpstreamErrorClass::Fatal { .. }) { return true; }
    if let Some(upstream) = numeric_code(provider, body).or_else(|| class_upstream(class).and_then(bounded_number)) {
        if numeric_known_code(provider, Some(upstream)) { return true; }
    }
    let nested = nested_error(body);
    let raw = [nested.get("message"), nested.get("msg"), nested.get("error_description"), body.get("message"), body.get("msg"), body.get("error_description"), body.get("error"), body.get("detail"), Some(body)]
        .into_iter().flatten().find_map(Value::as_str).unwrap_or("");
    if raw.len() > MAX_PRIVATE_MESSAGE { return false; }
    let lower = raw.to_lowercase();
    match class {
        UpstreamErrorClass::TokenExpired { .. } => status == 401,
        UpstreamErrorClass::QuotaLimited { .. } => provider == "copilot-subscription" && status == 429
            || status < 500 && !context_too_long(&lower) && (quota_words(&lower) || native_limit(provider, &lower)),
        UpstreamErrorClass::ContentBlocked { .. } => provider == "devin" && lower.contains("content policy"),
        UpstreamErrorClass::Fatal { .. } => true,
    }
}

fn native_limit(provider: &str, message: &str) -> bool {
    matches!(provider, "cursor" | "devin") && message.split_once("limit reached|")
        .is_some_and(|(_, rest)| rest.bytes().take_while(u8::is_ascii_digit).count() == 10)
}

fn context_too_long(message: &str) -> bool {
    ["context length", "context_length", "context-too-long", "context_too_long", "too many tokens",
        "too long", "maximum context", "exceeded model token limit", "request too large", "input is too large"]
        .iter().any(|word| message.contains(word))
}

fn quota_words(message: &str) -> bool {
    ["insufficient quota", "insufficient_quota", "insufficient balance", "insufficient credit",
        "credits exhausted", "credit exhausted", "out of budget", "budget exceeded", "budget exhausted",
        "usage limit", "usage_limit", "quota exceeded", "quota exhausted", "quota limit", "out of quota",
        "hit your usage limit", "rate limit", "rate_limit", "too many requests", "余额不足", "积分不足",
        "额度耗尽", "额度不足", "额度已用完", "限额已用完", "用量上限", "频率过高", "太频繁"]
        .iter().any(|word| message.contains(word))
}

pub(in crate::server::core::plugin_host) fn fatal(provider: &str, status: u16, code: &str) -> GatewayError {
    let body = json!({ "code": code });
    conversation(provider, status, UpstreamErrorClass::Fatal { status, message: String::new(), upstream_code: None }, &body, None)
}

/// 只重投影已在 Rust 错误槽中保存的内部凭证；无凭证不得按 HTTP 429 升级。
pub(in crate::server::core::plugin_host) fn reproject(provider: &str, error: &GatewayError) -> GatewayError {
    let status = u16::try_from(error.status_code).ok().map(safe_status).unwrap_or(502);
    let body = json!({ "code": error.code, "upstreamCode": error.upstream_code });
    let class = match error.conversation_class.as_ref() {
        Some(UpstreamErrorClass::QuotaLimited { status, upstream_code, reset_at, .. }) => UpstreamErrorClass::QuotaLimited {
            status: *status, upstream_code: *upstream_code, reset_at: *reset_at, message: String::new(),
        },
        Some(UpstreamErrorClass::TokenExpired { .. }) => UpstreamErrorClass::TokenExpired { message: String::new() },
        Some(UpstreamErrorClass::ContentBlocked { status, upstream_code, .. }) => UpstreamErrorClass::ContentBlocked {
            status: *status, upstream_code: *upstream_code, message: String::new(),
        },
        Some(UpstreamErrorClass::Fatal { status, upstream_code, .. }) => UpstreamErrorClass::Fatal {
            status: *status, upstream_code: *upstream_code, message: String::new(),
        },
        None => UpstreamErrorClass::Fatal {
            status, message: String::new(), upstream_code: error.upstream_code.and_then(bounded_number),
        },
    };
    let mut result = conversation(provider, status, class, &body, None);
    if matches!(error.code.as_deref(), Some("runtime_sdk_quota" | "runtime_sdk_token" | "runtime_sdk_content"))
        && error.conversation_class.is_none() {
        return fatal(provider, status, UNKNOWN);
    }
    if let Some(UpstreamErrorClass::QuotaLimited { reset_at, .. }) = &mut result.conversation_class {
        if reset_at.is_none() { *reset_at = error.retry_after_secs.and_then(retry_reset); }
        result.retry_after_secs = reset_at.and_then(seconds_until);
    }
    result
}

pub(in crate::server::core::plugin_host) fn cached_class(provider: &str, error: &GatewayError) -> UpstreamErrorClass {
    let GatewayError { conversation_class, status_code, message, upstream_code, .. } = reproject(provider, error);
    conversation_class.unwrap_or_else(|| UpstreamErrorClass::Fatal {
        status: safe_status(status_code as u16), message, upstream_code,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlMetadata {
    status: u16,
    #[serde(default)] code: Option<Value>,
    #[serde(default, alias = "upstream_code")] upstream_code: Option<i64>,
    #[serde(default, alias = "sign_in")] sign_in: Option<String>,
    #[serde(default, alias = "credential_scope")] credential_scope: Option<String>,
    #[serde(default, alias = "reset_at")] reset_at: Option<Value>,
    #[serde(default, alias = "resets_at")] resets_at: Option<Value>,
    #[serde(default, alias = "retry_after_secs", alias = "retryAfter")] retry_after_secs: Option<u64>,
}

fn control_metadata(error: &proto::WireError) -> Option<ControlMetadata> {
    let payload = error.code.as_deref()?.strip_prefix("provider_http:")?;
    if payload.len() > MAX_METADATA { return None; }
    let metadata: ControlMetadata = serde_json::from_str(payload).ok()?;
    if !(400..600).contains(&metadata.status) { return None; }
    if metadata.sign_in.as_deref().is_some_and(|value| !matches!(value, "kept" | "expired" | "renewed")) { return None; }
    if metadata.credential_scope.as_deref().is_some_and(|value| !codes::credential_scope(value)) { return None; }
    let code_shape = metadata.code.as_ref().is_none_or(|value| value.is_null()
        || value.as_str().is_some_and(|value| value.len() <= 256)
        || value.as_i64().and_then(bounded_number).is_some());
    if !code_shape { return None; }
    if metadata.upstream_code.is_some_and(|value| bounded_number(value).is_none()) { return None; }
    for value in [&metadata.reset_at, &metadata.resets_at].into_iter().flatten() {
        if !(value.is_null() || value.as_i64().and_then(bounded_number).is_some()
            || value.as_str().is_some_and(|value| value.len() <= 128)) { return None; }
    }
    Some(metadata)
}

/// 返回仅供当前 classify RPC 的私有正文；禁止写日志或作为公开 fallback。
pub(in crate::server::core::plugin_host) fn private_control(error: &proto::WireError) -> Option<(u16, Value)> {
    let metadata = control_metadata(error)?;
    let mut body = Map::new();
    body.insert("code".into(), metadata.code.unwrap_or(Value::Null));
    body.insert("upstreamCode".into(), metadata.upstream_code.and_then(bounded_number).map(Value::from).unwrap_or(Value::Null));
    if error.message.len() <= MAX_PRIVATE_MESSAGE { body.insert("message".into(), error.message.clone().into()); }
    if let Some(value) = metadata.sign_in { body.insert("signIn".into(), value.into()); }
    if let Some(value) = metadata.credential_scope { body.insert("credentialScope".into(), value.into()); }
    if let Some(value) = metadata.reset_at { body.insert("resetAt".into(), value); }
    if let Some(value) = metadata.resets_at { body.insert("resets_at".into(), value); }
    if let Some(value) = metadata.retry_after_secs { body.insert("retryAfterSecs".into(), value.into()); }
    Some((metadata.status, Value::Object(body)))
}

pub(in crate::server::core::plugin_host) fn wire_error(provider: &str, error: &proto::WireError, method: &str) -> GatewayError {
    if let Some((status, body)) = private_control(error) {
        // 控制面没有分类 RPC 的处置信任凭证，明确 Fatal；正文仅用于有限代码与 status 投影。
        return conversation(provider, status, UpstreamErrorClass::Fatal { status, message: String::new(), upstream_code: None }, &body, None);
    }
    if error.code.as_deref().is_some_and(|code| code.starts_with("provider_http:")) {
        return fatal(provider, 502, INVALID);
    }
    let code = error.code.as_deref().unwrap_or(UNKNOWN);
    let status = match code {
        proto::WireError::LOCAL_PRECONDITION => 556,
        proto::WireError::BAD_PARAMS => 400,
        proto::WireError::NOT_FOUND => 404,
        proto::WireError::NO_METHOD | proto::WireError::UNSUPPORTED => 501,
        proto::WireError::ABORTED | "ABORT_ERR" | "cancelled" => 499,
        "timeout" | "upstream_timeout" | "network_timeout" => 504,
        "busy" | "transport_closed" => 503,
        _ => code.parse::<u16>().ok().filter(|status| (400..600).contains(status)
            && code == status.to_string().as_str()).unwrap_or(502),
    };
    let mut projected = fatal(provider, status, code);
    if method != proto::methods::gateway::CONVERSATION && !matches!(status, 499 | 504 | 556) {
        projected.message = super::operation_failure(method, &projected.message);
    }
    projected
}

/// 流内错误只接显式 error 信封，保留短暂私有分类判据；公开错误槽只存 GatewayError。
pub(in crate::server::core::plugin_host) fn stream_failure(provider: &str, frame: &Value) -> Option<(GatewayError, Value)> {
    let raw = frame.get("error").filter(|value| crate::server::core::protocol::is_truthy(value))?;
    let statuses = [raw.get("statusCode"), raw.get("status_code"), raw.get("status"), frame.get("status"), frame.get("status_code")]
        .map(|value| value.and_then(Value::as_u64).and_then(|value| u16::try_from(value).ok())
            .filter(|value| (400..600).contains(value)));
    let status = statuses.iter().flatten().copied().find(|value| matches!(value, 499 | 504))
        .or_else(|| statuses.iter().flatten().copied().find(|value| *value == 556))
        .or_else(|| statuses.into_iter().flatten().next()).unwrap_or(502);
    let mut body = private_stream_body(raw);
    // 首次观察到的真实状态随私有输入保留，不能在错误槽回读时改成默认 502。
    if let Some(object) = body.as_object_mut() { object.insert("statusCode".into(), status.into()); }
    let projected = conversation(provider, status, UpstreamErrorClass::Fatal {
        status, message: String::new(), upstream_code: numeric_code(provider, &body),
    }, &body, None);
    if let Some(object) = body.as_object_mut() { object.insert("statusCode".into(), projected.status_code.into()); }
    Some((projected, body))
}

fn private_stream_body(raw: &Value) -> Value {
    let mut body = Map::new();
    if !raw.is_object() {
        if let Some(text) = raw.as_str().filter(|text| !text.trim().is_empty() && text.len() <= MAX_PRIVATE_MESSAGE) {
            return json!({ "message": text });
        }
        return json!({ "code": UNKNOWN });
    }
    for key in ["code", "type", "status", "upstreamCode", "upstream_code", "resetAt", "reset_at", "resetsAt", "resets_at", "retryAfterSecs", "retry_after_secs", "retryAfter"] {
        if let Some(value) = raw.get(key) {
            let scalar = value.is_null() || value.is_number() || value.as_str().is_some_and(|value| value.len() <= 256);
            body.insert(key.into(), if scalar { value.clone() } else { Value::String(INVALID.into()) });
        }
    }
    for key in ["message", "msg", "error_description"] {
        if let Some(value) = raw.get(key).and_then(Value::as_str).filter(|value| value.len() <= MAX_PRIVATE_MESSAGE) {
            body.insert(key.into(), value.into());
        }
    }
    Value::Object(body)
}

pub(in crate::server::core::plugin_host) fn classification_input(error: &GatewayError, private: Option<&Value>) -> Value {
    private.cloned().unwrap_or_else(|| json!({ "code": error.code, "upstreamCode": error.upstream_code }))
}

pub(in crate::server::core::plugin_host) fn login_poll(mut poll: proto::LoginPoll) -> proto::LoginPoll {
    match &mut poll {
        proto::LoginPoll::Failed { message } => *message = "登录未完成，请检查原客户端与所选账号后重新授权；原始诊断已隐藏".into(),
        proto::LoginPoll::Pending { hint } if hint.is_some() => *hint = Some("登录尚未完成，请在授权窗口或原客户端继续操作".into()),
        _ => {}
    }
    poll
}

pub(in crate::server::core::plugin_host) fn retry_decision(
    decision: crate::server::core::providers::adapter::RetryDecision,
) -> crate::server::core::providers::adapter::RetryDecision {
    use crate::server::core::providers::adapter::RetryDecision;
    match decision {
        RetryDecision::Default => RetryDecision::Default,
        RetryDecision::Retry { delay_ms, .. } => RetryDecision::Retry { delay_ms, reason: "提供商建议等待后重试".into() },
        RetryDecision::Stop { .. } => RetryDecision::Stop { reason: "提供商停止本次请求，原始诊断已隐藏".into() },
    }
}

pub(in crate::server::core::plugin_host) fn preserve_retry(result: &mut GatewayError, source: &GatewayError) {
    if let Some(UpstreamErrorClass::QuotaLimited { reset_at, .. }) = &mut result.conversation_class {
        if reset_at.is_none() { *reset_at = source.retry_after_secs.and_then(retry_reset); }
        result.retry_after_secs = reset_at.and_then(seconds_until);
    }
}

fn valid_reset(value: i64) -> bool {
    value > logging::now_ms() && value <= MAX_SAFE_INTEGER && chrono::DateTime::from_timestamp_millis(value).is_some()
}

fn epoch_ms(value: &Value) -> Option<i64> {
    if let Some(text) = value.as_str() {
        if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) { return Some(time.timestamp_millis()).filter(|time| valid_reset(*time)); }
    }
    value.as_i64().or_else(|| value.as_str()?.parse().ok()).filter(|time| valid_reset(*time))
}

fn reset_fields(provider: &str, body: &Value) -> Option<i64> {
    let nested = nested_error(body);
    for value in [body, nested] {
        for key in ["resetAt", "reset_at", "resetsAt", "resets_at"] {
            let reset = value.get(key).and_then(|value| {
                if provider == "codex-subscription" && key == "resets_at" {
                    value.as_i64().or_else(|| value.as_str()?.parse::<i64>().ok())?
                        .checked_mul(1000).filter(|time| valid_reset(*time))
                } else { epoch_ms(value) }
            });
            if reset.is_some() { return reset; }
        }
        for key in ["retryAfterSecs", "retry_after_secs", "retryAfter"] {
            if let Some(reset) = value.get(key).and_then(Value::as_u64).and_then(retry_reset) { return Some(reset); }
        }
    }
    None
}

fn retry_reset(seconds: u64) -> Option<i64> {
    if seconds == 0 { return None; }
    let reset = i64::try_from(seconds).ok()?.checked_mul(1000)?.checked_add(logging::now_ms())?;
    valid_reset(reset).then_some(reset)
}

fn retry_header(headers: Option<&BTreeMap<String, String>>) -> Option<i64> {
    let retry = headers?.iter().find(|(key, _)| key.eq_ignore_ascii_case("retry-after"))?.1.trim();
    if let Ok(seconds) = retry.parse() { return retry_reset(seconds); }
    chrono::DateTime::parse_from_rfc2822(retry).ok().map(|time| time.timestamp_millis()).filter(|time| valid_reset(*time))
}

fn seconds_until(reset: i64) -> Option<u64> {
    let remaining = reset.checked_sub(logging::now_ms())?;
    if remaining <= 0 { return None; }
    u64::try_from(remaining).ok()?.checked_add(999).map(|value| value / 1000)
}
