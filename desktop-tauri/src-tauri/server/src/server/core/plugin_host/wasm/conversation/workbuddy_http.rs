//! WorkBuddy国内原对话的固定一次HTTP投递；备用线路只在无凭据的发送前检查中选择。
//! 原Call/运行实例/账号/出口只通过宿主私有对象绑定，JSON仅是本次有限请求材料。

#[path = "workbuddy_http/preflight.rs"]
mod preflight;

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{atomic::Ordering, Arc, Weak};

use agent2api_provider_protocol as proto;
use base64::Engine;
use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Value};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::server::core::{
    plugin_host::{self as host, activity::UsageLease, script, Host},
    proxies::chain::SelectedPath,
    upstream::replay_guard::PreparationRef,
};
use crate::server::core::plugin_host::wasm::{
    context::{Context, Shared},
    managed_task::declaration::{self, ParseObjectError},
    runtime::Runtime,
};
use super::super::{ConversationEgress, EndState, Signal};

const PROVIDER: &str = "workbuddy";
const PATH: &str = "/v2/chat/completions";
const ORIGINS: [&str; 3] = [
    "https://www.workbuddy.cn", "https://copilot.tencent.com", "https://wb.tencentbuddy.com",
];
pub(in crate::server::core::plugin_host) const MAX_BODY: usize = 14 * 1024 * 1024;
const MAX_ENCODED: usize = ((MAX_BODY + 2) / 3) * 4;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_NODES: usize = 65_536;
const MAX_DEPTH: usize = 128;

struct Binding {
    signal: Arc<Signal>,
    host: Weak<Host>,
    runtime: Weak<Runtime>,
    shared: Weak<Shared>,
    generation: u64,
    rpc: i64,
    account: String,
    request: String,
    deadline: script::HttpDeadline,
    headers_deadline: script::HttpDeadline,
    call_cancel: CancellationToken,
    preparation: PreparationRef,
    egress: Weak<ConversationEgress>,
}

/// 故意无Clone/serde/自由构造器；只有同步准备门成功才能产生，一次消费后不再返回。
struct SendTicket {
    binding: Binding,
}

struct Material {
    params: Value,
    bytes: Bytes,
    headers: HeaderMap,
    url: String,
}

/// 仅原Context捕获的强执行占用；不能公开拼接Host/Runtime/usage或从JSON恢复。
/// 最后持有者在真实worker/Response/读者退出后释放，active通知不依赖Join槽是否为空。
pub(in crate::server::core::plugin_host) struct WorkBuddyExecution {
    host: Arc<Host>,
    runtime: Option<Arc<Runtime>>,
    usage: Option<Arc<UsageLease>>,
    active: bool,
    call_cancel: CancellationToken,
    call_ended: CancellationToken,
}

impl WorkBuddyExecution {
    fn capture(context: &Context, binding: &Binding) -> Result<Arc<Self>, proto::WireError> {
        let call = context.conversation.as_ref().ok_or_else(unavailable)?;
        if !Arc::ptr_eq(&call.signal, &binding.signal) || call.account != context.account
            || !Weak::ptr_eq(&call.host, &binding.host) || !Arc::ptr_eq(&context.shared, &binding.shared.upgrade().ok_or_else(unavailable)?) {
            return Err(unavailable());
        }
        let host = binding.host.upgrade().ok_or_else(unavailable)?;
        let runtime = binding.runtime.upgrade().ok_or_else(unavailable)?;
        // usage在Host lifecycle门外取得；失败/换代也在全部门退出后释放，避免反向锁序。
        let usage = host.activity.acquire().ok_or_else(unavailable)?;
        let mut execution = Self { host, runtime: Some(runtime), usage: Some(usage), active: false,
            call_cancel: binding.call_cancel.clone(), call_ended: binding.signal.ended.clone() };
        binding.with_gate(false, |_| {
            execution.host.embedded.active.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| count.checked_add(1))
                .map_err(|_| unavailable())?;
            execution.active = true;
            Ok(())
        })?;
        Ok(Arc::new(execution))
    }

    // 仅给原Download监听关闭；不返回Host/Runtime/凭据，也不授予发送资格。
    pub(in crate::server::core::plugin_host) fn cancellation_tokens(&self) -> Result<[CancellationToken; 3], proto::WireError> {
        let runtime = self.runtime.as_ref().ok_or_else(unavailable)?;
        Ok([self.call_cancel.clone(), self.call_ended.clone(), runtime.shared.cancel.clone()])
    }
}

