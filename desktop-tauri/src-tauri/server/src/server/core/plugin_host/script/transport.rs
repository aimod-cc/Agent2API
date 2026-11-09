use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};

use crate::server::core::{egress, proxies::{ResolvedProxy, ProxyResolution, resolve_account_proxy}};

struct Download {
    response: tokio::sync::Mutex<Option<reqwest::Response>>,
    pending: Mutex<bytes::Bytes>,
    chunk_limit: usize,
    touched: Mutex<std::time::Instant>,
    cancel: tokio_util::sync::CancellationToken,
    capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
}

fn downloads() -> &'static Mutex<HashMap<String, Arc<Download>>> {
    static ITEMS: OnceLock<Mutex<HashMap<String, Arc<Download>>>> = OnceLock::new();
    ITEMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn error(message: impl Into<String>) -> proto::WireError {
    proto::WireError::with_code(message, proto::WireError::INTERNAL)
}

struct Opening {
    id: Option<String>,
}

impl Drop for Opening {
    fn drop(&mut self) {
        if let Some(id) = &self.id {
            close(&json!({ "id": id }));
        }
    }
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

pub(super) fn proxy_for(url: &url::Url, params: &Value) -> Result<Option<ResolvedProxy>, proto::WireError> {
    let local = match url.host() {
        Some(url::Host::Domain(host)) => host.trim_end_matches('.').eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback()
            || address.to_ipv4_mapped().is_some_and(|address| address.is_loopback()),
        None => false,
    };
    // 插件的本机浏览器控制与回调不能发往账号的远程代理。
    if local { return Ok(None); }
    ResolvedProxy::from_json(params.get("proxy").unwrap_or(&Value::Null)).map_err(error)
}

pub async fn open(params: &Value) -> Result<Value, proto::WireError> {
    open_body(params, None).await
}

pub(in crate::server::core::plugin_host) enum RequestBody {
    Bytes(bytes::Bytes),
    Multipart(reqwest::multipart::Form),
}

pub(in crate::server::core::plugin_host) async fn open_body(params: &Value, body: Option<RequestBody>) -> Result<Value, proto::WireError> {
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
    let proxy = proxy_for(&url, params)?;
    let client = egress::resolved_client(proxy.as_ref(), false, "", &[]).map_err(error)?;
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
        request = match body { RequestBody::Bytes(bytes) => request.body(bytes), RequestBody::Multipart(form) => request.multipart(form) };
    } else if let Some(body) = params["body"].as_str() {
        if body.len() > 120 * 1024 * 1024 {
            return Err(error("脚本请求体过大"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|_| error("脚本请求体编码无效"))?;
        if capture.is_some() {
            sent_body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        }
        request = request.body(bytes);
    }
    if let Some(capture) = &capture {
        capture.reset_request(
            url.as_str(),
            params["provider"].as_str().unwrap_or(""),
            &sent_headers,
            &sent_body,
        );
    }
    let id = params["id"]
        .as_str()
        .filter(|id| id.len() <= 80 && !id.is_empty())
        .ok_or_else(|| error("缺少脚本请求标识"))?
        .to_string();
    let download = Arc::new(Download {
        response: tokio::sync::Mutex::new(None),
        pending: Mutex::new(bytes::Bytes::new()),
        chunk_limit: params["max_chunk"].as_u64().map(|size| size.clamp(1024, 256 * 1024) as usize).unwrap_or(usize::MAX),
        touched: Mutex::new(std::time::Instant::now()),
        cancel: tokio_util::sync::CancellationToken::new(),
        capture: capture.clone(),
    });
    {
        let mut items = downloads()
            .lock()
            .map_err(|_| error("脚本网络会话不可用"))?;
        items.retain(|_, entry| {
            let keep = entry
                .touched
                .lock()
                .is_ok_and(|time| time.elapsed() < Duration::from_secs(1800));
            if !keep {
                entry.cancel.cancel();
            }
            keep
        });
        if items.len() >= 128 || items.contains_key(&id) {
            return Err(error("脚本上游连接数已达上限或请求标识重复"));
        }
        items.insert(id.clone(), download.clone());
    }
    let mut opening = Opening {
        id: Some(id.clone()),
    };
    let timeout = crate::server::config::timeout_settings()
        .headers_ms()
        .clamp(1000, 600_000);
    let response = tokio::select! {
        biased;
        _ = download.cancel.cancelled() => return Err(cancellation()),
        response = tokio::time::timeout(Duration::from_millis(timeout), request.send()) => response
            .map_err(|_| proto::WireError::with_code("脚本上游连接等待超时", "timeout"))?.map_err(|failure| error(egress::describe_error_detail(&failure)))?,
    };
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
    let result = json!({ "id": id, "status": response.status().as_u16(), "headers": headers, "url": response.url().as_str() });
    *download.response.lock().await = Some(response);
    opening.id = None;
    Ok(result)
}

pub async fn read(params: &Value) -> Result<Value, proto::WireError> {
    let id = params["id"]
        .as_str()
        .ok_or_else(|| error("缺少脚本网络会话"))?;
    let download = downloads()
        .lock()
        .map_err(|_| error("脚本网络会话不可用"))?
        .get(id)
        .cloned()
        .ok_or_else(|| error("脚本网络会话已结束"))?;
    if let Ok(mut time) = download.touched.lock() {
        *time = std::time::Instant::now();
    }
    let mut guard = download.response.lock().await;
    if download.cancel.is_cancelled() { return Err(cancellation()); }
    {
        let mut pending = download.pending.lock().map_err(|_| error("网络分片不可用"))?;
        if !pending.is_empty() {
            let size = pending.len().min(download.chunk_limit);
            let bytes = pending.split_to(size);
            return Ok(json!({ "data": base64::engine::general_purpose::STANDARD.encode(bytes), "done": false }));
        }
    }
    let response = guard
        .as_mut()
        .ok_or_else(|| error("脚本上游响应头尚未就绪"))?;
    let wait = crate::server::config::timeout_settings()
        .stream_idle_ms()
        .clamp(1000, 1_800_000);
    let result = tokio::select! {
        biased;
        _ = download.cancel.cancelled() => return Err(cancellation()),
        result = tokio::time::timeout(Duration::from_millis(wait), response.chunk()) => result,
    };
    match result {
        Ok(Ok(Some(mut bytes))) => {
            if let Some(capture) = &download.capture {
                capture.push(&bytes);
            }
            if download.chunk_limit != usize::MAX && bytes.len() > 4 * 1024 * 1024 {
                close(params);
                return Err(error("上游单次网络分片过大"));
            }
            if bytes.len() > download.chunk_limit {
                let rest = bytes.split_off(download.chunk_limit);
                *download.pending.lock().map_err(|_| error("网络分片不可用"))? = rest;
            }
            Ok(
                json!({ "data": base64::engine::general_purpose::STANDARD.encode(bytes), "done": false }),
            )
        }
        Ok(Ok(None)) => {
            close(params);
            Ok(json!({ "done": true }))
        }
        Ok(Err(failure)) => {
            close(params);
            Err(error(egress::describe_error_detail(&failure)))
        }
        Err(_) => {
            close(params);
            Err(proto::WireError::with_code("脚本上游响应空闲超时", "timeout"))
        }
    }
}

pub fn close(params: &Value) {
    if let Some(id) = params["id"].as_str() {
        if let Ok(mut items) = downloads().lock() {
            if let Some(item) = items.remove(id) {
                item.cancel.cancel();
            }
        }
    }
}

pub fn clear() {
    if let Ok(mut items) = downloads().lock() {
        items.retain(|id, item| {
            if id.starts_with("wasm-") { return true; }
            item.cancel.cancel();
            false
        });
    }
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
