//! CatPaw跨协议先完整核原Chat输出，再核完整转换正文；未确认就不发成功终态。

use std::sync::Arc;

use axum::{body::Bytes, response::Response};
use futures::StreamExt;
use serde_json::{json, Value};

use super::{failure, OutputGuard};
use crate::server::core::{protocol::{anthropic, responses}, upstream::{ForwardOutcome, usage::RequestTelemetry}};
use crate::server::errors::GatewayError;

pub(in crate::server::api) async fn converted_outcome(
    outcome: Result<ForwardOutcome, GatewayError>, telemetry: &Arc<RequestTelemetry>, request: &Value,
    model: &str, stream: bool, as_responses: bool, collapse: bool,
) -> Result<Result<ForwardOutcome, GatewayError>, Response> {
    if outcome.is_err() { return Ok(outcome); }
    let mut guard = match telemetry.take_catpaw_output() {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(outcome),
        Err(_) => return Err(failure(telemetry)),
    };
    let result = prepare(outcome, &mut guard, request, model, stream, as_responses, collapse).await;
    match result {
        Ok(response) => {
            if guard.finish().is_err() { return Err(failure(telemetry)); }
            Err(response)
        }
        Err(_) => { guard.fail(); Err(failure(telemetry)) },
    }
}

async fn prepare(
    outcome: Result<ForwardOutcome, GatewayError>, guard: &mut OutputGuard, request: &Value,
    model: &str, stream: bool, as_responses: bool, collapse: bool,
) -> Result<Response, ()> {
    match outcome.map_err(|_| ())? {
        ForwardOutcome::Completion { body } => {
            let text = serde_json::to_vec(&body).map_err(|_| ())?;
            guard.observe_json(&text).map_err(|_| ())?;
        }
        ForwardOutcome::Stream { status, mut stream } => {
            if !(200..300).contains(&status) { return Err(()); }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
            while let Some(chunk) = tokio::time::timeout_at(deadline, stream.next()).await.map_err(|_| ())? {
                guard.push(&chunk.map_err(|_| ())?).map_err(|_| ())?;
            }
        }
        ForwardOutcome::Native(_) => return Err(()),
    }
    let chat = guard.prepare_chat().map_err(|_| ())?;
    let mut body = if as_responses { responses::responses_from_chat(&chat, model, request, collapse) }
        else { anthropic::anthropic_from_chat(&chat, model) };
    if !as_responses && !chat.get("usage").is_some_and(Value::is_object) { body["usage"] = Value::Null; }
    guard.verify_converted(&body, request, model, as_responses).map_err(|_| ())?;
    if !stream { return Ok(super::super::json_response(body)); }
    let input = chat_frames(&chat)?;
    let frames = if as_responses {
        let mut converter = responses::ResponsesStream::new(model, request, collapse);
        let frames = converter.push(&input);
        let terminal = frames.iter().filter_map(|frame| event(frame)).find_map(|(name, value)|
            (name == "response.completed").then(|| value["response"].clone())).ok_or(())?;
        guard.verify_converted(&terminal, request, model, true).map_err(|_| ())?;
        frames
    } else {
        let mut converter = anthropic::AnthropicStream::new(model, None);
        let mut frames = converter.push(&input);
        if !chat.get("usage").is_some_and(Value::is_object) {
            for frame in &mut frames {
                if let Some((name, mut value)) = event(frame) {
                    if name == "message_start" { value["message"]["usage"] = Value::Null; }
                    if name == "message_delta" { value["usage"] = Value::Null; }
                    *frame = Bytes::from(format!("event: {name}\ndata: {}\n\n", value));
                }
            }
        }
        if !frames.iter().any(|frame| event(frame).is_some_and(|(name, _)| name == "message_stop")) { return Err(()); }
        frames
    };
    Ok(super::super::sse_response(axum::http::StatusCode::OK,
        Box::new(futures::stream::iter(frames.into_iter().map(Ok)))))
}

fn chat_frames(chat: &Value) -> Result<Vec<u8>, ()> {
    let message = &chat["choices"][0]["message"];
    let mut input = Vec::new();
    let mut append = |delta: Value, finish: Value, usage: Option<&Value>| -> Result<(), ()> {
        let choices = if usage.is_some() { json!([]) } else { json!([{ "index": 0, "delta": delta, "finish_reason": finish }]) };
        let mut frame = json!({"id": chat["id"], "object": "chat.completion.chunk", "created": chat["created"],
            "model": chat["model"], "choices": choices});
        if let Some(usage) = usage { frame["usage"] = usage.clone(); }
        input.extend_from_slice(format!("data: {}\n\n", frame).as_bytes());
        if input.len() > 16 * 1024 * 1024 { return Err(()); }
        Ok(())
    };
    append(json!({"role": "assistant"}), Value::Null, None)?;
    let mut delta = json!({});
    for name in ["content", "reasoning_content"] {
        if let Some(value) = message.get(name).filter(|value| value.as_str().is_some_and(|text| !text.is_empty())) {
            delta[name] = value.clone();
        }
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        delta["tool_calls"] = Value::Array(calls.iter().enumerate().map(|(index, call)| {
            let mut call = call.clone(); call["index"] = json!(index); call
        }).collect());
    }
    if delta.as_object().is_some_and(|fields| !fields.is_empty()) { append(delta, Value::Null, None)?; }
    append(json!({}), chat["choices"][0]["finish_reason"].clone(), None)?;
    if let Some(usage) = chat.get("usage").filter(|value| value.is_object()) { append(Value::Null, Value::Null, Some(usage))?; }
    input.extend_from_slice(b"data: [DONE]\n\n");
    Ok(input)
}

fn event(bytes: &Bytes) -> Option<(String, Value)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let name = text.lines().find_map(|line| line.strip_prefix("event: "))?.to_string();
    let data = text.lines().find_map(|line| line.strip_prefix("data: "))?;
    Some((name, serde_json::from_str(data).ok()?))
}
