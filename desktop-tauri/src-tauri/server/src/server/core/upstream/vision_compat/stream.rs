use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;

use crate::server::errors::GatewayError;
use super::super::{usage::RequestTelemetry, ForwardOutcome};

pub(super) fn explicit_rejection(error: &GatewayError) -> bool {
    if !matches!(error.status_code, 400 | 406 | 415 | 422 | 500 | 502) { return false; }
    rejection_text(&error.message)
}

pub(super) fn rejection_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    if ["too large", "too many image", "image size", "image format", "invalid image", "download", "fetch image",
        "图片过大", "图片格式", "图片下载", "image dimensions", "image resolution"].iter().any(|marker| lower.contains(marker)) {
        return false;
    }
    ["001001020", "model is not multimodal", "model does not support image", "model doesn't support image",
        "model does not support vision", "model does not support multimodal", "image input is not supported for this model",
        "image inputs are not supported", "image_url is only supported by certain models",
        "this model only supports text", "不支持图片输入", "不支持图像输入", "模型不支持图片"].iter().any(|marker| lower.contains(marker))
}

fn frame_error(frame: &Value) -> Option<GatewayError> {
    if frame.get("type").and_then(Value::as_str) == Some("response.failed") {
        if let Some(response) = frame.get("response") { return frame_error(response); }
    }
    if let Some(code) = frame.get("error_code").and_then(Value::as_str) {
        if matches!(code.trim().to_ascii_lowercase().as_str(), "" | "0" | "0000" | "success") && frame.get("error").is_none() { return None; }
    }
    let value = frame.get("error").filter(|error| !error.is_null())
        .or_else(|| frame.get("error_msg").filter(|_| frame.get("error_code").is_some()))?;
    let message = value.as_str().map(str::to_string)
        .or_else(|| value.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| value.to_string());
    let code = frame.get("error_code").and_then(Value::as_str).unwrap_or("");
    Some(GatewayError::with_status(502, format!("{code} {message}").trim().to_string()))
}

fn has_output(frame: &Value) -> bool {
    if matches!(frame.get("type").and_then(Value::as_str), Some(
        "content_block_start" | "content_block_delta" | "response.output_item.added"
        | "response.output_text.delta" | "response.function_call_arguments.delta" | "response.reasoning_summary_text.delta"
        | "response.completed" | "response.incomplete" | "message_stop"
    )) { return true; }
    if frame.get("content").and_then(Value::as_array).is_some_and(|items| !items.is_empty())
        || frame.get("output").and_then(Value::as_array).is_some_and(|items| !items.is_empty())
    { return true; }
    frame.get("choices").and_then(Value::as_array).into_iter().flatten().any(|choice| {
        let delta = choice.get("delta").or_else(|| choice.get("message")).unwrap_or(choice);
        ["content", "reasoning_content", "reasoning", "tool_calls", "function_call", "text"].iter().any(|key|
            delta.get(*key).is_some_and(|value| match value {
                Value::String(text) => !text.is_empty(), Value::Array(items) => !items.is_empty(),
                Value::Object(items) => !items.is_empty(), _ => false,
            }))
    })
}

// 只读到首个正文、思考或工具调用；之后不能补救，避免重复回答或重复执行工具。
pub(super) async fn hold(outcome: ForwardOutcome, telemetry: &Arc<RequestTelemetry>) -> Result<ForwardOutcome, GatewayError> {
    let (status, stream, native) = match outcome {
        ForwardOutcome::Stream { status, stream } => (status, stream, None),
        ForwardOutcome::Native(response) => {
            let super::super::native::NativeResponse { protocol, status, headers, body } = response;
            match body {
                super::super::native::NativeBody::Stream(stream) => (status, stream, Some((protocol, headers))),
                super::super::native::NativeBody::Json(body) => {
                    if !has_output(&body) {
                        if let Some(error) = frame_error(&body) { return Err(error); }
                    }
                    return Ok(ForwardOutcome::Native(super::super::native::NativeResponse {
                        protocol, status, headers, body: super::super::native::NativeBody::Json(body),
                    }));
                }
            }
        }
        ForwardOutcome::Completion { body } => {
            if !has_output(&body) {
                if let Some(error) = frame_error(&body) { return Err(error); }
            }
            return Ok(ForwardOutcome::Completion { body });
        }
    };
    let mut stream = stream;
    let mut chunks = Vec::<Bytes>::new();
    let mut pending = Vec::new();
    let mut held = 0usize;
    let mut output = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(crate::server::config::timeout_settings().headers_ms());
    while held < 65_536 && !output {
        let next = tokio::time::timeout_at(deadline, stream.next());
        let item = if let Some(cancel) = telemetry.cancel_token() {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(super::super::cancellation::cancelled_error()),
                item = next => item,
            }
        } else { next.await };
        let item = item.map_err(|_| GatewayError::with_status(504, "等待图片请求首个有效响应超时"))?;
        let Some(item) = item else {
            if !output {
                return Err(GatewayError::with_status(502, "图片请求未返回可用的回答内容，已停止")
                    .with_code("vision_empty_response"));
            }
            break;
        };
        let chunk = item.map_err(|error| GatewayError::with_status(502, error.to_string()))?;
        held = held.saturating_add(chunk.len());
        pending.extend_from_slice(&chunk);
        chunks.push(chunk);
        let mut consumed = 0;
        for end in pending.iter().enumerate().filter_map(|(index, byte)| (*byte == b'\n').then_some(index)) {
            let line = &pending[consumed..end];
            consumed = end + 1;
            let Some(payload) = line.strip_prefix(b"data:") else { continue; };
            let Ok(frame) = serde_json::from_slice::<Value>(payload) else { continue; };
            if has_output(&frame) { output = true; break; }
            if let Some(error) = frame_error(&frame) { return Err(error); }
        }
        if consumed > 0 { pending.drain(..consumed); }
    }
    let stream = Box::new(futures::stream::iter(chunks.into_iter().map(Ok)).chain(stream));
    Ok(match native {
        Some((protocol, headers)) => ForwardOutcome::Native(super::super::native::NativeResponse {
            protocol, status, headers, body: super::super::native::NativeBody::Stream(stream),
        }),
        None => ForwardOutcome::Stream { status, stream },
    })
}

pub(in crate::server::core::upstream) fn rewrite(outcome: ForwardOutcome, requested: &str, service: &super::super::UpstreamService,
    telemetry: Arc<RequestTelemetry>) -> ForwardOutcome {
    match outcome {
        ForwardOutcome::Native(mut response) => {
            if let super::super::native::NativeBody::Json(body) = &mut response.body {
                crate::server::core::protocol::native::rewrite_response_model(response.protocol, body, requested);
            }
            ForwardOutcome::Native(response)
        }
        ForwardOutcome::Completion { mut body } => {
            if body.is_object() { body["model"] = requested.into(); }
            ForwardOutcome::Completion { body }
        }
        ForwardOutcome::Stream { status, stream } => ForwardOutcome::Stream {
            status,
            stream: Box::new(super::super::ForwardStream::from_translated(Box::pin(stream), None,
                super::super::connections::ConnectionGuard::new(service.connections.clone()), telemetry,
                super::super::sse::FramePolicy::with_rewrite(Some(super::super::sse::ModelRewrite { requested: requested.to_string() })))),
        },
    }
}
