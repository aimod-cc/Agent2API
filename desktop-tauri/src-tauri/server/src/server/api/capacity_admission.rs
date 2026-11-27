use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, HttpBody};
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use futures::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use std::sync::Arc;
use crate::server::core::key_usage::KeyUsageLease;
use crate::server::core::runtime_capacity::{self as capacity, AcquireError, Class, MemoryBudget, Ticket};
use crate::server::errors::GatewayError;
use crate::server::logging;

struct Admitted<S> {
    inner: S,
    _ticket: Ticket,
    _memory: MemoryBudget,
    // 排在 inner 之后：确保流式收尾（RecordingStream::Drop）先落账，再释放并发占额。
    _usage_lease: Option<Arc<KeyUsageLease>>,
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
    let path = request.uri().path().to_string();
    let ticket = match capacity::acquire_detailed(class, "", &caller, bytes, deadline, &cancel).await {
        Ok(ticket) => ticket,
        Err(AcquireError::Cancelled) => {
            return GatewayError::with_status(499, "容量等待已取消").payload_response();
        }
        Err(AcquireError::Rejected(rejection)) => {
            logging::log_with_level(
                "[Server]",
                &format!(
                    "⚠️ 网关容量拦截 (503 gateway_busy): 路径={path} 调用方={caller} {}",
                    rejection.describe(),
                ),
                "warn",
            );
            return GatewayError::with_status(503, rejection.message())
                .with_code("gateway_busy")
                .with_reason(rejection.reason.code())
                .with_retry_after_seconds(rejection.retry_after_seconds())
                .payload_response();
        }
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
    let usage_lease = request.extensions_mut().remove::<Arc<KeyUsageLease>>();
    let response = capacity::caller_scope(caller, next.run(request)).await;
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        Body::from_stream(Admitted {
            inner: body.into_data_stream(),
            _ticket: ticket,
            _memory: memory,
            _usage_lease: usage_lease,
        }),
    )
}