impl Drop for WorkBuddyExecution {
    fn drop(&mut self) {
        drop(self.runtime.take());
        drop(self.usage.take());
        if self.active {
            self.host.embedded.active.fetch_sub(1, Ordering::AcqRel);
            self.active = false;
        }
        // 未poll、prepare/start失败与真实最后读者退出均通知，不把槽空当成退出证明。
        self.host.embedded.idle.notify_waiters();
    }
}

struct Prepared {
    ticket: SendTicket,
    path: SelectedPath,
    material: Material,
    execution: Arc<WorkBuddyExecution>,
}

impl Context {
    /// 同步入口即占用一次准备机会；返回future不借Context，未poll丢弃也不回补。
    /// None只代表非本家或非对话路线；命中路线的任何错误都禁止普通HTTP回退。
    pub(in crate::server::core::plugin_host::wasm) fn workbuddy_http(
        &self, params: &Value, deadline: script::HttpDeadline,
    ) -> impl Future<Output = Result<Option<(Value, script::HttpHandle)>, proto::WireError>> + Send + 'static {
        let prepared = prepare(self, params, deadline);
        async move {
            let Some(Prepared { ticket, path, mut material, execution }) = prepared? else { return Ok(None); };
            let deadline = ticket.binding.deadline;
            let headers_deadline = ticket.binding.headers_deadline;
            let ended = ticket.binding.signal.ended.clone();
            let canceled = ticket.binding.call_cancel.clone();
            let (frozen, url) = tokio::select! {
                biased;
                _ = ended.cancelled() => return Err(unavailable()),
                _ = canceled.cancelled() => return Err(unavailable()),
                result = headers_deadline.run_until(preflight::prepare(path, &material.url, headers_deadline)) => result?,
            };
            deadline.check()?;
            ticket.binding.shared.upgrade().ok_or_else(unavailable)?.options.check_url(&url, false)?;
            ticket.binding.with_gate(true, |_| Ok(()))?;
            material.params["url"] = json!(url);
            material.url = url;
            let Material { params, bytes, headers, url } = material;
            let expected_bytes = bytes.clone();
            let prep = ticket.binding.preparation.clone();
            // 原Download接管整个Frozen包装与实际发送；此后不以外层select丢弃原等待。
            let result = script::open_frozen_once(&params, bytes, frozen, execution, deadline, headers_deadline, move |request| {
                ticket.consume(request, &url, &headers, &expected_bytes)
            }).await;
            match &result {
                Ok((value, _)) if value["status"].as_u64().is_some_and(|status| super::is_safe_http_rejection(status as u16)) => {
                    prep.confirm_unsubmitted_http_rejection();
                }
                Ok((value, _)) if super::supports_http_200_rejection_inspection(PROVIDER, value) => {}
                _ => prep.lock_permanent_replay_block(),
            }
            let result = result?;
            Ok(Some(result))
        }
    }
}

