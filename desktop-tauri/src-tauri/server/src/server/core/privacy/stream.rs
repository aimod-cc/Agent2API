//! 流式 SSE 隐私还原：
//! - 保持逐通道前缀有界缓冲（只暂扣半截占位符，不扣整帧，低延迟）；
//! - 兼容稳定占位符 `{{KIND_xxxxxxxx}}` 与单次随机隐藏类型占位符 `{{A2A_<32hex>}}`；
//! - 兼容 CR/LF/CRLF 换行、首帧 UTF-8 BOM、多行 data 与注释行，带膨胀与帧尺寸安全上限；
//! - 支持 JSON 结构化输出模式（流式正文通道自动做 JSON 字符串转义）。

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

use super::sse_frame::{parse_sse_frame, rewrite_sse_frame, SseFrameSplitter};
use super::stream_native::PrivacyNativeSseStream;
use super::vault::{
    normalize_placeholder_inner, push_json_escaped_inner, sorted_counts, PrivacyVault,
    RequestVault, MAX_RESTORE_EXPANSION,
};
use super::walker::unmask_json_response_with_mode;

const MAX_PLACEHOLDER_INNER_BYTES: usize = 48;

pub(super) type ByteStream = Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send + Unpin>;

/// 流式单文本通道的有界前缀缓冲状态机：跨任意 chunk 切分还原 `{{KIND_xxxxxxxx}}` 与 `{{A2A_<32hex>}}`。
#[derive(Clone, Debug)]
pub struct StreamChannelRestorer {
    buf: String,
    json_string_escape: bool,
    remaining_budget: usize,
}

impl Default for StreamChannelRestorer {
    fn default() -> Self {
        Self::new(false)
    }
}

