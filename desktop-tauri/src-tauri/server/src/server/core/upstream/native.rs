//! 同协议的原生响应，不进入 Chat 帧转换与用量改写链。

mod collect;
mod plugin;
pub(crate) use plugin::receive_plugin;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};

use crate::server::core::protocol::native::{error_frame, rewrite_response_model, WireProtocol};
use crate::server::core::protocol::usage::{token_count, TokenUsage};
use crate::server::errors::GatewayError;

use super::connections::ConnectionGuard;
use super::usage::RequestTelemetry;
use super::{cancellation, stall, InFlightGuard};

pub type ResponseStream = Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send + Unpin>;
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

pub struct NativeResponse {
    pub protocol: WireProtocol,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: NativeBody,
}

pub enum NativeBody {
    Stream(ResponseStream),
    Json(Value),
}

pub fn response_headers(headers: &HeaderMap) -> HeaderMap {
    let mut kept = HeaderMap::new();
    for (name, value) in headers {
        let key = name.as_str();
        if matches!(
            key,
            "request-id" | "x-request-id" | "retry-after" | "openai-processing-ms"
        ) || key.starts_with("anthropic-ratelimit-")
            || key.starts_with("x-ratelimit-")
        {
            kept.append(name.clone(), value.clone());
        }
    }
    kept
}

pub(crate) async fn receive(
    response: reqwest::Response,
    protocol: WireProtocol,
    requested: &str,
    stream: bool,
    telemetry: Arc<RequestTelemetry>,
    slot: &mut Option<InFlightGuard>,
    connections: &mut ConnectionGuard,
) -> Result<NativeResponse, GatewayError> {
    let status = response.status().as_u16();
    let headers = response_headers(response.headers());
    let event_stream = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    let capture = telemetry.capture();
    if let Some(capture) = &capture {
        capture.attach_response(status, response.headers());
    }
    let source = response.bytes_stream().map(|item| {
        item.map_err(|error| {
            io::Error::other(crate::server::core::egress::describe_error_detail(&error))
        })
    });
    let source = stall::idle_guard(
        Box::pin(source),
        Duration::from_millis(crate::server::config::timeout_settings().stream_idle_ms()),
    );
    let source = cancellation::cancellable(source, telemetry.cancel_token());
    if event_stream && stream {
        let body = NativeBody::Stream(Box::new(NativeStream {
            source,
            protocol,
            requested: requested.to_string(),
            telemetry,
            capture,
            observer: Observer::default(),
            buffer: Vec::new(),
            scan_from: 0,
            line_start: 0,
            pending: VecDeque::new(),
            ended: false,
            terminal: false,
            sequence: 0,
            saw_message: false,
            _slot: slot.take(),
            _connection: connections.handoff(),
        }));
        return Ok(NativeResponse {
            protocol,
            status,
            headers,
            body,
        });
    }
    if event_stream {
        let budget = Duration::from_millis(crate::server::config::timeout_settings().body_ms());
        let mut body = tokio::time::timeout(
            budget,
            collect_native(source, protocol, &telemetry, capture.as_deref()),
        )
        .await
        .map_err(|_| {
            GatewayError::with_status(502, format!("非流式响应超时({}秒)", budget.as_secs()))
        })??;
        rewrite_response_model(protocol, &mut body, requested);
        return Ok(NativeResponse {
            protocol,
            status,
            headers,
            body: NativeBody::Json(body),
        });
    }
    let budget = Duration::from_millis(crate::server::config::timeout_settings().body_ms());
    let read = async {
        let mut source = source;
        let mut bytes = Vec::new();
        while let Some(item) = source.next().await {
            let chunk = item.map_err(stream_error)?;
            if !chunk.is_empty() {
                telemetry.note_first_frame();
            }
            if let Some(capture) = &capture {
                capture.push(&chunk);
            }
            if bytes.len().saturating_add(chunk.len()) > 64 * 1024 * 1024 {
                return Err(GatewayError::with_status(
                    502,
                    "上游原生响应超过 64 MiB，已停止接收",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice::<Value>(&bytes).map_err(|error| {
            GatewayError::with_status(502, format!("上游原生响应不是有效 JSON：{error}"))
        })
    };
    let mut body = tokio::time::timeout(budget, read).await.map_err(|_| {
        GatewayError::with_status(502, format!("非流式响应超时({}秒)", budget.as_secs()))
    })??;
    let mut observer = Observer::default();
    observer.observe(protocol, &body, &telemetry);
    rewrite_response_model(protocol, &mut body, requested);
    Ok(NativeResponse {
        protocol,
        status,
        headers,
        body: NativeBody::Json(body),
    })
}

async fn collect_native(
    mut source: futures::stream::BoxStream<'static, Result<Bytes, io::Error>>,
    protocol: WireProtocol,
    telemetry: &RequestTelemetry,
    capture: Option<&crate::server::core::debug_traffic::TrafficCapture>,
) -> Result<Value, GatewayError> {
    let mut collector = collect::Collector::new(protocol);
    let mut observer = Observer::default();
    let mut buffer = Vec::new();
    while let Some(item) = source.next().await {
        let bytes = item.map_err(stream_error)?;
        if !bytes.is_empty() {
            telemetry.note_first_frame();
        }
        if let Some(capture) = capture {
            capture.push(&bytes);
        }
        buffer.extend_from_slice(&bytes);
        while let Some(end) = frame_end(&buffer) {
            if end > MAX_EVENT_BYTES {
                return Err(GatewayError::with_status(
                    502,
                    "上游原生事件超过 8 MiB，已停止接收",
                ));
            }
            let frame: Vec<_> = buffer.drain(..end).collect();
            if let Some(event) = event_json(&frame)? {
                observer.observe(protocol, &event, telemetry);
                collector.push(&event, frame.len())?;
                if collector.is_done() {
                    return collector.finish();
                }
            }
        }
        if buffer.len() > MAX_EVENT_BYTES {
            return Err(GatewayError::with_status(
                502,
                "上游原生事件超过 8 MiB，已停止接收",
            ));
        }
    }
    if !buffer.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "上游原生响应在事件完整到达前断开",
        ));
    }
    collector.finish()
}

fn frame_end(buffer: &[u8]) -> Option<usize> {
    let mut start = 0;
    for (index, byte) in buffer.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let line = &buffer[start..index];
        if line.is_empty() || line == b"\r" {
            return Some(index + 1);
        }
        start = index + 1;
    }
    None
}

