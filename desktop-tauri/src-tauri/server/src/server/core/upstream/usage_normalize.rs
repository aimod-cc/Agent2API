//! 在公共 Chat 出口补齐用量字段，覆盖普通上游与插件流；其他事件保留原始字节。

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use serde_json::Value;

use crate::server::core::protocol::usage::{normalize_chat, TokenUsage};

use super::usage::RequestTelemetry;
use super::ForwardOutcome;

type Source = Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send + Unpin>;

pub(super) fn normalize_outcome(outcome: ForwardOutcome, telemetry: Arc<RequestTelemetry>) -> ForwardOutcome {
    match outcome {
        ForwardOutcome::Native(response) => ForwardOutcome::Native(response),
        ForwardOutcome::Completion { mut body } => {
            if let Some(usage) = body.get_mut("usage").filter(|usage| usage.is_object()) {
                normalize_chat(usage);
                telemetry.report_usage(usage);
            }
            ForwardOutcome::Completion { body }
        }
        ForwardOutcome::Stream { status, stream } => ForwardOutcome::Stream {
            status,
            stream: Box::new(UsageStream {
                source: stream,
                telemetry,
                tokens: TokenUsage::default(),
                line: Vec::new(),
                pending: VecDeque::new(),
                passthrough_line: false,
                ended: false,
            }),
        },
    }
}

struct UsageStream {
    source: Source,
    telemetry: Arc<RequestTelemetry>,
    tokens: TokenUsage,
    line: Vec<u8>,
    pending: VecDeque<Result<Bytes, io::Error>>,
    passthrough_line: bool,
    ended: bool,
}

impl UsageStream {
    const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

    fn feed(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            let newline = rest.iter().position(|byte| *byte == b'\n');
            let end = newline.map_or(rest.len(), |index| index + 1);
            let part = &rest[..end];
            rest = &rest[end..];
            if self.passthrough_line {
                self.pending.push_back(Ok(Bytes::copy_from_slice(part)));
                self.passthrough_line = newline.is_none();
                continue;
            }
            if self.line.len().saturating_add(part.len()) > Self::MAX_LINE_BYTES {
                self.flush_raw();
                self.pending.push_back(Ok(Bytes::copy_from_slice(part)));
                self.passthrough_line = newline.is_none();
                continue;
            }
            self.line.extend_from_slice(part);
            if newline.is_some() {
                self.flush_line();
            }
        }
        // 同一来源分片的完整行一起下发，避免把原有 SSE 帧拆成多次交付。
        if self.pending.len() > 1 {
            let mut combined = Vec::new();
            while let Some(Ok(bytes)) = self.pending.pop_front() {
                combined.extend_from_slice(&bytes);
            }
            self.pending.push_back(Ok(Bytes::from(combined)));
        }
    }

    fn flush_raw(&mut self) {
        if !self.line.is_empty() {
            self.pending.push_back(Ok(Bytes::from(std::mem::take(&mut self.line))));
        }
    }

    fn flush_line(&mut self) {
        if self.line.is_empty() { return; }
        let line = std::mem::take(&mut self.line);
        let rewritten = self.rewrite_line(&line);
        self.pending.push_back(Ok(rewritten.unwrap_or_else(|| Bytes::from(line))));
    }

    fn rewrite_line(&mut self, line: &[u8]) -> Option<Bytes> {
        let payload = line.strip_prefix(b"data:")?;
        if !payload.windows(7).any(|window| window == b"\"usage\"") { return None; }
        let mut chunk: Value = serde_json::from_slice(payload).ok()?;
        let usage = chunk.get_mut("usage")?;
        self.tokens.absorb(TokenUsage::from_chat(usage)?);
        let changed = self.tokens.write_chat(usage);
        self.telemetry.report_usage(usage);
        if !changed { return None; }
        let text = serde_json::to_string(&chunk).ok()?;
        let ending = if line.ends_with(b"\r\n") { "\r\n" }
            else if line.ends_with(b"\n") { "\n" } else { "" };
        Some(Bytes::from(format!("data: {text}{ending}")))
    }
}

impl Stream for UsageStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        loop {
            if let Some(item) = this.pending.pop_front() {
                return Poll::Ready(Some(item));
            }
            if this.ended { return Poll::Ready(None); }
            match Pin::new(&mut this.source).poll_next(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(bytes))) => {
                    if this.line.is_empty() && !this.passthrough_line && bytes.ends_with(b"\n")
                        && !bytes.windows(7).any(|window| window == b"\"usage\"")
                    {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                    this.feed(&bytes);
                }
                Poll::Ready(Some(Err(error))) => {
                    // 传输中断时不能把残留半帧修成成功事件。
                    this.flush_raw();
                    this.pending.push_back(Err(error));
                    this.ended = true;
                }
                Poll::Ready(None) => {
                    this.flush_line();
                    this.ended = true;
                }
            }
        }
    }
}
