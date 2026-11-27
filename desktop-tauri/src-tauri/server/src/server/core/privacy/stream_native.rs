//! Anthropic 与 Responses 原生协议 SSE 流式隐私还原。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use serde_json::{json, Value};

use crate::server::core::protocol::native::WireProtocol;
use crate::server::core::upstream::usage::RequestTelemetry;

use super::sse_frame::{parse_sse_frame, rewrite_sse_frame, SseFrameSplitter};
use super::stream::{ByteStream, StreamChannelRestorer};
use super::vault::{sorted_counts, RequestVault, MAX_RESTORE_EXPANSION};
use super::walker::{
    restore_json_encoded_string, unmask_json_response_with_mode, unmask_responses_item,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NativeChannel {
    AnthropicText(usize),
    AnthropicJson(usize),
    ResponsesText(usize, usize),
    ResponsesRefusal(usize, usize),
    ResponsesArgs(usize),
    ResponsesMcpArgs(usize),
    ResponsesCustomInput(usize),
}

pub(super) struct PrivacyNativeSseStream {
    source: ByteStream,
    protocol: WireProtocol,
    telemetry: Arc<RequestTelemetry>,
    vault: RequestVault,
    json_output: bool,
    splitter: SseFrameSplitter,
    first_frame: bool,
    pending: VecDeque<Result<Bytes, io::Error>>,
    ended: bool,
    channels: BTreeMap<NativeChannel, StreamChannelRestorer>,
    stats: HashMap<String, usize>,
    stats_reported: bool,
}

impl PrivacyNativeSseStream {
    pub(super) fn new(
        source: ByteStream,
        protocol: WireProtocol,
        telemetry: Arc<RequestTelemetry>,
        vault: RequestVault,
        json_output: bool,
    ) -> Self {
        Self {
            source,
            protocol,
            telemetry,
            vault,
            json_output,
            splitter: SseFrameSplitter::default(),
            first_frame: true,
            pending: VecDeque::new(),
            ended: false,
            channels: BTreeMap::new(),
            stats: HashMap::new(),
            stats_reported: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        for frame in self.splitter.push(bytes) {
            self.process_frame(frame);
        }
        if let Some(overflow) = self.splitter.take_overflow() {
            self.pending.push_back(Ok(Bytes::from(overflow)));
        }
    }

    fn process_frame(&mut self, raw: Vec<u8>) {
        let parsed = parse_sse_frame(&raw, self.first_frame);
        self.first_frame = false;
        let Some(data_text) = parsed.data_text else {
            self.pending.push_back(Ok(Bytes::from(raw)));
            return;
        };
        let Ok(mut event) = serde_json::from_str::<Value>(&data_text) else {
            self.pending.push_back(Ok(Bytes::from(raw)));
            return;
        };
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(&parsed.event_name)
            .to_string();

        let modified = match self.protocol {
            WireProtocol::Anthropic => self.handle_anthropic(&kind, &mut event),
            WireProtocol::Responses => self.handle_responses(&kind, &mut event),
            WireProtocol::Chat => false,
        };

        if modified {
            if let Some(rewritten) = rewrite_sse_frame(&raw, parsed.leading_bom, &event) {
                self.pending.push_back(Ok(Bytes::from(rewritten)));
                return;
            }
        }
        self.pending.push_back(Ok(Bytes::from(raw)));
    }

    fn handle_anthropic(&mut self, kind: &str, event: &mut Value) -> bool {
        match kind {
            "content_block_delta" => {
                let idx = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let Some(delta) = event.get_mut("delta").and_then(Value::as_object_mut) else {
                    return false;
                };
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        if let Some(Value::String(text)) = delta.get_mut("text") {
                            let ch = self
                                .channels
                                .entry(NativeChannel::AnthropicText(idx))
                                .or_insert_with(|| StreamChannelRestorer::new(self.json_output));
                            let next = ch.push(text, &self.vault, &mut self.stats);
                            if next != *text {
                                *text = next;
                                return true;
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(Value::String(partial)) = delta.get_mut("partial_json") {
                            let ch = self
                                .channels
                                .entry(NativeChannel::AnthropicJson(idx))
                                .or_insert_with(|| StreamChannelRestorer::new(true));
                            let next = ch.push(partial, &self.vault, &mut self.stats);
                            if next != *partial {
                                *partial = next;
                                return true;
                            }
                        }
                    }
                    _ => {}
                }
                false
            }
            "content_block_stop" => {
                let idx = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                self.flush_native_channel(&NativeChannel::AnthropicText(idx));
                self.flush_native_channel(&NativeChannel::AnthropicJson(idx));
                false
            }
            "message_stop" => {
                self.flush_all_native_channels();
                self.report_stats();
                false
            }
            _ => false,
        }
    }

    fn handle_responses(&mut self, kind: &str, event: &mut Value) -> bool {
        let oidx = event
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let cidx = event
            .get("content_index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;

        match kind {
            "response.output_text.delta" => {
                if let Some(Value::String(text)) = event.get_mut("delta") {
                    let ch = self
                        .channels
                        .entry(NativeChannel::ResponsesText(oidx, cidx))
                        .or_insert_with(|| StreamChannelRestorer::new(self.json_output));
                    let next = ch.push(text, &self.vault, &mut self.stats);
                    if next != *text {
                        *text = next;
                        return true;
                    }
                }
                false
            }
            "response.output_text.done" => {
                self.flush_native_channel(&NativeChannel::ResponsesText(oidx, cidx));
                if let Some(Value::String(text)) = event.get_mut("text") {
                    if self.json_output {
                        let mut counts = HashMap::new();
                        let mut budget = MAX_RESTORE_EXPANSION;
                        if restore_json_encoded_string(text, &self.vault, &mut counts, &mut budget) {
                            self.absorb_fallback_stats(sorted_counts(counts));
                            return true;
                        }
                    } else if text.contains("{{") {
                        let (restored, hits) = self.vault.restore_text(text, false);
                        if !hits.is_empty() {
                            *text = restored;
                            self.absorb_fallback_stats(hits);
                            return true;
                        }
                    }
                }
                false
            }
            "response.refusal.delta" => {
                if let Some(Value::String(delta)) = event.get_mut("delta") {
                    let ch = self
                        .channels
                        .entry(NativeChannel::ResponsesRefusal(oidx, cidx))
                        .or_insert_with(|| StreamChannelRestorer::new(false));
                    let next = ch.push(delta, &self.vault, &mut self.stats);
                    if next != *delta {
                        *delta = next;
                        return true;
                    }
                }
                false
            }
            "response.refusal.done" => {
                self.flush_native_channel(&NativeChannel::ResponsesRefusal(oidx, cidx));
                if let Some(Value::String(refusal)) = event.get_mut("refusal") {
                    if refusal.contains("{{") {
                        let (restored, hits) = self.vault.restore_text(refusal, false);
                        if !hits.is_empty() {
                            *refusal = restored;
                            self.absorb_fallback_stats(hits);
                            return true;
                        }
                    }
                }
                false
            }
            "response.function_call_arguments.delta" => {
                if let Some(Value::String(delta)) = event.get_mut("delta") {
                    let ch = self
                        .channels
                        .entry(NativeChannel::ResponsesArgs(oidx))
                        .or_insert_with(|| StreamChannelRestorer::new(true));
                    let next = ch.push(delta, &self.vault, &mut self.stats);
                    if next != *delta {
                        *delta = next;
                        return true;
                    }
                }
                false
            }
            "response.function_call_arguments.done" => {
                self.flush_native_channel(&NativeChannel::ResponsesArgs(oidx));
                if let Some(Value::String(args)) = event.get_mut("arguments") {
                    let mut counts = HashMap::new();
                    let mut budget = MAX_RESTORE_EXPANSION;
                    if restore_json_encoded_string(args, &self.vault, &mut counts, &mut budget) {
                        self.absorb_fallback_stats(sorted_counts(counts));
                        return true;
                    }
                }
                false
            }
            "response.mcp_call_arguments.delta" => {
                if let Some(Value::String(delta)) = event.get_mut("delta") {
                    let ch = self
                        .channels
                        .entry(NativeChannel::ResponsesMcpArgs(oidx))
                        .or_insert_with(|| StreamChannelRestorer::new(true));
                    let next = ch.push(delta, &self.vault, &mut self.stats);
                    if next != *delta {
                        *delta = next;
                        return true;
                    }
                }
                false
            }
            "response.mcp_call_arguments.done" => {
                self.flush_native_channel(&NativeChannel::ResponsesMcpArgs(oidx));
                if let Some(Value::String(args)) = event.get_mut("arguments") {
                    let mut counts = HashMap::new();
                    let mut budget = MAX_RESTORE_EXPANSION;
                    if restore_json_encoded_string(args, &self.vault, &mut counts, &mut budget) {
                        self.absorb_fallback_stats(sorted_counts(counts));
                        return true;
                    }
                }
                false
            }
            "response.custom_tool_call_input.delta" => {
                if let Some(Value::String(delta)) = event.get_mut("delta") {
                    let ch = self
                        .channels
                        .entry(NativeChannel::ResponsesCustomInput(oidx))
                        .or_insert_with(|| StreamChannelRestorer::new(false));
                    let next = ch.push(delta, &self.vault, &mut self.stats);
                    if next != *delta {
                        *delta = next;
                        return true;
                    }
                }
                false
            }
            "response.custom_tool_call_input.done" => {
                self.flush_native_channel(&NativeChannel::ResponsesCustomInput(oidx));
                if let Some(Value::String(input)) = event.get_mut("input") {
                    if input.contains("{{") {
                        let (restored, hits) = self.vault.restore_text(input, false);
                        if !hits.is_empty() {
                            *input = restored;
                            self.absorb_fallback_stats(hits);
                            return true;
                        }
                    }
                }
                false
            }
            "response.output_item.done" | "response.output_item.added" => {
                if let Some(item) = event.get_mut("item") {
                    let mut counts = HashMap::new();
                    let mut budget = MAX_RESTORE_EXPANSION;
                    unmask_responses_item(
                        item,
                        &self.vault,
                        self.json_output,
                        &mut counts,
                        &mut budget,
                    );
                    if !counts.is_empty() {
                        self.absorb_fallback_stats(sorted_counts(counts));
                        return true;
                    }
                }
                false
            }
            "response.completed" | "response.incomplete" | "response.failed" | "response.done" => {
                self.flush_all_native_channels();
                let mut modified = false;
                if let Some(resp) = event.get_mut("response") {
                    let hits = unmask_json_response_with_mode(
                        resp,
                        WireProtocol::Responses,
                        &self.vault,
                        self.json_output,
                    );
                    if !hits.is_empty() {
                        self.absorb_fallback_stats(hits);
                        modified = true;
                    }
                }
                self.report_stats();
                modified
            }
            _ => false,
        }
    }

    fn absorb_fallback_stats(&mut self, hits: Vec<(String, usize)>) {
        if self.stats.is_empty() {
            for (k, v) in hits {
                *self.stats.entry(k).or_insert(0) += v;
            }
        }
    }

    fn flush_native_channel(&mut self, key: &NativeChannel) {
        let Some(ch) = self.channels.get_mut(key) else {
            return;
        };
        let tail = ch.flush();
        if tail.is_empty() {
            return;
        }
        let frame = match key {
            NativeChannel::AnthropicText(idx) => Some((
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": idx,
                    "delta": { "type": "text_delta", "text": tail },
                }),
            )),
            NativeChannel::AnthropicJson(idx) => Some((
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": idx,
                    "delta": { "type": "input_json_delta", "partial_json": tail },
                }),
            )),
            NativeChannel::ResponsesText(oidx, cidx) => Some((
                "response.output_text.delta",
                json!({
                    "type": "response.output_text.delta",
                    "output_index": oidx,
                    "content_index": cidx,
                    "delta": tail,
                }),
            )),
            NativeChannel::ResponsesRefusal(oidx, cidx) => Some((
                "response.refusal.delta",
                json!({
                    "type": "response.refusal.delta",
                    "output_index": oidx,
                    "content_index": cidx,
                    "delta": tail,
                }),
            )),
            NativeChannel::ResponsesArgs(oidx) => Some((
                "response.function_call_arguments.delta",
                json!({
                    "type": "response.function_call_arguments.delta",
                    "output_index": oidx,
                    "delta": tail,
                }),
            )),
            NativeChannel::ResponsesMcpArgs(oidx) => Some((
                "response.mcp_call_arguments.delta",
                json!({
                    "type": "response.mcp_call_arguments.delta",
                    "output_index": oidx,
                    "delta": tail,
                }),
            )),
            NativeChannel::ResponsesCustomInput(oidx) => Some((
                "response.custom_tool_call_input.delta",
                json!({
                    "type": "response.custom_tool_call_input.delta",
                    "output_index": oidx,
                    "delta": tail,
                }),
            )),
        };
        if let Some((ev, payload)) = frame {
            if let Ok(encoded) = serde_json::to_string(&payload) {
                self.pending
                    .push_back(Ok(Bytes::from(format!("event: {ev}\ndata: {encoded}\n\n"))));
            }
        }
    }

    fn flush_all_native_channels(&mut self) {
        let keys: Vec<NativeChannel> = self.channels.keys().cloned().collect();
        for key in keys {
            self.flush_native_channel(&key);
        }
    }

    fn report_stats(&mut self) {
        if !self.stats_reported && !self.stats.is_empty() {
            self.stats_reported = true;
            let list = sorted_counts(std::mem::take(&mut self.stats));
            self.telemetry.note_privacy_restored(&list);
        }
    }
}

impl Stream for PrivacyNativeSseStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        loop {
            if let Some(item) = this.pending.pop_front() {
                return Poll::Ready(Some(item));
            }
            if this.ended {
                return Poll::Ready(None);
            }
            match Pin::new(&mut this.source).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(bytes))) => {
                    this.feed(&bytes);
                }
                Poll::Ready(Some(Err(err))) => {
                    for tail in this.splitter.finish() {
                        this.pending.push_back(Ok(Bytes::from(tail)));
                    }
                    this.flush_all_native_channels();
                    this.report_stats();
                    this.pending.push_back(Err(err));
                    this.ended = true;
                }
                Poll::Ready(None) => {
                    for tail in this.splitter.finish() {
                        this.process_frame(tail);
                    }
                    this.flush_all_native_channels();
                    this.report_stats();
                    this.ended = true;
                }
            }
        }
    }
}

impl Drop for PrivacyNativeSseStream {
    fn drop(&mut self) {
        self.report_stats();
    }
}