fn event_json(bytes: &[u8]) -> Result<Option<Value>, GatewayError> {
    let mut data = Vec::new();
    let mut has_data = false;
    for line in bytes.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(value) = line.strip_prefix(b"data:") {
            if has_data {
                data.push(b'\n');
            }
            has_data = true;
            data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
        }
    }
    if !has_data || data.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let value = serde_json::from_slice::<Value>(&data)
        .map_err(|_| GatewayError::with_status(502, "上游原生事件不是有效 JSON，已停止接收"))?;
    if !value.is_object() {
        return Err(GatewayError::with_status(
            502,
            "上游原生事件必须是 JSON 对象，已停止接收",
        ));
    }
    Ok(Some(value))
}

fn stream_error(error: io::Error) -> GatewayError {
    if error.to_string() == cancellation::MANUAL_TERMINATED {
        cancellation::cancelled_error()
    } else {
        GatewayError::with_status(502, error.to_string())
    }
}

#[derive(Default)]
struct Observer {
    tokens: TokenUsage,
    credit: Option<Value>,
}

impl Observer {
    fn observe(&mut self, protocol: WireProtocol, event: &Value, telemetry: &RequestTelemetry) {
        let usage = match protocol {
            WireProtocol::Anthropic => event
                .pointer("/message/usage")
                .or_else(|| event.get("usage")),
            WireProtocol::Responses => event
                .pointer("/response/usage")
                .or_else(|| event.get("usage")),
            WireProtocol::Chat => event.get("usage"),
        };
        if let Some(usage) = usage {
            if let Some(mut next) = TokenUsage::from_chat(usage) {
                if protocol == WireProtocol::Anthropic {
                    next.prompt = token_count(usage.get("input_tokens"));
                    next.total = None;
                }
                self.tokens.absorb(next);
                if let Some(credit) = usage.as_object().and_then(super::usage::credit_from_object) {
                    self.credit = Some(Value::from(credit));
                }
                let mut tokens = self.tokens;
                if protocol == WireProtocol::Anthropic {
                    tokens.prompt = Some(
                        tokens
                            .prompt
                            .unwrap_or(0)
                            .saturating_add(tokens.cache_read.unwrap_or(0))
                            .saturating_add(tokens.cache_creation.unwrap_or(0)),
                    );
                }
                let mut reported = json!({});
                tokens.write_chat(&mut reported);
                if let Some(credit) = &self.credit {
                    reported["credit"] = credit.clone();
                }
                telemetry.report_usage(&reported);
            }
        }
        let error = event
            .get("error")
            .filter(|value| !value.is_null())
            .or_else(|| {
                event
                    .pointer("/response/error")
                    .filter(|value| !value.is_null())
            });
        if let Some(error) = error {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("上游原生响应返回错误");
            telemetry.note_error(message);
        } else if event.get("type").and_then(Value::as_str) == Some("error") {
            telemetry.note_error(
                event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("上游流式返回错误"),
            );
        } else if event.get("type").and_then(Value::as_str) == Some("response.failed")
            || event.get("status").and_then(Value::as_str) == Some("failed")
        {
            telemetry.note_error("上游 Responses 请求失败");
        }
    }
}

