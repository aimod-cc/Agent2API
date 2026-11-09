use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{json, Value};

use crate::server::core::protocol::native::WireProtocol;
use crate::server::core::upstream::{cancellation, usage::RequestTelemetry};
use crate::server::errors::GatewayError;
use crate::server::core::plugin_host::{Host, StreamEvent};
use super::super::{account_egress, rpc, RpcStream};

pub fn protocol(provider: &str, model: &str) -> Option<WireProtocol> {
    if !super::runtime::host().is_some_and(|host| {
        crate::server::core::plugin_host::host_for_provider(provider).is_some_and(|owner| Arc::ptr_eq(&owner, &host))
    }) { return None; }
    let rows = crate::server::core::plugin_host::rpc_cache::models_for(provider)?;
    let row = rows.iter().find(|row| row["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(model)))?;
    row_protocol(row)
}

pub fn has_protocol(provider: &str, protocol: WireProtocol) -> bool {
    if !crate::server::core::plugin_host::host_for_provider(provider).is_some_and(|host| host.options.script) { return false; }
    crate::server::core::plugin_host::rpc_cache::models_for(provider)
        .is_some_and(|rows| rows.iter().any(|row| row_protocol(row) == Some(protocol)))
}

fn row_protocol(row: &Value) -> Option<WireProtocol> {
    let npm = row.pointer("/scriptApi/npm").or_else(|| row.pointer("/scriptModel/api/npm"))?.as_str()?;
    match npm {
        "@ai-sdk/anthropic" => Some(WireProtocol::Anthropic),
        "@ai-sdk/openai" => Some(WireProtocol::Responses),
        "@ai-sdk/openai-compatible" | "@ai-sdk/groq" | "@ai-sdk/deepseek" | "@ai-sdk/xai" => Some(WireProtocol::Chat),
        _ => None,
    }
}

pub async fn forward(
    provider: &str, account: &str, body: Value, headers: &axum::http::HeaderMap,
    proxy: Option<crate::server::core::proxies::ResolvedProxy>, protocol: WireProtocol,
    requested: &str, stream: bool, telemetry: &Arc<RequestTelemetry>,
) -> Result<crate::server::core::upstream::ForwardOutcome, GatewayError> {
    let host: Arc<Host> = crate::server::core::plugin_host::host_for_provider(provider)
        .ok_or_else(|| GatewayError::with_status(503, "脚本提供商尚未就绪"))?;
    let lease = host.activity.acquire().ok_or_else(|| GatewayError::with_status(503, "插件正在切换"))?;
    let model = body["model"].as_str().unwrap_or("").to_string();
    let mut params = json!({ "provider": provider, "account": account, "body": body,
        "headers": headers.iter().filter_map(|(key, value)| value.to_str().ok().map(|value| (key.as_str().to_string(), Value::String(value.to_string())))).collect::<serde_json::Map<String, Value>>(),
        "proxy": proxy.as_ref().map(crate::server::core::proxies::chain::wire_json).transpose()
            .map_err(|message| GatewayError::with_status(502, message))?,
        "session": crate::server::core::upstream::plugin_session_id(headers, &body),
        "wire_model": model, "native_protocol": protocol.as_str(),
        "script_trace": super::trace::register(telemetry),
    });
    account_egress::apply(&mut params)?;
    let call = host.open_stream("conversation", params).await.map_err(|error| GatewayError::with_status(502, error))?;
    let id = call.id;
    let opening = rpc::ErrorBodyScope::new(host.clone(), id, lease.clone());
    let mut events = call.events;
    let receive = tokio::time::timeout(Duration::from_millis(crate::server::config::timeout_settings().headers_ms()), events.recv());
    let first = if let Some(token) = telemetry.cancel_token() {
        tokio::select! {
            biased;
            _ = token.cancelled() => return Err(cancellation::cancelled_error()),
            result = receive => result,
        }
    } else { receive.await };
    let head = match first {
        Ok(Some(StreamEvent::Head(head))) => head,
        Ok(Some(StreamEvent::Done(Err(error)))) => return Err(control_failure(provider, error).await),
        Ok(Some(StreamEvent::Failed(message))) => return Err(GatewayError::with_status(502, message)),
        Err(_) => return Err(GatewayError::with_status(504, "等待插件原生响应头超时")),
        _ => return Err(GatewayError::with_status(502, "插件未返回原生响应头")),
    };
    let adapter = crate::server::core::plugin_host::lookup(provider)
        .ok_or_else(|| GatewayError::with_status(503, "脚本提供商已停止"))?;
    let source = RpcStream::with_pending(adapter.provider_id(), host, id, events, None,
        telemetry.clone(), None, None, lease).raw();
    opening.handoff();
    if head.status >= 400 {
        let source = crate::server::core::upstream::stall::idle_guard(source.boxed(),
            Duration::from_millis(crate::server::config::timeout_settings().stream_idle_ms()));
        let mut source = cancellation::cancellable(source, telemetry.cancel_token());
        let read = async {
            let mut bytes = Vec::new();
            while let Some(chunk) = source.next().await {
                bytes.extend_from_slice(&chunk.map_err(|error| GatewayError::with_status(502, error.to_string()))?);
                if bytes.len() > 2 * 1024 * 1024 { return Err(GatewayError::with_status(502, "插件错误响应过大")); }
            }
            Ok::<_, GatewayError>(bytes)
        };
        let bytes = tokio::time::timeout(Duration::from_millis(crate::server::config::timeout_settings().body_ms()), read)
            .await.map_err(|_| GatewayError::with_status(504, "等待插件错误响应超时"))??;
        let body: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        let class = adapter.classify_error_async(head.status, &body).await;
        return Err(rpc::conversation_error(head.status, class, &body));
    }
    if head.headers.get("x-agent2api-native").map(String::as_str) != Some(protocol.as_str()) {
        return Err(GatewayError::with_status(502, "插件没有确认原生响应协议"));
    }
    let mut kept = axum::http::HeaderMap::new();
    for (key, value) in &head.headers {
        if let (Ok(key), Ok(value)) = (axum::http::HeaderName::from_bytes(key.as_bytes()), axum::http::HeaderValue::from_str(value)) { kept.insert(key, value); }
    }
    let response = crate::server::core::upstream::native::receive_plugin(Box::new(source), protocol, head.status,
        crate::server::core::upstream::native::response_headers(&kept), requested, stream, telemetry.clone()).await?;
    Ok(crate::server::core::upstream::ForwardOutcome::Native(response))
}

async fn control_failure(provider: &str, error: agent2api_provider_protocol::WireError) -> GatewayError {
    let mut failure = rpc::rpc_error(provider, error.clone(), "conversation");
    let Some(mut detail) = error.code.as_deref().and_then(|code| code.strip_prefix("provider_http:"))
        .and_then(|text| serde_json::from_str::<Value>(text).ok()).filter(Value::is_object) else { return failure; };
    let Some(adapter) = crate::server::core::plugin_host::lookup(provider) else { return failure; };
    detail["message"] = json!(failure.message);
    let status = u16::try_from(failure.status_code).unwrap_or(502);
    let class = adapter.classify_error_async(status, &detail).await;
    let mut classified = rpc::conversation_error(status, class, &detail);
    classified.code = failure.code.take();
    classified.with_optional_code(failure.upstream_code)
}
