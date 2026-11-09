use std::sync::Arc;

use crate::server::core::protocol::{anthropic_outbound, responses_outbound};
use crate::server::core::upstream::usage::RequestTelemetry;

use super::OutboundKind;

pub(super) struct ProtocolTranslateStream {
    inner: futures::stream::BoxStream<'static, Result<bytes::Bytes, std::io::Error>>,
    machine: TranslateMachine,
    pending: std::collections::VecDeque<bytes::Bytes>,
    upstream_done: bool,
    capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
}

enum TranslateMachine {
    Responses(responses_outbound::ChatFromResponsesStream),
    Anthropic(anthropic_outbound::ChatFromAnthropicStream),
}

impl TranslateMachine {
    fn push(&mut self, chunk: &[u8]) -> Vec<bytes::Bytes> {
        match self {
            Self::Responses(machine) => machine.push(chunk),
            Self::Anthropic(machine) => machine.push(chunk),
        }
    }

    fn finish(&mut self) -> Vec<bytes::Bytes> {
        match self {
            Self::Responses(machine) => machine.finish(),
            Self::Anthropic(machine) => machine.finish(),
        }
    }
}

impl ProtocolTranslateStream {
    pub(super) fn new(
        kind: OutboundKind,
        model: &str,
        response: reqwest::Response,
        telemetry: &Arc<RequestTelemetry>,
    ) -> Self {
        use futures::StreamExt;
        let capture = telemetry.capture();
        if let Some(capture) = &capture {
            capture.attach_response(response.status().as_u16(), response.headers());
        }
        let described = response.bytes_stream().map(|item| {
            item.map_err(|error| {
                std::io::Error::other(crate::server::core::egress::describe_error_detail(&error))
            })
        });
        let guarded = crate::server::core::upstream::stall::idle_guard(
            Box::pin(described),
            std::time::Duration::from_millis(
                crate::server::config::timeout_settings().stream_idle_ms(),
            ),
        );
        Self {
            inner: guarded,
            machine: match kind {
                OutboundKind::Responses => TranslateMachine::Responses(
                    responses_outbound::ChatFromResponsesStream::new(model),
                ),
                OutboundKind::Anthropic => TranslateMachine::Anthropic(
                    anthropic_outbound::ChatFromAnthropicStream::new(model),
                ),
            },
            pending: std::collections::VecDeque::new(),
            upstream_done: false,
            capture,
        }
    }
}

impl futures::Stream for ProtocolTranslateStream {
    type Item = Result<bytes::Bytes, std::io::Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use futures::StreamExt;
        loop {
            if let Some(frame) = self.pending.pop_front() {
                return std::task::Poll::Ready(Some(Ok(frame)));
            }
            if self.upstream_done {
                return std::task::Poll::Ready(None);
            }
            match self.inner.poll_next_unpin(cx) {
                std::task::Poll::Pending => return std::task::Poll::Pending,
                std::task::Poll::Ready(None) => {
                    self.upstream_done = true;
                    for frame in self.machine.finish() {
                        self.pending.push_back(frame);
                    }
                }
                std::task::Poll::Ready(Some(Ok(bytes))) => {
                    // 采集上游原始字节，不把生成的 Chat 帧混入调试报文。
                    if let Some(capture) = &self.capture {
                        capture.push(&bytes);
                    }
                    for frame in self.machine.push(&bytes) {
                        self.pending.push_back(frame);
                    }
                }
                std::task::Poll::Ready(Some(Err(error))) => {
                    self.upstream_done = true;
                    return std::task::Poll::Ready(Some(Err(error)));
                }
            }
        }
    }
}
