use axum::http::StatusCode;
use axum::response::Response;

use crate::server::core::protocol::native::WireProtocol;
use crate::server::core::upstream::native::{NativeBody, NativeResponse};
use crate::server::errors::GatewayError;

use super::super::disconnect_guard::DisconnectGuard;
use super::super::pipeline::{self, RecordContext, RecordingStream};

pub(in crate::server::api) fn respond(
    response: NativeResponse,
    context: RecordContext,
    guard: &mut DisconnectGuard,
) -> Response {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::OK);
    let mut context = RecordContext {
        status: i64::from(status.as_u16()),
        ..context
    };
    let mut result = match response.body {
        NativeBody::Stream(source) => {
            let terminals = match response.protocol {
                WireProtocol::Anthropic => pipeline::terminal::ANTHROPIC,
                WireProtocol::Responses => pipeline::terminal::RESPONSES,
                WireProtocol::Chat => pipeline::terminal::CHAT,
            };
            let stream = RecordingStream::with_terminals(source, context, terminals);
            guard.handoff();
            pipeline::sse_response(status, Box::new(stream))
        }
        NativeBody::Json(body) => {
            context.raw_response = pipeline::capture_raw_json(&body);
            pipeline::record_entry(&context, None);
            guard.complete();
            let mut result = pipeline::json_response(body);
            *result.status_mut() = status;
            result
        }
    };
    result.headers_mut().extend(response.headers);
    result
}

pub(super) fn error_response(
    protocol: WireProtocol,
    status: u16,
    error: &GatewayError,
) -> Option<Response> {
    let (source, body, headers) = error.native_response.as_ref()?;
    if *source != protocol {
        return None;
    }
    let mut response = pipeline::json_response(body.clone());
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    response.headers_mut().extend(headers.clone());
    Some(response)
}
