mod lifecycle;
mod pending;
mod frozen;
pub(in crate::server::core::plugin_host) use lifecycle::{HttpDeadline, HttpHandle};

use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};

use crate::server::core::{egress, proxies::{ResolvedProxy, ProxyResolution, resolve_account_proxy}};
use lifecycle::Download;
use crate::server::core::plugin_host::wasm::conversation::WorkBuddyExecution;

fn error(message: impl Into<String>) -> proto::WireError {
    proto::WireError::with_code(message, proto::WireError::INTERNAL)
}

fn cancellation() -> proto::WireError {
    proto::WireError::with_code("脚本上游请求已取消", proto::WireError::ABORTED)
}

pub fn resolve_proxy(params: &Value) -> Result<Value, proto::WireError> {
    let proxy = match resolve_account_proxy(params.get("config")) {
        Some(ProxyResolution::Resolved(proxy)) => Some(proxy),
        Some(ProxyResolution::Failed(message)) => return Err(error(message)),
        None => None,
    };
    Ok(match proxy {
        Some(proxy) => crate::server::core::proxies::chain::wire_json(&proxy).map_err(error)?,
        None => Value::Null,
    })
}

pub(super) fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => host.trim_end_matches('.').eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback()
            || address.to_ipv4_mapped().is_some_and(|address| address.is_loopback()),
        None => false,
    }
}

pub(super) fn proxy_for(url: &url::Url, params: &Value) -> Result<Option<ResolvedProxy>, proto::WireError> {
    // 插件的本机浏览器控制与回调不能发往账号的远程代理。
    if is_loopback(url) { return Ok(None); }
    ResolvedProxy::from_json(params.get("proxy").unwrap_or(&Value::Null)).map_err(error)
}

pub async fn open(params: &Value) -> Result<Value, proto::WireError> {
    // 脚本 JSON 不是可信期限入口；不能悄悄忽略字段后声称已执行总期限。
    if params.get("timeout_ms").is_some() {
        return Err(proto::WireError::with_code("此入口不接受单请求总期限参数", "bad_params"));
    }
    let (result, handle) = open_with_client(params, None, None, false, HttpDeadline::default()).await?;
    handle.detach();
    Ok(result)
}

pub(in crate::server::core::plugin_host) enum RequestBody {
    Bytes(bytes::Bytes),
    Multipart(reqwest::multipart::Form),
}

pub(in crate::server::core::plugin_host) async fn open_body_with_policy(params: &Value, body: Option<RequestBody>,
    system_proxy_when_unset: bool, deadline: HttpDeadline) -> Result<(Value, HttpHandle), proto::WireError> {
    open_with_client(params, body, None, system_proxy_when_unset, deadline).await
}

// 客户端和期限都是宿主构造的类型化值，不能从插件 JSON 注入 DNS/代理/重定向/期限策略。
pub(in crate::server::core::plugin_host) async fn open_pinned(params: &Value, client: reqwest::Client,
    deadline: HttpDeadline) -> Result<(Value, HttpHandle), proto::WireError> {
    open_with_client(params, None, Some(client), false, deadline).await
}

pub(in crate::server::core::plugin_host) async fn open_pinned_body(params: &Value, body: Option<RequestBody>,
    client: reqwest::Client, deadline: HttpDeadline) -> Result<(Value, HttpHandle), proto::WireError> {
    open_with_client(params, body, Some(client), false, deadline).await
}

// 仅宿主可传整个冻结包装、原强执行占用和非Clone发送票；JSON不能替换它们。
// 本分支同步登记唯一worker后独立发送；外层只等结果，取消/trap不丢已execute的future。
pub(in crate::server::core::plugin_host) async fn open_frozen_once<F>(params: &Value, body: bytes::Bytes,
    owner: egress::managed::FrozenHttpClient, execution: Arc<WorkBuddyExecution>, deadline: HttpDeadline,
    headers_deadline: HttpDeadline, permit: F,
) -> Result<(Value, HttpHandle), proto::WireError>
where F: FnOnce(&reqwest::Request) -> Result<(), proto::WireError> + Send + 'static {
    frozen::open(params, body, owner, execution, deadline, headers_deadline, permit).await
}

async fn open_with_client(params: &Value, body: Option<RequestBody>, pinned: Option<reqwest::Client>,
    system_proxy_when_unset: bool, deadline: HttpDeadline) -> Result<(Value, HttpHandle), proto::WireError> {
    deadline.run(open_inner(params, body, pinned, system_proxy_when_unset, deadline)).await
}

