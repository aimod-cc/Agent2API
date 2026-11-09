use std::sync::Arc;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::custom_providers;
use crate::server::core::model_rules;
use crate::server::core::protocol::{anthropic_thinking, native::WireProtocol};
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::connections::ConnectionGuard;
use crate::server::core::upstream::request::{
    read_upstream_error, send_chat_request, TransportRequest,
};
use crate::server::core::upstream::usage::{LogPhase, RequestTelemetry};
use crate::server::core::upstream::{cancellation, native, ForwardOutcome, InFlightGuard};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{apply_user_agent_override, classify_custom_error, describe_proxy, ProviderQuirks};

pub(super) async fn forward(
    protocol: WireProtocol,
    provider_id: &str,
    account_id: &str,
    api_key: &str,
    user_agent: &str,
    base_url: &str,
    quirks: &ProviderQuirks,
    body: &Value,
    route_model: Option<&str>,
    client_headers: &HeaderMap,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
    slot: &mut Option<InFlightGuard>,
    connections: &mut ConnectionGuard,
) -> Result<ForwardOutcome, GatewayError> {
    let requested = body.get("model").and_then(Value::as_str).unwrap_or("");
    let (wire_model, reasoning) = if route_model.is_some() {
        (requested.to_string(), None)
    } else {
        custom_providers::wire_model_for(provider_id, requested)
    };
    let mut outbound = body.clone();
    if !wire_model.is_empty() {
        outbound["model"] = wire_model.clone().into();
    }
    if let Some(level) = reasoning
        .as_deref()
        .filter(|level| !level.trim().is_empty() && !model_rules::reasoning_is_off(level))
    {
        match protocol {
            WireProtocol::Anthropic => {
                anthropic_thinking::set_effort(&mut outbound, &wire_model, level)
            }
            WireProtocol::Responses => {
                let mut reasoning = outbound
                    .get("reasoning")
                    .filter(|value| value.is_object())
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                reasoning["effort"] = level.into();
                outbound["reasoning"] = reasoning;
            }
            WireProtocol::Chat => {
                outbound["reasoning_effort"] = level.into();
            }
        }
    }
    if protocol == WireProtocol::Anthropic {
        let changes = anthropic_thinking::normalize(&mut outbound, &wire_model);
        if !changes.is_empty() {
            logging::verbose(
                "[CustomProvider]",
                &format!(
                    "provider={provider_id} model={wire_model}：{}",
                    changes.join("；")
                ),
            );
        }
        if quirks.tool_type_custom {
            if let Some(tools) = outbound.get_mut("tools").and_then(Value::as_array_mut) {
                for tool in tools {
                    if let Some(object) = tool
                        .as_object_mut()
                        .filter(|tool| !tool.contains_key("type"))
                    {
                        object.insert("type".into(), "custom".into());
                    }
                }
            }
        }
    }
    let base = base_url.trim_end_matches('/');
    let path = match protocol {
        WireProtocol::Anthropic if !base.ends_with("/v1") => "/v1/messages",
        WireProtocol::Anthropic => "/messages",
        WireProtocol::Responses => "/responses",
        WireProtocol::Chat => "/chat/completions",
    };
    let url = format!("{base}{path}{}", quirks.url_suffix);
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    if protocol == WireProtocol::Anthropic {
        headers.push(("anthropic-version".into(), "2023-06-01".into()));
    }
    // 只携带协议功能头，客户端网关凭证、Cookie 与 Host 不能交给上游。
    let allowed: &[&str] = match protocol {
        WireProtocol::Anthropic => &["anthropic-version", "anthropic-beta"],
        WireProtocol::Responses => &["openai-beta"],
        WireProtocol::Chat => &[],
    };
    for name in allowed {
        let values: Vec<_> = client_headers
            .get_all(*name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();
        if !values.is_empty() {
            headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
            headers.push((name.to_string(), values.join(",")));
        }
    }
    let api_key = api_key.trim();
    if !api_key.is_empty() {
        headers.push(if protocol == WireProtocol::Anthropic {
            ("x-api-key".into(), api_key.to_string())
        } else {
            ("Authorization".into(), format!("Bearer {api_key}"))
        });
    }
    quirks.apply_emulation_headers(&mut headers, api_key, &outbound, base_url);
    quirks.apply_to(&mut headers);
    apply_user_agent_override(&mut headers, user_agent);
    let level = match protocol {
        WireProtocol::Anthropic => anthropic_thinking::effort(&outbound),
        WireProtocol::Responses => outbound
            .pointer("/reasoning/effort")
            .and_then(Value::as_str)
            .map(str::to_string),
        WireProtocol::Chat => model_rules::read_client_level(&outbound),
    };
    telemetry.note_upstream_model(&wire_model);
    telemetry.note_upstream_reasoning(level);
    let payload = serde_json::to_string(&outbound)
        .map_err(|error| GatewayError::new(format!("请求体序列化失败：{error}")))?;
    telemetry.note_attempt_body_bytes(payload.len().min(i64::MAX as usize) as i64);
    telemetry.note_phase(LogPhase::Waiting);
    if let Some(capture) = telemetry.capture() {
        capture.reset_request(&url, provider_id, &headers, &outbound);
    }
    logging::verbose("[CustomProvider]", &format!(
        "POST {url} protocol={} 原生转发 model={wire_model} stream={stream} account={account_id} 出口={}",
        protocol.as_str(), describe_proxy(proxy.as_ref()),
    ));
    let transport = TransportRequest {
        url,
        headers,
        payload,
        proxy,
        system_proxy_when_unset: false,
    };
    let send = send_chat_request(&transport);
    let response = if let Some(cancel) = telemetry.cancel_token() {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancellation::cancelled_error()),
            result = send => result,
        }
    } else {
        send.await
    }
    .map_err(|error| error.to_gateway_error())?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let headers = native::response_headers(response.headers());
        let retry = headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let capture = telemetry.capture();
        let budget =
            std::time::Duration::from_millis(crate::server::config::timeout_settings().body_ms());
        let read = tokio::time::timeout(budget, read_upstream_error(response, capture.as_deref()));
        let detail = if let Some(cancel) = telemetry.cancel_token() {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(cancellation::cancelled_error()),
                result = read => result,
            }
        } else {
            read.await
        }
        .map_err(|_| GatewayError::with_status(502, "读取上游错误响应超时"))?;
        let mut error = classify_custom_error(status, &detail.message, retry.as_deref());
        if let Some(body) = detail.payload {
            error.native_response = Some((protocol, body, headers));
        }
        return Err(error);
    }
    native::receive(
        response,
        protocol,
        route_model.unwrap_or(requested),
        stream,
        telemetry.clone(),
        slot,
        connections,
    )
    .await
    .map(ForwardOutcome::Native)
}