fn prepare(context: &Context, params: &Value, deadline: script::HttpDeadline) -> Result<Option<Prepared>, proto::WireError> {
    let Some(url) = route(context, params)? else { return Ok(None); };
    context.permission("http.v1")?;
    context.permission("chat.stream.v1")?;
    context.shared.options.check_url(&url, false)?;
    let deadline = deadline.cap_at(context.call_deadline);
    let headers_deadline = deadline.cap_at(Instant::now() + std::time::Duration::from_millis(
        crate::server::config::timeout_settings().headers_ms().clamp(1000, 600_000)));
    headers_deadline.check()?;
    deadline.check()?;
    let material = material(context, params, url)?;
    let call = context.conversation.as_ref().ok_or_else(unavailable)?;
    let cancel = context.sink.as_ref().ok_or_else(unavailable)?.cancel.clone();
    if call.provider != PROVIDER || call.account.is_empty() || call.account != context.account
        || !std::ptr::eq(call.shared.as_ptr(), Arc::as_ptr(&context.shared)) || cancel.is_cancelled() {
        return Err(unavailable());
    }
    let host = call.host.upgrade().ok_or_else(unavailable)?;
    // 只摘原运行实例弱身份；真正准入在后面的同Signal/Host/Runtime门内终检。
    let runtime = host.embedded.runtime.try_read().map_err(|_| unavailable())?
        .clone().ok_or_else(unavailable)?;
    if !Arc::ptr_eq(&runtime.shared, &context.shared) { return Err(unavailable()); }
    let egress = call.egress.as_ref().map_err(|_| unavailable())?;
    let binding = Binding {
        signal: call.signal.clone(), host: call.host.clone(), runtime: Arc::downgrade(&runtime),
        shared: call.shared.clone(), generation: call.generation, rpc: call.rpc,
        account: call.account.clone(), request: call.request.clone(), deadline, headers_deadline, call_cancel: cancel,
        preparation: call.preparation.clone(), egress: Arc::downgrade(egress),
    };
    let execution = WorkBuddyExecution::capture(context, &binding)?;
    let path = binding.with_gate(false, |state| {
        let original = binding.egress.upgrade().ok_or_else(unavailable)?;
        if !original.matches(&Arc::downgrade(&binding.signal), PROVIDER, &binding.account) { return Err(unavailable()); }
        // 与close共用原Signal门；永久freeze成功后，任何take/冻结/传输失败均不恢复机会。
        binding.preparation.freeze().map_err(|_| unavailable())?;
        state.http_submission = true;
        // Replay内部锁已退出才取出口槽；不同时嵌套这两把锁，也不从JSON代理补路径。
        original.take_path().map_err(|_| unavailable())
    })?;
    Ok(Some(Prepared { ticket: SendTicket { binding }, path, material, execution }))
}

impl Binding {
    fn with_gate<R>(&self, submitted: bool, action: impl FnOnce(&mut EndState) -> Result<R, proto::WireError>) -> Result<R, proto::WireError> {
        // 短锁序固定Signal→原Host生命周期→原Runtime只读；锁内不等待、构建客户端或发网。
        let mut state = self.signal.state.try_lock().map_err(|_| unavailable())?;
        if !state.guard_issued || state.consumer || state.execution.is_some() || state.task_issued
            || state.source_issued || state.record.is_some() || state.http_submission != submitted
            || self.signal.ended.is_cancelled() || self.call_cancel.is_cancelled()
            || self.deadline.check().is_err() || self.headers_deadline.check().is_err() {
            return Err(unavailable());
        }
        let host = self.host.upgrade().ok_or_else(unavailable)?;
        let shared = self.shared.upgrade().ok_or_else(unavailable)?;
        let original_runtime = self.runtime.upgrade().ok_or_else(unavailable)?;
        if self.rpc <= 0 || self.generation == 0 || self.account.is_empty() || self.account.len() > 512
            || self.request.is_empty() || self.request.len() > 512 || self.account.chars().any(char::is_control)
            || self.request.chars().any(char::is_control) || shared.options.provider != PROVIDER
            || host.activity.retired() || host::runtime::admission_closed() || host::registry::is_switching(PROVIDER)
            || !host::serves_provider(PROVIDER)
            || !host::host_for_provider(PROVIDER).is_some_and(|current| Arc::ptr_eq(&current, &host)) {
            return Err(unavailable());
        }
        let _lifecycle = host.lifecycle.try_lock().map_err(|_| unavailable())?;
        let current = host.embedded.runtime.try_read().map_err(|_| unavailable())?;
        if !host.ready() || host.generation.load(Ordering::Acquire) != self.generation
            || shared.cancel.is_cancelled() || self.call_cancel.is_cancelled() || self.deadline.check().is_err()
            || self.headers_deadline.check().is_err()
            || host.options.script || host.options.wasm.as_ref().is_none_or(|options| options.provider != PROVIDER)
            || !shared.options.allows("http.v1") || !shared.options.allows("chat.stream.v1")
            || !Arc::ptr_eq(&original_runtime.shared, &shared)
            || !current.as_ref().is_some_and(|runtime| Arc::ptr_eq(runtime, &original_runtime)) {
            return Err(unavailable());
        }
        action(&mut state)
    }
}

