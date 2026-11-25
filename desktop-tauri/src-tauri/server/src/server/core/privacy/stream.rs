use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use serde_json::{json, Value};

use crate::server::core::protocol::native::WireProtocol;
use crate::server::core::upstream::native::{NativeBody, NativeResponse};
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::ForwardOutcome;

use super::vault::{
    normalize_placeholder_inner, push_json_escaped_inner, sorted_counts, PrivacyVault,
};
use super::walker::unmask_json_response;

const MAX_PLACEHOLDER_INNER_BYTES: usize = 48;
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

type ByteStream = Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send + Unpin>;

/// 流式单文本通道的有界前缀缓冲状态机：跨任意 chunk 切分还原 `{{KIND_xxxxxxxx}}`。
#[derive(Clone, Debug, Default)]
pub struct StreamChannelRestorer {
    buf: String,
    json_string_escape: bool,
}

impl StreamChannelRestorer {
    pub fn new(json_string_escape: bool) -> Self {
        Self {
            buf: String::new(),
            json_string_escape,
        }
    }

    pub fn push(
        &mut self,
        delta: &str,
        vault: &PrivacyVault,
        stats: &mut HashMap<String, usize>,
    ) -> String {
        if self.buf.is_empty() && !delta.contains('{') {
            return delta.to_string();
        }
        self.buf.push_str(delta);
        let mut out = String::with_capacity(self.buf.len());
        let mut cursor = 0usize;

        while cursor < self.buf.len() {
            let Some(rel) = self.buf[cursor..].find('{') else {
                out.push_str(&self.buf[cursor..]);
                cursor = self.buf.len();
                break;
            };
            let brace_pos = cursor + rel;
            out.push_str(&self.buf[cursor..brace_pos]);
            let tail = &self.buf[brace_pos..];
            if tail.len() == 1 {
                // 末尾单个 '{' 可能是 '{{' 的前半截，留待下一片决定。
                cursor = brace_pos;
                break;
            }
            if !tail.starts_with("{{") {
                out.push('{');
                cursor = brace_pos + 1;
                continue;
            }
            let after_open = &tail[2..];
            match scan_placeholder_after_open(after_open) {
                ScanState::Complete { inner_end } => {
                    let inner = &after_open[..inner_end];
                    let whole = &tail[..2 + inner_end + 2];
                    if normalize_placeholder_inner(inner).is_some() {
                        if let Some((original, kind)) = vault.lookup(inner) {
                            if self.json_string_escape {
                                push_json_escaped_inner(&mut out, &original);
                            } else {
                                out.push_str(&original);
                            }
                            *stats.entry(kind).or_insert(0) += 1;
                        } else {
                            out.push_str(whole);
                        }
                    } else {
                        out.push_str(whole);
                    }
                    cursor = brace_pos + 2 + inner_end + 2;
                }
                ScanState::NeedMore => {
                    cursor = brace_pos;
                    break;
                }
                ScanState::Invalid => {
                    out.push_str("{{");
                    cursor = brace_pos + 2;
                }
            }
        }

        if cursor >= self.buf.len() {
            self.buf.clear();
        } else if cursor > 0 {
            self.buf.drain(..cursor);
        }
        out
    }

