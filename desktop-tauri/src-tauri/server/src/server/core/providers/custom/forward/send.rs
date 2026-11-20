//! Cancellation covers credential/request preparation and the pending HTTP send.
use crate::server::core::upstream::{cancellation, request::{send_chat_request, TransportRequest}, usage::RequestTelemetry};
use crate::server::errors::GatewayError;

pub(super) async fn send_or_cancel(transport: &TransportRequest, telemetry: &RequestTelemetry,
    ordinary_inference: bool) -> Result<reqwest::Response, GatewayError> {
    if telemetry.is_cancelled() { return Err(cancellation::cancelled_error()); }
    let send = send_chat_request(transport);
    let result = if let Some(cancel) = telemetry.cancel_token() {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancellation::cancelled_error()),
            result = send => result,
        }
    } else { send.await };
    result.map_err(|error| {
        let mut gateway = error.to_gateway_error();
        gateway.unconfirmed_inference = !error.safe_to_retry && ordinary_inference;
        gateway
    })
}

#[cfg(test)]
mod tests;
