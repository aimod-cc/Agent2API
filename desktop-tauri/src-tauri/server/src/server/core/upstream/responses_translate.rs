//! Responses 上游流复用协议转换器，终态前断开不能折成成功。

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};

use super::usage::RequestTelemetry;
use crate::server::core::protocol::responses_outbound::ChatFromResponsesStream;

pub struct ResponsesToChatStream {
    inner: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    machine: ChatFromResponsesStream,
    pending: VecDeque<Bytes>,
    ended: bool,
    capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
}

impl ResponsesToChatStream {
    pub fn new(
        response: reqwest::Response,
        model: &str,
        telemetry: &Arc<RequestTelemetry>,
    ) -> Self {
        let source = response.bytes_stream().map(|item| {
            item.map_err(|error| {
                std::io::Error::other(crate::server::core::egress::describe_error_detail(&error))
            })
        });
        Self {
            inner: super::stall::idle_guard(
                Box::pin(source),
                Duration::from_millis(crate::server::config::timeout_settings().stream_idle_ms()),
            ),
            machine: ChatFromResponsesStream::new_strict(model),
            pending: VecDeque::new(),
            ended: false,
            capture: telemetry.capture(),
        }
    }
}

impl Stream for ResponsesToChatStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(frame) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(frame)));
            }
            if self.ended {
                return Poll::Ready(None);
            }
            match self.inner.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Some(capture) = &self.capture {
                        capture.push(&bytes);
                    }
                    let frames = self.machine.push(&bytes);
                    self.pending.extend(frames);
                    self.ended = self.machine.is_finished();
                }
                Poll::Ready(Some(Err(error))) => {
                    self.ended = true;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    self.ended = true;
                    let frames = self.machine.finish();
                    self.pending.extend(frames);
                }
            }
        }
    }
}