struct NativeStream {
    source: futures::stream::BoxStream<'static, Result<Bytes, io::Error>>,
    protocol: WireProtocol,
    requested: String,
    telemetry: Arc<RequestTelemetry>,
    capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
    observer: Observer,
    buffer: Vec<u8>,
    scan_from: usize,
    line_start: usize,
    pending: VecDeque<Bytes>,
    ended: bool,
    terminal: bool,
    sequence: i64,
    saw_message: bool,
    _slot: Option<InFlightGuard>,
    _connection: ConnectionGuard,
}

impl NativeStream {
    fn frame(&mut self, bytes: Vec<u8>) {
        if self.ended {
            return;
        }
        if bytes.len() > MAX_EVENT_BYTES {
            self.fail(io::Error::other("上游原生事件超过 8 MiB，已停止接收"));
            return;
        }
        let mut event_name = String::new();
        for line in bytes.split(|byte| *byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if let Some(value) = line.strip_prefix(b"event:") {
                event_name = String::from_utf8_lossy(value).trim().to_string();
            }
        }
        let mut event = match event_json(&bytes) {
            Ok(Some(event)) => event,
            Ok(None) => {
                self.pending.push_back(Bytes::from(bytes));
                return;
            }
            Err(error) => {
                self.fail(io::Error::other(error.message));
                return;
            }
        };
        if let Some(sequence) = event.get("sequence_number").and_then(Value::as_i64) {
            self.sequence = self.sequence.max(sequence.saturating_add(1));
        }
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(&event_name)
            .to_string();
        self.saw_message |= match self.protocol {
            WireProtocol::Anthropic => {
                kind == "message_start" && event.get("message").is_some_and(Value::is_object)
            }
            WireProtocol::Responses => event.get("response").is_some_and(Value::is_object),
            WireProtocol::Chat => event.get("choices").is_some_and(Value::is_array),
        };
        let terminal = match self.protocol {
            WireProtocol::Anthropic => matches!(kind.as_str(), "message_stop" | "error"),
            WireProtocol::Responses => matches!(
                kind.as_str(),
                "response.completed" | "response.failed" | "response.incomplete" | "error"
            ),
            WireProtocol::Chat => kind == "error",
        };
        if terminal && kind != "error" && !self.saw_message {
            self.fail(io::Error::other(
                "上游原生响应没有有效消息，不能仅凭结束标记判定成功",
            ));
            return;
        }
        self.terminal |= terminal;
        self.ended |= terminal && (self.protocol != WireProtocol::Responses || kind != "error");
        self.observer
            .observe(self.protocol, &event, &self.telemetry);
        if !rewrite_response_model(self.protocol, &mut event, &self.requested) {
            if terminal && event_name.is_empty() {
                let mut framed = format!("event: {kind}\n").into_bytes();
                framed.extend_from_slice(&bytes);
                self.pending.push_back(Bytes::from(framed));
            } else {
                self.pending.push_back(Bytes::from(bytes));
            }
            return;
        }
        let Ok(encoded) = serde_json::to_vec(&event) else {
            self.pending.push_back(Bytes::from(bytes));
            return;
        };
        let mut out = Vec::with_capacity(bytes.len());
        if terminal && event_name.is_empty() {
            out.extend_from_slice(format!("event: {kind}\n").as_bytes());
        }
        let mut written = false;
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            if line.starts_with(b"data:") {
                if !written {
                    out.extend_from_slice(b"data: ");
                    out.extend_from_slice(&encoded);
                    if line.ends_with(b"\r\n") {
                        out.extend_from_slice(b"\r\n");
                    } else if line.ends_with(b"\n") {
                        out.push(b'\n');
                    }
                    written = true;
                }
            } else {
                out.extend_from_slice(line);
            }
        }
        self.pending.push_back(Bytes::from(out));
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
        self.buffer.extend_from_slice(bytes);
        let mut start = 0;
        let mut line_start = self.line_start;
        let mut frames = Vec::new();
        for (index, byte) in self.buffer.iter().enumerate().skip(self.scan_from) {
            if *byte != b'\n' {
                continue;
            }
            let line = &self.buffer[line_start..index];
            if line.is_empty() || line == b"\r" {
                if index + 1 - start > MAX_EVENT_BYTES {
                    return Err(io::Error::other("上游原生事件超过 8 MiB，已停止接收"));
                }
                frames.push(self.buffer[start..=index].to_vec());
                start = index + 1;
            }
            line_start = index + 1;
        }
        self.scan_from = self.buffer.len().saturating_sub(start);
        self.line_start = line_start.saturating_sub(start);
        if start > 0 {
            self.buffer.drain(..start);
        }
        for frame in frames {
            self.frame(frame);
        }
        if self.buffer.len() > MAX_EVENT_BYTES {
            return Err(io::Error::other("上游原生事件超过 8 MiB，已停止接收"));
        }
        Ok(())
    }

    fn fail(&mut self, error: io::Error) {
        self.ended = true;
        self.buffer.clear();
        let message = error.to_string();
        self.telemetry.note_error(&message);
        self.pending
            .push_back(error_frame(self.protocol, &message, self.sequence));
    }
}

impl Stream for NativeStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(bytes) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(bytes)));
            }
            if self.ended {
                return Poll::Ready(None);
            }
            match self.source.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Some(capture) = &self.capture {
                        capture.push(&bytes);
                    }
                    if !bytes.is_empty() {
                        self.telemetry.note_first_frame();
                    }
                    if let Err(error) = self.feed(&bytes) {
                        self.fail(error);
                    }
                }
                Poll::Ready(Some(Err(error))) => self.fail(error),
                Poll::Ready(None) => {
                    self.ended = true;
                    if !self.buffer.is_empty() {
                        self.buffer.clear();
                        self.telemetry
                            .note_error("上游原生响应在事件完整到达前断开");
                    }
                    if !self.terminal {
                        self.fail(io::Error::other("上游原生响应在结束事件到达前断开"));
                    }
                }
            }
        }
    }
}