impl SendTicket {
    fn consume(self, request: &reqwest::Request, url: &str, headers: &HeaderMap, bytes: &Bytes) -> Result<(), proto::WireError> {
        // 核对实际构造请求而非只核JSON；固定材料不得贴到另一URL、另一正文或另一套头。
        if request.method() != reqwest::Method::POST || request.url().as_str() != url
            || request.body().and_then(reqwest::Body::as_bytes) != Some(bytes.as_ref())
            || request.headers().len() != headers.len()
            || headers.iter().any(|(name, value)| request.headers().get(name) != Some(value)) {
            return Err(integrity_mismatch());
        }
        self.binding.headers_deadline.check()?;
        self.binding.with_gate(true, |_| {
            // 本次票按值消费；此freeze只再次核原活动代次，不签发或放回新的发送票。
            self.binding.preparation.freeze().map_err(|_| unavailable())
        })
    }
}

fn route(context: &Context, params: &Value) -> Result<Option<String>, proto::WireError> {
    if context.shared.options.provider != PROVIDER { return Ok(None); }
    route_url(params)
}

fn route_url(params: &Value) -> Result<Option<String>, proto::WireError> {
    let raw = params["url"].as_str().filter(|url| url.len() <= 8192).ok_or_else(invalid_endpoint)?;
    let parsed = url::Url::parse(raw).map_err(|_| invalid_endpoint())?;
    // 识别一次/多次编码及重复斜杠的对话别名后拒绝，不能借普通HTTP绕过固定路线。
    let mut probe = parsed.path().to_string();
    for _ in 0..4 {
        let decoded = super::decoded_path(&probe).map_err(|_| invalid_endpoint())?;
        if decoded == probe { break; }
        probe = decoded;
    }
    if probe.contains('%') { return Err(invalid_endpoint()); }
    let normalized = format!("/{}", probe.split('/').filter(|part| !part.is_empty())
        .map(|part| part.split(';').next().unwrap_or("")).collect::<Vec<_>>().join("/"));
    if !normalized.trim_end_matches('/').eq_ignore_ascii_case(PATH) { return Ok(None); }
    if params["method"].as_str() != Some("POST") || parsed.scheme() != "https"
        || parsed.query().is_some() || parsed.fragment().is_some() || !parsed.username().is_empty()
        || parsed.password().is_some() || !ORIGINS.iter().any(|origin| raw == format!("{origin}{PATH}")) {
        return Err(invalid_endpoint());
    }
    Ok(Some(raw.to_string()))
}

fn material(context: &Context, params: &Value, url: String) -> Result<Material, proto::WireError> {
    let (id, trace) = validate_envelope(params)?;
    let (headers, wire_headers) = validate_headers(&params["headers"])?;
    let bytes = body_bytes(context, params)?;
    validate_chat_body(&bytes)?;
    let params = json!({ "method": "POST", "url": url, "headers": wire_headers, "id": id,
        "provider": PROVIDER, "trace": trace, "max_chunk": 64 * 1024 });
    Ok(Material { params, bytes, headers, url })
}