impl StreamChannelRestorer {
    pub fn new(json_string_escape: bool) -> Self {
        Self {
            buf: String::new(),
            json_string_escape,
            remaining_budget: MAX_RESTORE_EXPANSION,
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
                            if let Some(next_budget) =
                                self.remaining_budget.checked_sub(original.len())
                            {
                                self.remaining_budget = next_budget;
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

pub fn unmask_outcome(
    outcome: ForwardOutcome,
    telemetry: Arc<RequestTelemetry>,
) -> ForwardOutcome {
    unmask_outcome_with_options(outcome, telemetry, RequestVault::global(), false)
}

pub fn unmask_outcome_with_options(
    outcome: ForwardOutcome,
    telemetry: Arc<RequestTelemetry>,
    vault: RequestVault,
    json_output: bool,
) -> ForwardOutcome {
    match outcome {
        ForwardOutcome::Completion { mut body } => {
            let restored =
                unmask_json_response_with_mode(&mut body, WireProtocol::Chat, &vault, json_output);
            if !restored.is_empty() {
                telemetry.note_privacy_restored(&restored);
            }
            ForwardOutcome::Completion { body }
        }
        ForwardOutcome::Stream { status, stream } => ForwardOutcome::Stream {
            status,
            stream: Box::new(PrivacyChatSseStream::new(
                stream,
                telemetry,
                vault,
                json_output,
            )),
        },
        ForwardOutcome::Native(NativeResponse {
            protocol,
            status,
            headers,
            body,
        }) => {
            let body = match body {
                NativeBody::Json(mut json) => {
                    let restored =
                        unmask_json_response_with_mode(&mut json, protocol, &vault, json_output);
                    if !restored.is_empty() {
                        telemetry.note_privacy_restored(&restored);
                    }
                    NativeBody::Json(json)
                }
                NativeBody::Stream(stream) => match protocol {
                    WireProtocol::Chat => NativeBody::Stream(Box::new(PrivacyChatSseStream::new(
                        stream,
                        telemetry,
                        vault,
                        json_output,
                    ))),
                    WireProtocol::Anthropic | WireProtocol::Responses => NativeBody::Stream(
                        Box::new(PrivacyNativeSseStream::new(
                            stream,
                            protocol,
                            telemetry,
                            vault,
                            json_output,
                        )),
                    ),
                },
            };
            ForwardOutcome::Native(NativeResponse {
                protocol,
                status,
                headers,
                body,
            })
        }
    }
}

// ─── Chat Completions SSE 还原流 ─────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ChatChannel {
    Content(usize),
    Refusal(usize),
    Reasoning(usize),
    LegacyArgs(usize),
    ToolArgs(usize, usize),
    CustomInput(usize, usize),
}

struct PrivacyChatSseStream {
    source: ByteStream,
    telemetry: Arc<RequestTelemetry>,
    vault: RequestVault,
    json_output: bool,
    splitter: SseFrameSplitter,
    first_frame: bool,
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
    fn new(
        source: ByteStream,
        telemetry: Arc<RequestTelemetry>,
        vault: RequestVault,
        json_output: bool,
    ) -> Self {
        Self {
            source,
            telemetry,
            vault,
            json_output,
            splitter: SseFrameSplitter::default(),
            first_frame: true,
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
        if parsed.is_done {
            self.flush_all_channels();
            self.report_stats();
            self.pending.push_back(Ok(Bytes::from(raw)));
            return;
        }
        let Some(data_text) = parsed.data_text else {
            self.pending.push_back(Ok(Bytes::from(raw)));
            return;
        };
        let Ok(mut chunk) = serde_json::from_str::<Value>(&data_text) else {
            self.pending.push_back(Ok(Bytes::from(raw)));
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
                    modified |= self.process_chat_delta(cidx, delta);
                }
                if let Some(msg) = choice.get_mut("message").and_then(Value::as_object_mut) {
                    // 某些兼容上游会在流式帧里直接携带完整 message
                    let mut wrapper = json!({ "choices": [{ "message": Value::Object(msg.clone()) }] });
                    let restored = unmask_json_response_with_mode(
                        &mut wrapper,
                        WireProtocol::Chat,
                        &self.vault,
                        self.json_output,
                    );
                    if !restored.is_empty() {
                        if let Some(updated) = wrapper
                            .pointer_mut("/choices/0/message")
                            .and_then(Value::as_object_mut)
                        {
                            *msg = std::mem::take(updated);
                        }
                        for (k, v) in restored {
                            *self.stats.entry(k).or_insert(0) += v;
                        }
                        modified = true;
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
            if let Some(rewritten) = rewrite_sse_frame(&raw, parsed.leading_bom, &chunk) {
                self.pending.push_back(Ok(Bytes::from(rewritten)));
                return;
            }
        }
        self.pending.push_back(Ok(Bytes::from(raw)));
    }

    fn process_chat_delta(
        &mut self,
        cidx: usize,
        delta: &mut serde_json::Map<String, Value>,
    ) -> bool {
        let mut modified = false;
        if let Some(Value::String(text)) = delta.get_mut("content") {
            let ch = self
                .channels
                .entry(ChatChannel::Content(cidx))
                .or_insert_with(|| StreamChannelRestorer::new(self.json_output));
            let next = ch.push(text, &self.vault, &mut self.stats);
            if next != *text {
                *text = next;
                modified = true;
            }
        }
        if let Some(Value::String(text)) = delta.get_mut("refusal") {
            let ch = self
                .channels
                .entry(ChatChannel::Refusal(cidx))
                .or_insert_with(|| StreamChannelRestorer::new(false));
            let next = ch.push(text, &self.vault, &mut self.stats);
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
                let next = ch.push(text, &self.vault, &mut self.stats);
                if next != *text {
                    *text = next;
                    modified = true;
                }
            }
        }
        if let Some(Value::String(args)) = delta
            .get_mut("function_call")
            .and_then(|fc| fc.get_mut("arguments"))
        {
            let ch = self
                .channels
                .entry(ChatChannel::LegacyArgs(cidx))
                .or_insert_with(|| StreamChannelRestorer::new(true));
            let next = ch.push(args, &self.vault, &mut self.stats);
            if next != *args {
                *args = next;
                modified = true;
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
                    let next = ch.push(args, &self.vault, &mut self.stats);
                    if next != *args {
                        *args = next;
                        modified = true;
                    }
                }
                if let Some(Value::String(input)) =
                    call.get_mut("custom").and_then(|c| c.get_mut("input"))
                {
                    let ch = self
                        .channels
                        .entry(ChatChannel::CustomInput(cidx, tidx))
                        .or_insert_with(|| StreamChannelRestorer::new(false));
                    let next = ch.push(input, &self.vault, &mut self.stats);
                    if next != *input {
                        *input = next;
                        modified = true;
                    }
                }
            }
        }
        modified
    }

    fn flush_choice_channels(&mut self, cidx: usize) {
        let keys: Vec<ChatChannel> = self
            .channels
            .keys()
            .filter(|k| match k {
                ChatChannel::Content(i)
                | ChatChannel::Refusal(i)
                | ChatChannel::Reasoning(i)
                | ChatChannel::LegacyArgs(i)
                | ChatChannel::ToolArgs(i, _)
                | ChatChannel::CustomInput(i, _) => *i == cidx,
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
            ChatChannel::Refusal(i) => (i, json!({ "refusal": text })),
            ChatChannel::Reasoning(i) => (i, json!({ "reasoning_content": text })),
            ChatChannel::LegacyArgs(i) => (i, json!({ "function_call": { "arguments": text } })),
            ChatChannel::ToolArgs(i, ti) => (
                i,
                json!({ "tool_calls": [{ "index": ti, "function": { "arguments": text } }] }),
            ),
            ChatChannel::CustomInput(i, ti) => (
                i,
                json!({ "tool_calls": [{ "index": ti, "custom": { "input": text } }] }),
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
                    if this.splitter.is_empty()
                        && !this.first_frame
                        && this.channels.values().all(StreamChannelRestorer::is_empty)
                        && (bytes.ends_with(b"\n\n") || bytes.ends_with(b"\r\n\r\n"))
                        && !bytes.contains(&b'{')
                    {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                    this.feed(&bytes);
                }
                Poll::Ready(Some(Err(err))) => {
                    for tail in this.splitter.finish() {
                        this.pending.push_back(Ok(Bytes::from(tail)));
                    }
                    this.flush_all_channels();
                    this.report_stats();
                    this.pending.push_back(Err(err));
                    this.ended = true;
                }
                Poll::Ready(None) => {
                    for tail in this.splitter.finish() {
                        this.process_frame(tail);
                    }
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