async fn open_inner(params: &Value, body: Option<RequestBody>, pinned: Option<reqwest::Client>,
    system_proxy_when_unset: bool, deadline: HttpDeadline) -> Result<(Value, HttpHandle), proto::WireError> {
    deadline.check()?;
    let sensitive = pinned.is_some();
    let url = params["url"]
        .as_str()
        .ok_or_else(|| error("脚本请求缺少地址"))?;
    let url = url::Url::parse(url).map_err(|_| error("脚本请求地址无效"))?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(error("脚本请求只允许不含账号信息的 HTTP 地址"));
    }
    let client = match pinned {
        Some(client) => client,
        None => {
            let proxy = proxy_for(&url, params)?;
            egress::resolved_client(proxy.as_ref(), system_proxy_when_unset && !is_loopback(&url), "", &[]).map_err(error)?
        }
    };
    deadline.check()?;
    let method = reqwest::Method::from_bytes(params["method"].as_str().unwrap_or("GET").as_bytes())
        .map_err(|_| error("脚本请求方法无效"))?;
    let mut request = client.request(method, url.clone());
    let multipart = matches!(body, Some(RequestBody::Multipart(_)));
    let mut sent_headers = Vec::new();
    if let Some(headers) = params["headers"].as_object() {
        if headers.len() > 128 {
            return Err(error("脚本请求头过多"));
        }
        for (key, value) in headers {
            if multipart && key.eq_ignore_ascii_case("content-type") { continue; }
            if matches!(
                key.to_ascii_lowercase().as_str(),
                "host"
                    | "connection"
                    | "content-length"
                    | "transfer-encoding"
                    | "proxy-authorization"
            ) {
                continue;
            }
            if let Some(value) = value.as_str() {
                request = request.header(key, value);
                sent_headers.push((key.clone(), value.to_string()));
            }
        }
    }
    let capture = params["trace"].as_str().and_then(super::trace::find);
    let mut sent_body = Value::Null;
    if let Some(body) = body {
        request = match body {
            // 资源承载的正文在调试开启时同样留痕；未开启不解析，避免额外复制。
            RequestBody::Bytes(bytes) => {
                if capture.is_some() {
                    sent_body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                }
                request.body(bytes)
            }
            RequestBody::Multipart(form) => request.multipart(form),
        };
    } else if let Some(body) = params["body"].as_str() {
        if body.len() > 120 * 1024 * 1024 {
            return Err(error("脚本请求体过大"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|_| error("脚本请求体编码无效"))?;
        deadline.check()?;
        if capture.is_some() {
            sent_body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        }
        request = request.body(bytes);
    }
    deadline.check()?;
    if let Some(capture) = &capture {
        capture.reset_request(
            url.as_str(),
            params["provider"].as_str().unwrap_or(""),
            &sent_headers,
            &sent_body,
        );
    }
    let handle = lifecycle::register(params, deadline, capture.clone(), sensitive, system_proxy_when_unset)?;
    let download = &handle.download;
    let mut guard = download.wait(download.response.lock()).await?;
    let timeout = crate::server::config::timeout_settings()
        .headers_ms()
        .clamp(1000, 600_000);
    let response = download.wait(tokio::time::timeout(Duration::from_millis(timeout), request.send())).await?
        .map_err(|_| {
            if let Some(trace) = &download.usage_trace {
                trace.failed(crate::server::core::usage_query::network::FailureKind::RequestTimeout,
                    "连接或等待响应头超时");
            }
            proto::WireError::with_code("脚本上游连接等待超时", "timeout")
        })?
        .map_err(|failure| {
            if let (Some(trace), Some(kind)) = (&download.usage_trace,
                crate::server::core::usage_query::network::classify(&failure, false)) {
                trace.failed(kind, &egress::describe_error_detail(&failure));
            }
            if sensitive { error("受控账号服务器连接失败") } else { error(egress::describe_error_detail(&failure)) }
        })?;
    if let Some(trace) = &download.usage_trace { trace.response(response.status().as_u16()); }
    if let Some(capture) = &capture {
        capture.attach_response(response.status().as_u16(), response.headers());
    }
    let headers: serde_json::Map<String, Value> = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), Value::String(value.to_string())))
        })
        .collect();
    // 按原始条目保留同名头；Set-Cookie 不能合并或按逗号拆分。
    let headers_raw: Vec<(String, String)> = response.headers().iter().filter_map(|(name, value)| {
        value.to_str().ok().map(|value| (name.as_str().to_string(), value.to_string()))
    }).collect();
    let result = json!({ "id": handle.id(), "status": response.status().as_u16(), "headers": headers,
        "headers_raw": headers_raw, "url": response.url().as_str() });
    download.check()?;
    *guard = Some(response);
    download.check()?;
    drop(guard);
    Ok((result, handle))
}