fn validate_envelope(params: &Value) -> Result<(&str, Value), proto::WireError> {
    let object = params.as_object().ok_or_else(incompatible_params)?;
    if object.keys().any(|key| !matches!(key.as_str(),
        "method" | "url" | "headers" | "body" | "body_resource" | "timeout_ms" | "id" | "provider" | "proxy" | "trace" | "max_chunk")) {
        return Err(incompatible_params());
    }
    if params["provider"] != PROVIDER || params["max_chunk"].as_u64() != Some(64 * 1024) {
        return Err(incompatible_params());
    }
    let id = params["id"].as_str().filter(|id| id.starts_with("wasm-") && id.len() <= 80
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
        .ok_or_else(incompatible_params)?;
    let trace = params.get("trace").and_then(Value::as_str)
        .filter(|trace| trace.len() <= 512 && !trace.chars().any(char::is_control))
        .map(|trace| json!(trace)).unwrap_or(Value::Null);
    Ok((id, trace))
}

fn validate_headers(raw: &Value) -> Result<(HeaderMap, serde_json::Map<String, Value>), proto::WireError> {
    let fields = raw.as_object().filter(|headers| headers.len() <= 128).ok_or_else(invalid_headers)?;
    let mut headers = HeaderMap::new();
    let mut wire_headers = serde_json::Map::new();
    let mut total = 0usize;
    for (name, value) in fields {
        let lower = name.to_ascii_lowercase();
        if forbidden_header(&lower) { return Err(invalid_headers()); }
        let text = value.as_str().ok_or_else(|| {
            if lower == "authorization" { invalid_token() } else { invalid_headers() }
        })?;
        total = total.checked_add(name.len()).and_then(|bytes| bytes.checked_add(text.len()))
            .filter(|bytes| *bytes <= MAX_HEADER_BYTES).ok_or_else(invalid_headers)?;
        let parsed_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid_header_chars())?;
        let mut parsed_value = HeaderValue::from_str(text).map_err(|_| {
            if lower == "authorization" { invalid_token() } else { invalid_header_chars() }
        })?;
        if headers.contains_key(&parsed_name) { return Err(invalid_headers()); }
        if lower == "authorization" { parsed_value.set_sensitive(true); }
        headers.insert(parsed_name, parsed_value);
        wire_headers.insert(name.clone(), json!(text));
    }
    if !headers.get("authorization").and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.strip_prefix("Bearer ").is_some_and(|token| !token.is_empty() && !token.chars().any(char::is_whitespace))) {
        return Err(invalid_token());
    }
    let header_is = |name: &str, expected: &str| headers.get(name).and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected));
    if !header_is("content-type", "application/json") || !header_is("accept", "text/event-stream") {
        return Err(invalid_headers());
    }
    Ok((headers, wire_headers))
}

fn validate_chat_body(bytes: &[u8]) -> Result<(), proto::WireError> {
    let value: Value = declaration::parse_object_detailed(bytes, MAX_BODY, MAX_DEPTH)
        .map_err(|error| match error {
            ParseObjectError::TooLarge => payload_too_large(),
            ParseObjectError::DepthExceeded => depth_exceeded(),
            ParseObjectError::DuplicateKey => duplicate_field(),
            ParseObjectError::Invalid => invalid_body(),
        })?;
    let mut nodes = 0;
    shape(&value, 0, &mut nodes)?;
    if value["stream"] != true {
        return Err(non_stream_request());
    }
    if !value["messages"].as_array().is_some_and(|rows| !rows.is_empty()) {
        return Err(missing_messages());
    }
    if !value["model"].as_str().is_some_and(|model| !model.trim().is_empty() && model.len() <= 1024 && !model.chars().any(char::is_control)) {
        return Err(invalid_model());
    }
    Ok(())
}

