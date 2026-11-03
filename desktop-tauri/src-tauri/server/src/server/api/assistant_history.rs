use std::sync::Arc;

use axum::{body::Bytes, extract::{Path, State}, http::HeaderMap, response::{IntoResponse, Response}};
use serde_json::{json, Value};

use crate::server::{config, core::{assistant_sessions, upstream::{ForwardOutcome, ForwardRequest, usage::RequestTelemetry}}, errors, http::{ok_json, parse_body}, logging, ServerState};
use super::disconnect_guard::DisconnectGuard;
use super::model_access::{self, ModelAccess};
use super::pipeline::{self, RecordContext};

pub async fn fork(State(_state): State<ServerState>, Path(id): Path<String>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let new_id = payload.get("id").and_then(Value::as_str).unwrap_or("").trim();
    let message_id = payload.get("messageId").and_then(Value::as_str).unwrap_or("").trim();
    match assistant_sessions::fork(&id, new_id, message_id) {
        Ok(session) => ok_json(json!({ "session": session })),
        Err(error) => errors::management_error(400, error),
    }
}

pub async fn title(State(state): State<ServerState>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let access = match model_access::authenticate(&headers, model_access::ASSISTANT_TITLE) {
        Ok(access) => access,
        Err(error) => return error.into_response(),
    };
    match summarize(&state, &id, access).await {
        Ok(session) => ok_json(json!({ "session": session })),
        Err(error) => error.into_response(),
    }
}

async fn summarize(state: &ServerState, id: &str, access: ModelAccess) -> Result<Value, errors::GatewayError> {
    let session = assistant_sessions::get(id).ok_or_else(|| errors::GatewayError::with_status(404, "会话不存在"))?;
    if session["titleGenerated"] == true {
        let mut session = session;
        session.as_object_mut().expect("session object").remove("messages");
        return Ok(session);
    }
    let messages = session["messages"].as_array().ok_or_else(|| errors::GatewayError::bad_request("会话消息无效"))?;
    let config = config::assistant_config();
    let model = config.title_model.as_deref().or_else(|| session["model"].as_str()).or(config.model.as_deref())
        .ok_or_else(|| errors::GatewayError::bad_request("还没有选择标题总结模型"))?.to_string();
    let mut excerpts = Vec::new();
    for message in messages {
        let role = message["role"].as_str().unwrap_or("");
        if role != "user" && role != "assistant" { continue; }
        let text = message["content"].as_str().unwrap_or("");
        if !text.trim().is_empty() { excerpts.push(json!({ "role": role, "text": text.chars().take(2000).collect::<String>() })); }
        if excerpts.len() == 4 { break; }
    }
    if !excerpts.iter().any(|item| item["role"] == "assistant") { return Err(errors::GatewayError::bad_request("请先完成一轮对话，再总结标题")); }
    let prompt = if config.title_prompt.trim().is_empty() { config::DEFAULT_TITLE_PROMPT } else { &config.title_prompt };
    let mut body = json!({ "model": model, "stream": false, "max_tokens": 120,
        "messages": [{ "role": "system", "content": prompt }, { "role": "user", "content": json!(excerpts).to_string() }] });
    let started = logging::now_ms();
    let requested = match super::request_prepare::resolve_model(state, &mut body, access.scope.as_ref()).await {
        Ok(requested) => requested,
        Err(error) => {
            pipeline::record_early_failure(state, started, &model, "", &access.source, &error);
            return Err(error);
        }
    };
    let telemetry = Arc::new(RequestTelemetry::with_id());
    let request_id = telemetry.id();
    super::request_prepare::record_started(state.request_stats(), &request_id, started, &requested, &model, "", &access.source, true).await;
    if let Some(token) = crate::server::core::upstream::cancellation::register(&request_id) {
        telemetry.set_cancel_token(token);
    }
    let mut guard = DisconnectGuard::new(state.request_stats(), request_id);
    let outcome = state.upstream().forward_assistant(ForwardRequest {
        body, stream: false, dedupe_key: String::new(), client_headers: HeaderMap::new(), telemetry: telemetry.clone(),
        allowed_providers: access.scope, pinned_account: None, ignore_model_gate: false, proxy_override: None,
    }).await;
    let mut context = RecordContext { stats: state.request_stats(), telemetry, started_at: started, model: requested,
        client_model: model, client_reasoning: String::new(), status: 200, raw_request: None, raw_response: None, is_test: true, source: access.source };
    guard.complete();
    let body = match outcome {
        Ok(ForwardOutcome::Completion { body }) => { pipeline::record_entry(&context, None); body }
        Ok(ForwardOutcome::Stream { .. }) => {
            context.status = 502;
            pipeline::record_entry(&context, Some("标题总结返回了意外的流式响应".into()));
            return Err(errors::GatewayError::with_status(502, "标题总结响应无效"));
        }
        Err(error) => {
            context.status = i64::from(error.http_status().as_u16());
            pipeline::record_entry(&context, Some(error.message.clone()));
            return Err(error);
        }
    };
    let title = body.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("").trim()
        .trim_matches(['"', '\'', '`', '“', '”']).lines().next().unwrap_or("").trim();
    if title.is_empty() { return Err(errors::GatewayError::with_status(502, "模型没有返回标题")); }
    assistant_sessions::set_title(id, &session["messages"], title).map_err(errors::GatewayError::new)
}