    pub fn flush(&mut self) -> String {
        std::mem::take(&mut self.buf)
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

enum ScanState {
    Complete { inner_end: usize },
    NeedMore,
    Invalid,
}

fn scan_placeholder_after_open(after_open: &str) -> ScanState {
    let bytes = after_open.as_bytes();
    let limit = bytes.len().min(MAX_PLACEHOLDER_INNER_BYTES + 2);
    let mut idx = 0usize;
    while idx < limit {
        let b = bytes[idx];
        if b == b'}' {
            if idx + 1 >= bytes.len() {
                return ScanState::NeedMore;
            }
            if bytes[idx + 1] == b'}' {
                return ScanState::Complete { inner_end: idx };
            }
            return ScanState::Invalid;
        }
        if idx >= MAX_PLACEHOLDER_INNER_BYTES {
            return ScanState::Invalid;
        }
        if !(b.is_ascii_alphanumeric() || b == b'_' || b == b' ' || b == b'\t') {
            return ScanState::Invalid;
        }
        idx += 1;
    }
    if bytes.len() > MAX_PLACEHOLDER_INNER_BYTES {
        ScanState::Invalid
    } else {
        ScanState::NeedMore
    }
}

pub fn unmask_outcome(outcome: ForwardOutcome, telemetry: Arc<RequestTelemetry>) -> ForwardOutcome {
    let vault = PrivacyVault::global();
    match outcome {
        ForwardOutcome::Completion { mut body } => {
            let restored = unmask_json_response(&mut body, WireProtocol::Chat, vault);
            if !restored.is_empty() {
                telemetry.note_privacy_restored(&restored);
            }
            ForwardOutcome::Completion { body }
        }
        ForwardOutcome::Stream { status, stream } => ForwardOutcome::Stream {
            status,
            stream: Box::new(PrivacyChatSseStream::new(stream, telemetry)),
        },
        ForwardOutcome::Native(NativeResponse {
            protocol,
            status,
            headers,
            body,
        }) => {
            let body = match body {
                NativeBody::Json(mut json) => {
                    let restored = unmask_json_response(&mut json, protocol, vault);
                    if !restored.is_empty() {
                        telemetry.note_privacy_restored(&restored);
                    }
                    NativeBody::Json(json)
                }
                NativeBody::Stream(stream) => match protocol {
                    WireProtocol::Chat => {
                        NativeBody::Stream(Box::new(PrivacyChatSseStream::new(stream, telemetry)))
                    }
                    WireProtocol::Anthropic | WireProtocol::Responses => NativeBody::Stream(
                        Box::new(PrivacyNativeSseStream::new(stream, protocol, telemetry)),
                    ),
                },
            };
            NativeResponse {
                protocol,
                status,
                headers,
                body,
            }
            .into_outcome()
        }
    }
}

trait IntoOutcome {
    fn into_outcome(self) -> ForwardOutcome;
}

impl IntoOutcome for NativeResponse {
    fn into_outcome(self) -> ForwardOutcome {
        ForwardOutcome::Native(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ChatChannel {
    Content(usize),
    Reasoning(usize),
    ToolArgs(usize, usize),
}

struct PrivacyChatSseStream {
    source: ByteStream,
    telemetry: Arc<RequestTelemetry>,
    line: Vec<u8>,
    passthrough_line: bool,
    pending: VecDeque<Result<Bytes, io::Error>>,
    ended: bool,
    channels: BTreeMap<ChatChannel, StreamChannelRestorer>,
    meta_id: Option<Value>,
    meta_model: Option<Value>,
    meta_created: Option<Value>,
    stats: HashMap<String, usize>,
    stats_reported: bool,
}

impl PrivacyChatSseStream {
    fn new(source: ByteStream, telemetry: Arc<RequestTelemetry>) -> Self {
        Self {
            source,
            telemetry,
            line: Vec::new(),
            passthrough_line: false,
            pending: VecDeque::new(),
            ended: false,
            channels: BTreeMap::new(),
            meta_id: None,
            meta_model: None,
            meta_created: None,
            stats: HashMap::new(),
            stats_reported: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            let newline = rest.iter().position(|byte| *byte == b'\n');
            let end = newline.map_or(rest.len(), |idx| idx + 1);
            let part = &rest[..end];
            rest = &rest[end..];
            if self.passthrough_line {
                self.pending.push_back(Ok(Bytes::copy_from_slice(part)));
                self.passthrough_line = newline.is_none();
                continue;
            }
            if self.line.len().saturating_add(part.len()) > MAX_LINE_BYTES {
                self.flush_raw_line();
                self.pending.push_back(Ok(Bytes::copy_from_slice(part)));
                self.passthrough_line = newline.is_none();
                continue;
            }
            self.line.extend_from_slice(part);
            if newline.is_some() {
                self.process_complete_line();
            }
        }
    }

    fn flush_raw_line(&mut self) {
        if !self.line.is_empty() {
            self.pending
                .push_back(Ok(Bytes::from(std::mem::take(&mut self.line))));
        }
    }

    fn process_complete_line(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let line = std::mem::take(&mut self.line);
        let ending = if line.ends_with(b"\r\n") {
            "\r\n"
        } else if line.ends_with(b"\n") {
            "\n"
        } else {
            ""
        };
        let body = &line[..line.len() - ending.len()];
        let Some(payload) = body.strip_prefix(b"data:") else {
            self.pending.push_back(Ok(Bytes::from(line)));
            return;
        };
        let trimmed = payload.trim_ascii();
        if trimmed == b"[DONE]" {
            self.flush_all_channels();
            self.report_stats();
            self.pending.push_back(Ok(Bytes::from(line)));
            return;
        }
        let Ok(mut chunk) = serde_json::from_slice::<Value>(trimmed) else {
            self.pending.push_back(Ok(Bytes::from(line)));
            return;
        };
        if let Some(id) = chunk.get("id").filter(|v| !v.is_null()) {
            self.meta_id = Some(id.clone());
        }
        if let Some(model) = chunk.get("model").filter(|v| !v.is_null()) {
            self.meta_model = Some(model.clone());
        }
        if let Some(created) = chunk.get("created").filter(|v| !v.is_null()) {
            self.meta_created = Some(created.clone());
        }

        let vault = PrivacyVault::global();
        let mut modified = false;
        let mut flush_choices = Vec::new();
        if let Some(choices) = chunk.get_mut("choices").and_then(Value::as_array_mut) {
            for (pos, choice) in choices.iter_mut().enumerate() {
                let cidx = choice
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                    .unwrap_or(pos);
                let has_finish = choice.get("finish_reason").is_some_and(|v| !v.is_null());
                if let Some(delta) = choice.get_mut("delta").and_then(Value::as_object_mut) {
                    if let Some(Value::String(text)) = delta.get_mut("content") {
                        let ch = self
                            .channels
                            .entry(ChatChannel::Content(cidx))
                            .or_insert_with(|| StreamChannelRestorer::new(false));
                        let next = ch.push(text, vault, &mut self.stats);
                        if next != *text {
                            *text = next;
                            modified = true;
                        }
                    }
                    if !delta.contains_key("signature") {
                        if let Some(Value::String(text)) = delta.get_mut("reasoning_content") {
                            let ch = self
                                .channels
                                .entry(ChatChannel::Reasoning(cidx))
                                .or_insert_with(|| StreamChannelRestorer::new(false));
                            let next = ch.push(text, vault, &mut self.stats);
                            if next != *text {
                                *text = next;
                                modified = true;
                            }
                        }
                    }
                    if let Some(calls) = delta.get_mut("tool_calls").and_then(Value::as_array_mut) {
                        for (tpos, call) in calls.iter_mut().enumerate() {
                            let tidx = call
                                .get("index")
                                .and_then(Value::as_u64)
                                .map(|n| n as usize)
                                .unwrap_or(tpos);
                            if let Some(Value::String(args)) = call
                                .get_mut("function")
                                .and_then(|f| f.get_mut("arguments"))
                            {
                                let ch = self
                                    .channels
                                    .entry(ChatChannel::ToolArgs(cidx, tidx))
                                    .or_insert_with(|| StreamChannelRestorer::new(true));
                                let next = ch.push(args, vault, &mut self.stats);
                                if next != *args {
                                    *args = next;
                                    modified = true;
                                }
                            }
                        }
                    }
                }
                if has_finish {
                    flush_choices.push(cidx);
                }
            }
        }
        for cidx in flush_choices {
            self.flush_choice_channels(cidx);
        }
        if modified {
            if let Ok(encoded) = serde_json::to_string(&chunk) {
                let frame_ending = if ending.is_empty() { "\n" } else { ending };
                self.pending
                    .push_back(Ok(Bytes::from(format!("data: {encoded}{frame_ending}"))));
                return;
            }
        }
        self.pending.push_back(Ok(Bytes::from(line)));
    }

    fn flush_choice_channels(&mut self, cidx: usize) {
        let keys: Vec<ChatChannel> = self
            .channels
            .keys()
            .filter(|k| match k {
                ChatChannel::Content(i) | ChatChannel::Reasoning(i) | ChatChannel::ToolArgs(i, _) => {
                    *i == cidx
                }
            })
            .cloned()
            .collect();
        for key in keys {
            if let Some(ch) = self.channels.get_mut(&key) {
                let tail = ch.flush();
                if !tail.is_empty() {
                    self.emit_chat_flush_frame(key, tail);
                }
            }
        }
    }

    fn flush_all_channels(&mut self) {
        let keys: Vec<ChatChannel> = self.channels.keys().cloned().collect();
        for key in keys {
            if let Some(ch) = self.channels.get_mut(&key) {
                let tail = ch.flush();
                if !tail.is_empty() {
                    self.emit_chat_flush_frame(key, tail);
                }
            }
        }
    }

    fn emit_chat_flush_frame(&mut self, key: ChatChannel, text: String) {
        let (index, delta) = match key {
            ChatChannel::Content(i) => (i, json!({ "content": text })),
            ChatChannel::Reasoning(i) => (i, json!({ "reasoning_content": text })),
            ChatChannel::ToolArgs(i, ti) => (
                i,
                json!({ "tool_calls": [{ "index": ti, "function": { "arguments": text } }] }),
            ),
        };
        let mut frame = json!({
            "object": "chat.completion.chunk",
            "choices": [{
                "index": index,
                "delta": delta,
                "finish_reason": Value::Null,
            }],
        });
        if let Some(id) = &self.meta_id {
            frame["id"] = id.clone();
        }
        if let Some(model) = &self.meta_model {
            frame["model"] = model.clone();
        }
        if let Some(created) = &self.meta_created {
            frame["created"] = created.clone();
        }
        if let Ok(encoded) = serde_json::to_string(&frame) {
            self.pending
                .push_back(Ok(Bytes::from(format!("data: {encoded}\n\n"))));
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

impl Stream for PrivacyChatSseStream {
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
                    if this.line.is_empty()
                        && !this.passthrough_line
                        && this.channels.values().all(StreamChannelRestorer::is_empty)
                        && bytes.ends_with(b"\n")
                        && !bytes.contains(&b'{')
                    {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                    this.feed(&bytes);
                }
                Poll::Ready(Some(Err(err))) => {
                    this.flush_raw_line();
                    this.flush_all_channels();
                    this.report_stats();
                    this.pending.push_back(Err(err));
                    this.ended = true;
                }
                Poll::Ready(None) => {
                    this.process_complete_line();
                    this.flush_all_channels();
                    this.report_stats();
                    this.ended = true;
                }
            }
        }
    }
}

impl Drop for PrivacyChatSseStream {
    fn drop(&mut self) {
        self.report_stats();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NativeChannel {
    AnthropicText(usize),
    AnthropicJson(usize),
    ResponsesText(usize, usize),
    ResponsesArgs(usize),
    ResponsesCustomInput(usize),
}

struct PrivacyNativeSseStream {
    source: ByteStream,
    protocol: WireProtocol,
    telemetry: Arc<RequestTelemetry>,
    buffer: Vec<u8>,
    scan_from: usize,
    line_start: usize,
    pending: VecDeque<Result<Bytes, io::Error>>,
    ended: bool,
    channels: BTreeMap<NativeChannel, StreamChannelRestorer>,
    stats: HashMap<String, usize>,
    stats_reported: bool,
}

impl PrivacyNativeSseStream {
    fn new(source: ByteStream, protocol: WireProtocol, telemetry: Arc<RequestTelemetry>) -> Self {
        Self {
            source,
            protocol,
            telemetry,
            buffer: Vec::new(),
            scan_from: 0,
            line_start: 0,
            pending: VecDeque::new(),
            ended: false,
            channels: BTreeMap::new(),
            stats: HashMap::new(),
            stats_reported: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
        let mut start = 0usize;
        let mut line_start = self.line_start;
        let mut frames = Vec::new();
        for (idx, byte) in self.buffer.iter().enumerate().skip(self.scan_from) {
            if *byte != b'\n' {
                continue;
            }
            let line = &self.buffer[line_start..idx];
            if line.is_empty() || line == b"\r" {
                frames.push(self.buffer[start..=idx].to_vec());
                start = idx + 1;
            }
            line_start = idx + 1;
        }
        self.scan_from = self.buffer.len().saturating_sub(start);
        self.line_start = line_start.saturating_sub(start);
        if start > 0 {
            self.buffer.drain(..start);
        }
        for frame in frames {
            self.process_frame(frame);
        }
        if self.buffer.len() > MAX_EVENT_BYTES {
            let overflow = std::mem::take(&mut self.buffer);
            self.scan_from = 0;
            self.line_start = 0;
            self.pending.push_back(Ok(Bytes::from(overflow)));
        }
    }

    fn process_frame(&mut self, bytes: Vec<u8>) {
        let mut event_name = String::new();
        let mut data = Vec::new();
        let mut has_data = false;
        for line in bytes.split(|b| *b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if let Some(val) = line.strip_prefix(b"event:") {
                event_name = String::from_utf8_lossy(val).trim().to_string();
            } else if let Some(val) = line.strip_prefix(b"data:") {
                if has_data {
                    data.push(b'\n');
                }
                has_data = true;
                data.extend_from_slice(val.strip_prefix(b" ").unwrap_or(val));
            }
        }
        if !has_data || data.iter().all(u8::is_ascii_whitespace) {
            self.pending.push_back(Ok(Bytes::from(bytes)));
            return;
        }
        let Ok(mut event) = serde_json::from_slice::<Value>(&data) else {
            self.pending.push_back(Ok(Bytes::from(bytes)));
            return;
        };
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(&event_name)
            .to_string();

        let modified = match self.protocol {
            WireProtocol::Anthropic => self.handle_anthropic(&kind, &mut event),
            WireProtocol::Responses => self.handle_responses(&kind, &mut event),
            WireProtocol::Chat => false,
        };

        if modified {
            if let Ok(encoded) = serde_json::to_vec(&event) {
                let mut out = Vec::with_capacity(bytes.len());
                let mut written = false;
                for line in bytes.split_inclusive(|b| *b == b'\n') {
                    if line.starts_with(b"data:") {
                        if !written {
                            out.extend_from_slice(b"data: ");
                            out.extend_from_slice(&encoded);
                            if line.ends_with(b"\r\n") {
                                out.extend_from_slice(b"\r\n");
                            } else if line.ends_with(b"\n") {
                                out.push(b'\n');
                            }
                            written = true;
                        }
                    } else {
                        out.extend_from_slice(line);
                    }
                }
                self.pending.push_back(Ok(Bytes::from(out)));
                return;
            }
        }
        self.pending.push_back(Ok(Bytes::from(bytes)));
    }

    fn handle_anthropic(&mut self, kind: &str, event: &mut Value) -> bool {
        let vault = PrivacyVault::global();
        match kind {
            "content_block_delta" => {
                let idx = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let Some(delta) = event.get_mut("delta").and_then(Value::as_object_mut) else {
                    return false;
                };
                let delta_type = delta.get("type").and_then(Value::as_str).unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        if let Some(Value::String(text)) = delta.get_mut("text") {
                            let ch = self
                                .channels
                                .entry(NativeChannel::AnthropicText(idx))
                                .or_insert_with(|| StreamChannelRestorer::new(false));
                            let next = ch.push(text, vault, &mut self.stats);
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
                            let next = ch.push(partial, vault, &mut self.stats);
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
        let vault = PrivacyVault::global();
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
                        .or_insert_with(|| StreamChannelRestorer::new(false));
                    let next = ch.push(text, vault, &mut self.stats);
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
                    if text.contains("{{") {
                        let (restored, hits) = vault.restore_text(text, false);
                        if !hits.is_empty() {
                            *text = restored;
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
                    let next = ch.push(delta, vault, &mut self.stats);
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
                    if args.contains("{{") {
                        let (restored, hits) = vault.restore_text(args, true);
                        if !hits.is_empty() {
                            *args = restored;
                            self.absorb_fallback_stats(hits);
                            return true;
                        }
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
                    let next = ch.push(delta, vault, &mut self.stats);
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
                        let (restored, hits) = vault.restore_text(input, false);
                        if !hits.is_empty() {
                            *input = restored;
                            self.absorb_fallback_stats(hits);
                            return true;
                        }
                    }
                }
                false
            }
            "response.completed" | "response.incomplete" | "response.done" => {
                self.flush_all_native_channels();
                let mut modified = false;
                if let Some(resp) = event.get_mut("response") {
                    let hits = unmask_json_response(resp, WireProtocol::Responses, vault);
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
            NativeChannel::ResponsesArgs(oidx) => Some((
                "response.function_call_arguments.delta",
                json!({
                    "type": "response.function_call_arguments.delta",
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
                    if !this.buffer.is_empty() {
                        let tail = std::mem::take(&mut this.buffer);
                        this.pending.push_back(Ok(Bytes::from(tail)));
                    }
                    this.flush_all_native_channels();
                    this.report_stats();
                    this.pending.push_back(Err(err));
                    this.ended = true;
                }
                Poll::Ready(None) => {
                    if !this.buffer.is_empty() {
                        let tail = std::mem::take(&mut this.buffer);
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