fn body_bytes(context: &Context, params: &Value) -> Result<Bytes, proto::WireError> {
    let raw = params.get("body").filter(|value| !value.is_null());
    let resource = params.get("body_resource").filter(|value| !value.is_null());
    match (raw, resource) {
        (Some(raw), None) => {
            let encoded = raw.as_str().filter(|body| !body.is_empty()).ok_or_else(invalid_body)?;
            if encoded.len() > MAX_ENCODED {
                return Err(payload_too_large());
            }
            let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| invalid_body())?;
            if bytes.is_empty() {
                return Err(invalid_body());
            }
            if bytes.len() > MAX_BODY {
                return Err(payload_too_large());
            }
            if base64::engine::general_purpose::STANDARD.encode(&bytes) != encoded {
                return Err(invalid_body());
            }
            Ok(Bytes::from(bytes))
        }
        (None, Some(resource)) => {
            context.permission("resources.v1")?;
            let id = resource.as_str().filter(|id| !id.is_empty()).ok_or_else(invalid_body)?;
            let bytes = context.blobs.raw(id).map_err(|_| invalid_body())?;
            if bytes.is_empty() {
                return Err(invalid_body());
            }
            if bytes.len() > MAX_BODY {
                return Err(payload_too_large());
            }
            Ok(bytes)
        }
        _ => Err(invalid_body()),
    }
}

fn shape(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), proto::WireError> {
    *nodes = nodes.checked_add(1).filter(|nodes| *nodes <= MAX_NODES).ok_or_else(too_many_nodes)?;
    if depth > MAX_DEPTH { return Err(depth_exceeded()); }
    match value {
        Value::Array(items) => for item in items { shape(item, depth + 1, nodes)?; },
        Value::Object(fields) => {
            let mut names = BTreeSet::new();
            for (key, item) in fields {
                if key.len() > 1024 { return Err(field_name_too_long()); }
                if !names.insert(key) { return Err(duplicate_field()); }
                shape(item, depth + 1, nodes)?;
            }
        }
        _ => {},
    }
    Ok(())
}

fn forbidden_header(name: &str) -> bool {
    matches!(name, "host" | "connection" | "content-length" | "transfer-encoding" | "proxy-authorization"
        | "proxy-connection" | "cookie" | "x-refresh-token" | "forwarded" | "x-original-host" | "x-host"
        | "x-original-uri" | "x-original-url" | "x-rewrite-url" | "x-http-method-override" | "x-http-method"
        | "x-method-override" | "http-method-override" | "x-original-method") || name.starts_with("x-forwarded-")
}

fn bad_params(message: impl Into<String>) -> proto::WireError {
    proto::WireError::with_code(message, "bad_params")
}

fn payload_too_large() -> proto::WireError {
    proto::WireError::with_code(
        format!("WorkBuddy 国内版请求内容超过 {} MiB 上限，请缩短对话历史或减少图片后重试", MAX_BODY / (1024 * 1024)),
        "plugin_request_too_large",
    )
}

fn too_many_nodes() -> proto::WireError {
    bad_params("WorkBuddy 国内版请求内容过于复杂（数据结构节点超过 65,536 上限），请精简对话历史、减少工具数量或新开会话重试")
}

fn depth_exceeded() -> proto::WireError {
    bad_params(format!("WorkBuddy 国内版请求内容嵌套过深（超过 {} 层上限），请简化请求或新开会话重试", MAX_DEPTH))
}

fn duplicate_field() -> proto::WireError {
    bad_params("WorkBuddy 国内版请求内容包含重复字段，请检查客户端发送的请求格式")
}

fn field_name_too_long() -> proto::WireError {
    bad_params("WorkBuddy 国内版请求内容包含过长的字段名，请检查客户端发送的请求格式")
}

fn invalid_body() -> proto::WireError {
    bad_params("WorkBuddy 国内版请求内容为空或格式无效，请重试；若持续出现请检查插件与主程序版本")
}

fn missing_messages() -> proto::WireError {
    bad_params("WorkBuddy 国内版请求缺少对话消息，请检查客户端发送的内容")
}

fn invalid_model() -> proto::WireError {
    bad_params("WorkBuddy 国内版请求的模型名称缺失或格式无效")
}

