use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, HttpBody};
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use futures::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::server::core::runtime_capacity::{self as capacity, Class, MemoryBudget, Ticket};
use crate::server::errors::GatewayError;

struct Admitted<S> {
    inner: S,
    _ticket: Ticket,
    _memory: MemoryBudget,
}

impl<S: Stream + Unpin> Stream for Admitted<S> {
    type Item = S::Item;
    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(context)
    }
}

pub async fn admit(mut request: Request, next: Next) -> Response {
    let class = match request.uri().path() {
        "/v1/chat/completions" | "/v1/responses" | "/v1/messages" => Class::Request,
        "/v1/images/generations" | "/v1/images/edits" => Class::ImageRequest,
        _ => return next.run(request).await,
    };
    let caller = request.extensions().get::<crate::server::request_stats::ApiKeyIdentity>()
        .map(|identity| identity.filter_value()).unwrap_or_else(|| "unknown".into());
    let limits = capacity::limits();
    let cancel = CancellationToken::new();
    let bytes = request.body().size_hint().upper().and_then(|bytes| usize::try_from(bytes).ok()).unwrap_or(64 * 1024);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(limits.queue_wait_seconds);
    let ticket = match capacity::acquire(class, "", &caller, bytes, deadline, &cancel).await {
        Ok(ticket) => ticket,
        Err(error) => return GatewayError::with_status(503, error.message).with_code("gateway_busy")
            .with_retry_after_seconds(capacity::retry_after_seconds()).payload_response(),
    };
    let memory = MemoryBudget::new();
    let budget = memory.clone();
    let original = std::mem::replace(request.body_mut(), Body::empty());
    let body = original.into_data_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        let reserved = chunk.len().saturating_mul(4);
        budget.grow(reserved).map_err(|error| std::io::Error::other(error.message))?;
        Ok::<_, std::io::Error>(chunk)
    });
    *request.body_mut() = Body::from_stream(body);
    let response = capacity::caller_scope(caller, next.run(request)).await;
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, Body::from_stream(Admitted { inner: body.into_data_stream(), _ticket: ticket, _memory: memory }))
}
