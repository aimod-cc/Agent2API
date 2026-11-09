use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use futures::StreamExt;

use super::{collect_native, NativeBody, NativeResponse, NativeStream, Observer, ResponseStream};
use crate::server::core::protocol::native::{rewrite_response_model, WireProtocol};
use crate::server::core::upstream::{cancellation, connections::{ConnectionGuard, Connections}, stall, usage::RequestTelemetry};
use crate::server::errors::GatewayError;

pub(crate) async fn receive_plugin(source: ResponseStream, protocol: WireProtocol, status: u16, headers: HeaderMap,
    model: &str, stream: bool, telemetry: Arc<RequestTelemetry>) -> Result<NativeResponse, GatewayError> {
    let source = stall::idle_guard(source.boxed(), Duration::from_millis(crate::server::config::timeout_settings().stream_idle_ms()));
    let source = cancellation::cancellable(source, telemetry.cancel_token());
    let body = if stream {
        NativeBody::Stream(Box::new(NativeStream {
            source, protocol, requested: model.to_string(), telemetry, capture: None,
            observer: Observer::default(), buffer: Vec::new(), scan_from: 0, line_start: 0,
            pending: VecDeque::new(), ended: false, terminal: false, sequence: 0, saw_message: false,
            _slot: None, _connection: ConnectionGuard::new(Connections::new()),
        }))
    } else {
        let mut body = tokio::time::timeout(Duration::from_millis(crate::server::config::timeout_settings().body_ms()),
            collect_native(source, protocol, &telemetry, None)).await
            .map_err(|_| GatewayError::with_status(504, "原生插件响应等待超时"))??;
        rewrite_response_model(protocol, &mut body, model);
        NativeBody::Json(body)
    };
    Ok(NativeResponse { protocol, status, headers, body })
}
