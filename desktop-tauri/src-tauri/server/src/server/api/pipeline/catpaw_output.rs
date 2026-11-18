//! 最终Chat字节的只读观察；正常取帧只表示交给HTTP框架，不是写完或客户端接收。

use std::{collections::VecDeque, io, pin::Pin, sync::Arc, task::{Context, Poll}};

use axum::body::{Body, Bytes};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use futures::Stream;
use serde_json::Value;

use crate::server::core::plugin_host::wasm::CatPawOutputGuard as OutputGuard;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::errors::GatewayError;

mod converted;
pub(in crate::server::api) use converted::converted_outcome;

type FrameStream = Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send + Unpin>;

struct ObservedStream {
    source: FrameStream,
    guard: Option<OutputGuard>,
    pending: VecDeque<Bytes>,
    pending_bytes: usize,
    reading_done: bool,
    ended: bool,
}

impl Stream for ObservedStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.ended { return Poll::Ready(None); }
        loop {
            if self.reading_done {
                if let Some(bytes) = self.pending.pop_front() {
                    self.pending_bytes = self.pending_bytes.saturating_sub(bytes.len());
                    return Poll::Ready(Some(Ok(bytes)));
                }
                    self.ended = true;
                    return Poll::Ready(None);
            }
            match Pin::new(&mut self.source).poll_next(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(bytes))) => {
                    if self.guard.as_mut().is_some_and(|guard| guard.push(&bytes).is_err()) {
                        if let Some(mut guard) = self.guard.take() { guard.fail(); }
                        self.pending.clear();
                        self.ended = true;
                        return Poll::Ready(Some(Err(invalid())));
                    }
                    if self.guard.as_ref().is_some_and(OutputGuard::terminal_seen) || !self.pending.is_empty() {
                        let Some(total) = self.pending_bytes.checked_add(bytes.len()).filter(|total| *total <= 2 * 1024 * 1024) else {
                            if let Some(mut guard) = self.guard.take() { guard.fail(); }
                            self.pending.clear();
                            self.ended = true;
                            return Poll::Ready(Some(Err(invalid())));
                        };
                        self.pending_bytes = total;
                        self.pending.push_back(bytes);
                        continue;
                    }
                    return Poll::Ready(Some(Ok(bytes)));
                }
                Poll::Ready(Some(Err(error))) => {
                    if let Some(mut guard) = self.guard.take() { guard.fail(); }
                    self.pending.clear();
                    self.ended = true;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    if self.guard.as_mut().is_none_or(|guard| guard.prepare().is_err()) {
                        if let Some(mut guard) = self.guard.take() { guard.fail(); }
                        self.pending.clear();
                        self.ended = true;
                        return Poll::Ready(Some(Err(invalid())));
                    }
                    if self.guard.take().is_none_or(|guard| guard.finish().is_err()) {
                        self.pending.clear();
                        self.ended = true;
                        return Poll::Ready(Some(Err(invalid())));
                    }
                    self.reading_done = true;
                }
            }
        }
    }
}

pub(in crate::server::api) fn observe_stream(source: FrameStream, telemetry: &Arc<RequestTelemetry>) -> FrameStream {
    match telemetry.take_catpaw_output() {
        Ok(Some(guard)) => Box::new(ObservedStream {
            source, guard: Some(guard), pending: VecDeque::new(), pending_bytes: 0, reading_done: false, ended: false,
        }),
        Ok(None) => source,
        Err(_) => Box::new(futures::stream::once(std::future::ready(Err(invalid())))),
    }
}

pub(in crate::server::api) fn json_response(body: Value, telemetry: &Arc<RequestTelemetry>) -> Response {
    let mut guard = match telemetry.take_catpaw_output() {
        Ok(Some(guard)) => guard,
        Ok(None) => return super::json_response(body),
        Err(_) => return failure(telemetry),
    };
    let text = match serde_json::to_string(&body) {
        Ok(text) => text,
        Err(_) => { guard.fail(); return failure(telemetry); }
    };
    if guard.observe_json(text.as_bytes()).is_err() || guard.prepare().is_err() {
        guard.fail();
        return failure(telemetry);
    }
    if guard.finish().is_err() { return failure(telemetry); }
    let source = futures::stream::iter([Ok(Bytes::from(text))]);
    let mut response = Response::new(Body::from_stream(JsonStream {
        source: Box::new(source), guard: None, ended: false,
    }));
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
    response
}

struct JsonStream {
    source: FrameStream,
    guard: Option<OutputGuard>,
    ended: bool,
}

impl Stream for JsonStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.ended { return Poll::Ready(None); }
        match Pin::new(&mut self.source).poll_next(context) {
            Poll::Ready(None) => {
                self.ended = true;
                if self.guard.take().is_some_and(|guard| guard.finish().is_err()) {
                    Poll::Ready(Some(Err(invalid())))
                } else { Poll::Ready(None) }
            }
            result => result,
        }
    }
}

pub(in crate::server::api) fn unsupported_response(telemetry: &Arc<RequestTelemetry>) -> Option<Response> {
    match telemetry.take_catpaw_output() {
        Ok(None) => None,
        Ok(Some(mut guard)) => { guard.fail(); Some(failure(telemetry)) }
        Err(_) => Some(failure(telemetry)),
    }
}

fn invalid() -> io::Error { io::Error::other("catpaw_output_unconfirmed") }

fn failure(telemetry: &RequestTelemetry) -> Response {
    telemetry.note_error("CatPaw 最终工具输出核对未确认，未开放交接或续接");
    GatewayError::with_status(i32::from(StatusCode::BAD_GATEWAY.as_u16()),
        "CatPaw 最终工具输出未完整核对，未把输出准备当作交付成功或开放续接")
        .with_code("catpaw_output_unconfirmed").payload_response()
}