fn non_stream_request() -> proto::WireError {
    bad_params("WorkBuddy 国内版对话通道仅支持流式请求，请将 WorkBuddy 插件更新至最新版本")
}

pub(super) fn invalid_endpoint() -> proto::WireError {
    bad_params("WorkBuddy 国内版对话地址无效（仅允许官方国内对话端点），请检查账号端点设置或重新登录")
}

fn invalid_token() -> proto::WireError {
    bad_params("WorkBuddy 国内版账号登录令牌缺失或格式无效，请重新登录账号")
}

fn invalid_header_chars() -> proto::WireError {
    bad_params("WorkBuddy 国内版请求头包含无效字符，请检查账号信息（如部门名称）或自定义客户端标识")
}

fn invalid_headers() -> proto::WireError {
    bad_params("WorkBuddy 国内版请求头不合规或超出上限，请检查自定义请求头及插件版本")
}

fn incompatible_params() -> proto::WireError {
    bad_params("WorkBuddy 国内版插件与当前主程序参数不兼容，请将主程序与 WorkBuddy 插件更新至最新版本")
}

fn integrity_mismatch() -> proto::WireError {
    bad_params("WorkBuddy 国内版发送前安全核对未通过，请求已中止")
}

fn unavailable() -> proto::WireError {
    proto::WireError::with_code("WorkBuddy原对话调用或一次出口不可用，未重新提交、选路或恢复发送资格", "request_replay_blocked")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_headers() -> Value {
        json!({
            "Authorization": "Bearer test-token-123",
            "Content-Type": "application/json",
            "Accept": "text/event-stream"
        })
    }

    #[test]
    fn route_url_distinguishes_valid_and_invalid_endpoints() {
        let ok = route_url(&json!({
            "method": "POST",
            "url": "https://www.workbuddy.cn/v2/chat/completions"
        }));
        assert_eq!(ok.unwrap().as_deref(), Some("https://www.workbuddy.cn/v2/chat/completions"));

        let non_chat = route_url(&json!({
            "method": "GET",
            "url": "https://www.workbuddy.cn/v2/plugin/user"
        }));
        assert_eq!(non_chat.unwrap(), None);

        let err = route_url(&json!({
            "method": "POST",
            "url": "https://www.codebuddy.cn/v2/chat/completions"
        }))
        .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("bad_params"));
        assert_eq!(err.message, invalid_endpoint().message);
    }

    #[test]
    fn validate_envelope_rejects_unknown_fields_with_clear_message() {
        let valid_params = json!({
            "method": "POST",
            "url": "https://www.workbuddy.cn/v2/chat/completions",
            "headers": valid_headers(),
            "body": "e30=",
            "id": "wasm-net-1",
            "provider": "workbuddy",
            "max_chunk": 65536
        });
        assert!(validate_envelope(&valid_params).is_ok());

        let invalid_params = json!({
            "method": "POST",
            "url": "https://www.workbuddy.cn/v2/chat/completions",
            "headers": valid_headers(),
            "unexpected_field": true,
            "id": "wasm-net-1",
            "provider": "workbuddy",
            "max_chunk": 65536
        });
        let err = validate_envelope(&invalid_params).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("bad_params"));
        assert_eq!(err.message, incompatible_params().message);
    }

    #[test]
    fn validate_headers_distinguishes_token_chars_and_policy_errors() {
        assert!(validate_headers(&valid_headers()).is_ok());

        let missing_token = validate_headers(&json!({
            "Content-Type": "application/json",
            "Accept": "text/event-stream"
        }))
        .unwrap_err();
        assert_eq!(missing_token.message, invalid_token().message);

        let invalid_chars = validate_headers(&json!({
            "Authorization": "Bearer test-token-123",
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
            "X-Department-Info": "line1\nline2"
        }))
        .unwrap_err();
        assert_eq!(invalid_chars.message, invalid_header_chars().message);

        let forbidden = validate_headers(&json!({
            "Authorization": "Bearer test-token-123",
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
            "Cookie": "sid=1"
        }))
        .unwrap_err();
        assert_eq!(forbidden.message, invalid_headers().message);
    }

    #[test]
    fn validate_chat_body_distinguishes_structure_and_field_errors() {
        let ok = br#"{"stream":true,"model":"deepseek-v4.1-flash","messages":[{"role":"user","content":"hi"}]}"#;
        assert!(validate_chat_body(ok).is_ok());

        let dup = br#"{"stream":true,"stream":true,"model":"m","messages":[{"role":"user","content":"hi"}]}"#;
        assert_eq!(validate_chat_body(dup).unwrap_err().message, duplicate_field().message);

        let mut nested_35 = String::from(r#"{"stream":true,"model":"m","messages":[{"role":"user","content":"hi"}],"extra":"#);
        for _ in 0..34 { nested_35.push('['); }
        nested_35.push('1');
        for _ in 0..34 { nested_35.push(']'); }
        nested_35.push('}');
        assert!(validate_chat_body(nested_35.as_bytes()).is_ok());
        // 默认受控声明解析仍保持 32 层上限，不随对话体放宽。
        assert!(declaration::parse_object::<Value>(nested_35.as_bytes(), MAX_BODY).is_err());

        // 根对象 1 层 + 126 层数组 = 127 层容器（含最内层标量共 128 层，对齐 serde_json 默认上限），应通过。
        let mut at_limit = String::from(r#"{"stream":true,"model":"m","messages":[{"role":"user","content":"hi"}],"extra":"#);
        for _ in 0..126 { at_limit.push('['); }
        at_limit.push('1');
        for _ in 0..126 { at_limit.push(']'); }
        at_limit.push('}');
        assert!(validate_chat_body(at_limit.as_bytes()).is_ok());

        // 根对象 1 层 + 127 层数组 = 128 层容器（触达 serde_json 128 层递归上限），应精准报 depth_exceeded 而非 invalid_body。
        let mut deep = String::from(r#"{"stream":true,"model":"m","messages":[{"role":"user","content":"hi"}],"extra":"#);
        for _ in 0..127 { deep.push('['); }
        deep.push('1');
        for _ in 0..127 { deep.push(']'); }
        deep.push('}');
        assert_eq!(validate_chat_body(deep.as_bytes()).unwrap_err().message, depth_exceeded().message);

        let long_key = format!(
            r#"{{"stream":true,"model":"m","messages":[{{"role":"user","content":"hi"}}],"{}":1}}"#,
            "k".repeat(1025)
        );
        assert_eq!(validate_chat_body(long_key.as_bytes()).unwrap_err().message, field_name_too_long().message);

        let many = format!(
            r#"{{"stream":true,"model":"m","messages":[{{"role":"user","content":"hi"}}],"items":[{}]}}"#,
            vec!["0"; MAX_NODES].join(",")
        );
        assert_eq!(validate_chat_body(many.as_bytes()).unwrap_err().message, too_many_nodes().message);

        let non_stream = br#"{"stream":false,"model":"m","messages":[{"role":"user","content":"hi"}]}"#;
        assert_eq!(validate_chat_body(non_stream).unwrap_err().message, non_stream_request().message);

        let empty_messages = br#"{"stream":true,"model":"m","messages":[]}"#;
        assert_eq!(validate_chat_body(empty_messages).unwrap_err().message, missing_messages().message);

        let empty_model = br#"{"stream":true,"model":"   ","messages":[{"role":"user","content":"hi"}]}"#;
        assert_eq!(validate_chat_body(empty_model).unwrap_err().message, invalid_model().message);

        let invalid_json = b"not-json";
        assert_eq!(validate_chat_body(invalid_json).unwrap_err().message, invalid_body().message);

        let oversized = vec![b' '; MAX_BODY + 1];
        let err = validate_chat_body(&oversized).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("plugin_request_too_large"));
        assert_eq!(err.message, payload_too_large().message);
    }
}