pub async fn read(params: &Value) -> Result<Value, proto::WireError> {
    let id = params["id"].as_str().ok_or_else(|| error("缺少脚本网络会话"))?;
    let download = lifecycle::find(id)?;
    if download.holds_frozen() { return Err(error("封闭HTTP响应只允许原Context持有的具体对象读取")); }
    read_download(id, &download).await
}

async fn read_download(id: &str, download: &Arc<Download>) -> Result<Value, proto::WireError> {
    let result = read_inner(download).await;
    let done = result.as_ref().is_ok_and(|value| value["done"] == true);
    let result = if done { result } else { match download.check() { Ok(()) => result, Err(error) => Err(error) } };
    if result.is_err() || done {
        // 包括拿锁、pending、取消等早退错误，不能只删 Context 归属而留下全局下载。
        lifecycle::close_matching(id, download);
    }
    result
}

async fn read_inner(download: &Download) -> Result<Value, proto::WireError> {
    download.check()?;
    *download.wait(download.touched.lock()).await? = std::time::Instant::now();
    download.check()?;
    let mut guard = download.wait(download.response.lock()).await?;
    download.check()?;
    let cached = {
        let mut pending = download.wait(download.pending.lock()).await?;
        download.check()?;
        let encoded = pending.read_encoded(download.chunk_limit)?;
        download.check()?;
        encoded
    };
    download.check()?;
    if let Some(data) = cached {
        let result = json!({ "data": data, "done": false });
        download.check()?;
        return Ok(result);
    }
    let response = guard.as_mut().ok_or_else(|| error("脚本上游响应头尚未就绪"))?;
    let wait = crate::server::config::timeout_settings().stream_idle_ms().clamp(1000, 1_800_000);
    let result = download.wait(tokio::time::timeout(Duration::from_millis(wait), response.chunk())).await?;
    match result {
        Ok(Ok(Some(bytes))) => {
            if let Some(capture) = download.capture().await? { capture.push(&bytes); }
            download.check()?;
            if download.chunk_limit != usize::MAX && bytes.len() > pending::MAX_BYTES {
                return Err(error("上游单次网络分片过大"));
            }
            let size = bytes.len().min(download.chunk_limit);
            if size < bytes.len() {
                let mut pending = download.wait(download.pending.lock()).await?;
                download.check()?;
                pending.store(&bytes[size..])?;
                download.check()?;
            }
            download.check()?;
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes[..size]);
            drop(bytes);
            download.check()?;
            let result = json!({ "data": data, "done": false });
            download.check()?;
            Ok(result)
        }
        Ok(Ok(None)) => {
            download.finish()?;
            if let Some(trace) = &download.usage_trace { trace.finish(); }
            Ok(json!({ "done": true }))
        }
        Ok(Err(failure)) => {
            if let (Some(trace), Some(kind)) = (&download.usage_trace,
                crate::server::core::usage_query::network::classify(&failure, true)) {
                trace.failed(kind, &egress::describe_error_detail(&failure));
            }
            Err(if download.sensitive { error("受控账号服务器响应中断") }
                else { error(egress::describe_error_detail(&failure)) })
        }
        Err(_) => {
            if let Some(trace) = &download.usage_trace {
                trace.failed(crate::server::core::usage_query::network::FailureKind::ResponseTimeout,
                    "读取上游响应超时");
            }
            Err(proto::WireError::with_code("脚本上游响应空闲超时", "timeout"))
        }
    }
}

pub fn close(params: &Value) {
    if let Some(id) = params["id"].as_str() { lifecycle::close_id(id); }
}

pub fn clear() {
    lifecycle::clear_scripts();
    super::protocol::clear();
    super::websocket::clear();
}

pub fn convert(params: &Value) -> Result<Value, proto::WireError> {
    let body = &params["body"];
    let model = params["model"].as_str().unwrap_or("");
    match params["protocol"].as_str().unwrap_or("chat") {
        "anthropic" => {
            crate::server::core::protocol::anthropic_outbound::anthropic_request_from_chat(
                body, model,
            )
            .map_err(error)
        }
        "responses" => {
            crate::server::core::protocol::responses_outbound::responses_request_from_chat(
                body, model,
            )
            .map_err(error)
        }
        "chat" => {
            let mut body = body.clone();
            body["model"] = json!(model);
            Ok(body)
        }
        _ => Err(proto::WireError::with_code(
            "脚本插件使用了尚未支持的模型协议",
            proto::WireError::UNSUPPORTED,
        )),
    }
}
