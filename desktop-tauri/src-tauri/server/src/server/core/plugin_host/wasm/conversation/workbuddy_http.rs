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
use crate::server::core::plugin_host::wasm::{context::{Context, Shared}, runtime::Runtime};
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
            // 原Download接管整个Frozen包装与实际发送；此后不以外层select丢弃原等待。
            let result = script::open_frozen_once(&params, bytes, frozen, execution, deadline, headers_deadline, move |request| {
                ticket.consume(request, &url, &headers, &expected_bytes)
            }).await?;
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
            return Err(invalid());
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
    let raw = params["url"].as_str().filter(|url| url.len() <= 8192).ok_or_else(invalid)?;
    let parsed = url::Url::parse(raw).map_err(|_| invalid())?;
    // 识别一次/多次编码及重复斜杠的对话别名后拒绝，不能借普通HTTP绕过固定路线。
    let mut probe = parsed.path().to_string();
    for _ in 0..4 {
        let decoded = super::decoded_path(&probe).map_err(|_| invalid())?;
        if decoded == probe { break; }
        probe = decoded;
    }
    if probe.contains('%') { return Err(invalid()); }
    let normalized = format!("/{}", probe.split('/').filter(|part| !part.is_empty())
        .map(|part| part.split(';').next().unwrap_or("")).collect::<Vec<_>>().join("/"));
    if !normalized.trim_end_matches('/').eq_ignore_ascii_case(PATH) { return Ok(None); }
    if params["method"].as_str() != Some("POST") || parsed.scheme() != "https"
        || parsed.query().is_some() || parsed.fragment().is_some() || !parsed.username().is_empty()
        || parsed.password().is_some() || !ORIGINS.iter().any(|origin| raw == format!("{origin}{PATH}")) {
        return Err(invalid());
    }
    Ok(Some(raw.to_string()))
}

fn material(context: &Context, params: &Value, url: String) -> Result<Material, proto::WireError> {
    let object = params.as_object().ok_or_else(invalid)?;
    if object.keys().any(|key| !matches!(key.as_str(),
        "method" | "url" | "headers" | "body" | "body_resource" | "timeout_ms" | "id" | "provider" | "proxy" | "trace" | "max_chunk")) {
        return Err(invalid());
    }
    if params["provider"] != PROVIDER || params["max_chunk"].as_u64() != Some(64 * 1024) { return Err(invalid()); }
    let id = params["id"].as_str().filter(|id| id.starts_with("wasm-") && id.len() <= 80
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))).ok_or_else(invalid)?;
    let trace = params.get("trace").and_then(Value::as_str)
        .filter(|trace| trace.len() <= 512 && !trace.chars().any(char::is_control))
        .map(|trace| json!(trace)).unwrap_or(Value::Null);
    let fields = params["headers"].as_object().filter(|headers| headers.len() <= 128).ok_or_else(invalid)?;
    let mut headers = HeaderMap::new();
    let mut wire_headers = serde_json::Map::new();
    let mut total = 0usize;
    for (name, value) in fields {
        let lower = name.to_ascii_lowercase();
        if forbidden_header(&lower) { return Err(invalid()); }
        let text = value.as_str().ok_or_else(invalid)?;
        total = total.checked_add(name.len()).and_then(|bytes| bytes.checked_add(text.len()))
            .filter(|bytes| *bytes <= MAX_HEADER_BYTES).ok_or_else(invalid)?;
        let parsed_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?;
        let mut parsed_value = HeaderValue::from_str(text).map_err(|_| invalid())?;
        if headers.contains_key(&parsed_name) { return Err(invalid()); }
        if lower == "authorization" { parsed_value.set_sensitive(true); }
        headers.insert(parsed_name, parsed_value);
        wire_headers.insert(name.clone(), json!(text));
    }
    let header_is = |name: &str, expected: &str| headers.get(name).and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected));
    if !header_is("content-type", "application/json") || !header_is("accept", "text/event-stream")
        || !headers.get("authorization").and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.strip_prefix("Bearer ").is_some_and(|token| !token.is_empty() && !token.chars().any(char::is_whitespace))) {
        return Err(invalid());
    }
    let bytes = body_bytes(context, params)?;
    let value: Value = crate::server::core::plugin_host::wasm::managed_task::declaration::parse_object(&bytes, MAX_BODY)
        .map_err(|_| invalid())?;
    let mut nodes = 0;
    shape(&value, 0, &mut nodes)?;
    if value["stream"] != true || !value["messages"].as_array().is_some_and(|rows| !rows.is_empty())
        || !value["model"].as_str().is_some_and(|model| !model.trim().is_empty() && model.len() <= 1024 && !model.chars().any(char::is_control)) {
        return Err(invalid());
    }
    let params = json!({ "method": "POST", "url": url, "headers": wire_headers, "id": id,
        "provider": PROVIDER, "trace": trace, "max_chunk": 64 * 1024 });
    Ok(Material { params, bytes, headers, url })
}

fn body_bytes(context: &Context, params: &Value) -> Result<Bytes, proto::WireError> {
    let raw = params.get("body").filter(|value| !value.is_null());
    let resource = params.get("body_resource").filter(|value| !value.is_null());
    match (raw, resource) {
        (Some(raw), None) => {
            let encoded = raw.as_str().filter(|body| !body.is_empty() && body.len() <= MAX_ENCODED).ok_or_else(invalid)?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| invalid())?;
            if bytes.is_empty() || bytes.len() > MAX_BODY || base64::engine::general_purpose::STANDARD.encode(&bytes) != encoded {
                return Err(invalid());
            }
            Ok(Bytes::from(bytes))
        }
        (None, Some(resource)) => {
            context.permission("resources.v1")?;
            let id = resource.as_str().filter(|id| !id.is_empty()).ok_or_else(invalid)?;
            let bytes = context.blobs.raw(id).map_err(|_| invalid())?;
            if bytes.is_empty() {
                return Err(invalid());
            }
            if bytes.len() > MAX_BODY {
                return Err(proto::WireError::with_code(
                    format!("请求正文超过 {} MiB 的受控上限，请缩小上下文或图片后重试", MAX_BODY / (1024 * 1024)),
                    "plugin_request_too_large",
                ));
            }
            Ok(bytes)
        }
        _ => Err(invalid()),
    }
}

fn shape(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), proto::WireError> {
    *nodes = nodes.checked_add(1).filter(|nodes| *nodes <= MAX_NODES).ok_or_else(invalid)?;
    if depth > 32 { return Err(invalid()); }
    match value {
        Value::Array(items) => for item in items { shape(item, depth + 1, nodes)?; },
        Value::Object(fields) => {
            let mut names = BTreeSet::new();
            for (key, item) in fields {
                if key.len() > 1024 || !names.insert(key) { return Err(invalid()); }
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

fn invalid() -> proto::WireError {
    proto::WireError::with_code("WorkBuddy国内固定对话材料无效，未换域、读取资源或回退普通HTTP", "bad_params")
}

fn unavailable() -> proto::WireError {
    proto::WireError::with_code("WorkBuddy原对话调用或一次出口不可用，未重新提交、选路或恢复发送资格", "request_replay_blocked")
}
