use std::collections::VecDeque;
use std::sync::Arc;

use bytes::Bytes;
use futures::{stream::BoxStream, StreamExt};
use serde_json::{json, Value};

use crate::server::core::protocol;
use crate::server::errors::GatewayError;

use super::super::usage::RequestTelemetry;
use super::{
    failed,
    parser::{self, Output},
    request::Context,
    MAX_RESPONSE_BYTES,
};

type Source = BoxStream<'static, Result<Bytes, std::io::Error>>;

struct State {
    source: Option<Source>,
    context: Context,
    telemetry: Arc<RequestTelemetry>,
    failure: Arc<std::sync::Mutex<Option<GatewayError>>>,
    pending: VecDeque<Bytes>,
    line: Vec<u8>,
    data: Vec<u8>,
    content: String,
    template: Value,
    terminal: Option<Value>,
    usage: Vec<Value>,
    done: bool,
    ended: bool,
    heartbeat: tokio::time::Interval,
}

pub fn wrap(
    source: Source,
    context: Context,
    telemetry: Arc<RequestTelemetry>,
    failure: Arc<std::sync::Mutex<Option<GatewayError>>>,
) -> Source {
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(2));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let source = super::super::stall::idle_guard(
        source,
        std::time::Duration::from_millis(
            crate::server::config::timeout_settings().stream_idle_ms(),
        ),
    );
    let state = State {
        source: Some(super::super::cancellation::cancellable(
            source,
            telemetry.cancel_token(),
        )),
        context,
        telemetry,
        failure,
        pending: VecDeque::new(),
        line: Vec::new(),
        data: Vec::new(),
        content: String::new(),
        template: json!({ "id": protocol::random_id("chatcmpl"), "object": "chat.completion.chunk" }),
        terminal: None,
        usage: Vec::new(),
        done: false,
        ended: false,
        heartbeat,
    };
    futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(frame) = state.pending.pop_front() { return Some((Ok(frame), state)); }
            if state.ended { return None; }
            let Some(source) = &mut state.source else { state.ended = true; continue; };
            let item = tokio::select! {
                biased;
                item = source.next() => item,
                _ = state.heartbeat.tick() => {
                    return Some((Ok(Bytes::from_static(b": waiting for tool validation\n\n")), state));
                }
            };
            match item {
                Some(Ok(bytes)) => {
                    state.telemetry.note_first_frame();
                    if let Err(error) = state.feed(&bytes) { state.fail(error); }
                }
                Some(Err(error)) => state.fail(failed(format!("网页响应中断，未返回工具调用：{error}"))),
                None => {
                    if state.telemetry.is_cancelled() { state.fail(super::super::cancellation::cancelled_error()); }
                    else if let Err(error) = state.finish() { state.fail(error); }
                }
            }
        }
    }).boxed()
}

impl State {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), GatewayError> {
        for byte in bytes {
            if *byte == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if line.is_empty() {
                    self.event()?;
                    if self.ended {
                        return Ok(());
                    }
                } else if let Some(value) = line.strip_prefix(b"data:") {
                    let value = value.strip_prefix(b" ").unwrap_or(value);
                    if self.data.len().saturating_add(value.len() + 1) > MAX_RESPONSE_BYTES {
                        return Err(failed("网页工具响应单帧超过大小上限"));
                    }
                    if !self.data.is_empty() {
                        self.data.push(b'\n');
                    }
                    self.data.extend_from_slice(value);
                } else if line.starts_with(b":") {
                    let mut comment = line;
                    comment.extend_from_slice(b"\n\n");
                    self.pending.push_back(Bytes::from(comment));
                }
            } else {
                self.line.push(*byte);
                if self.line.len() > MAX_RESPONSE_BYTES {
                    return Err(failed("网页工具响应单行超过大小上限"));
                }
            }
        }
        Ok(())
    }

    fn event(&mut self) -> Result<(), GatewayError> {
        let data = std::mem::take(&mut self.data);
        if data.is_empty() {
            return Ok(());
        }
        let text =
            std::str::from_utf8(&data).map_err(|_| failed("网页工具响应含有无效文字编码"))?;
        if text.trim() == "[DONE]" {
            if self.done {
                return Err(failed("网页响应重复结束，未返回工具调用"));
            }
            self.done = true;
            return Ok(());
        }
        let mut frame =
            parser::parse_json(text).map_err(|_| failed("网页工具响应不是有效的标准事件"))?;
        if frame.get("error").is_some_and(protocol::is_truthy) {
            self.telemetry.note_error(
                frame["error"]["message"]
                    .as_str()
                    .unwrap_or("网页上游返回错误"),
            );
            self.pending.push_back(protocol::chat_frame(&frame));
            self.pending
                .push_back(Bytes::from_static(b"data: [DONE]\n\n"));
            self.ended = true;
            self.source.take();
            return Ok(());
        }
        if self.ended {
            return Ok(());
        }
        if self.done
            && frame
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(|choices| !choices.is_empty())
        {
            return Err(failed("网页响应在结束后仍返回正文，未执行工具"));
        }
        for key in ["id", "model", "created", "object", "system_fingerprint"] {
            if let Some(value) = frame.get(key) {
                self.template[key] = value.clone();
            }
        }
        let choices = frame
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(|| failed("网页工具响应缺少标准 choices 数组"))?;
        if choices.is_empty() {
            if frame.get("usage").is_some() {
                if self.usage.len() >= 64 {
                    return Err(failed("网页用量事件过多"));
                }
                self.usage.push(frame);
            }
            return Ok(());
        }
        if choices.len() != 1 || choices[0]["index"].as_i64().is_some_and(|index| index != 0) {
            return Err(failed("网页工具模拟不接受多份或交错的回答"));
        }
        if self.terminal.is_some() {
            return Err(failed("网页模型在终止帧后继续生成，未执行工具"));
        }
        let choice = &choices[0];
        let delta = choice
            .get("delta")
            .and_then(Value::as_object)
            .ok_or_else(|| failed("网页工具响应缺少标准 delta 对象"))?;
        if delta
            .get("tool_calls")
            .is_some_and(|calls| !calls.is_null() && calls != &json!([]))
            || delta
                .get("function_call")
                .is_some_and(|call| !call.is_null())
        {
            return Err(failed("网页上游返回了未经模拟层确认的工具调用，未执行"));
        }
        if let Some(value) = delta.get("content").filter(|value| !value.is_null()) {
            let text = value
                .as_str()
                .ok_or_else(|| failed("网页工具响应正文必须是文字"))?;
            if self.content.len().saturating_add(text.len()) > MAX_RESPONSE_BYTES {
                return Err(failed("网页工具响应正文超过大小上限"));
            }
            self.content.push_str(text);
        }
        let finish = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|finish| !finish.is_empty());
        if finish.is_some() {
            self.terminal = Some(frame.clone());
        }
        frame["choices"][0]["delta"]
            .as_object_mut()
            .map(|delta| delta.remove("content"));
        frame["choices"][0]["finish_reason"] = Value::Null;
        if frame["choices"][0]["delta"]
            .as_object()
            .is_some_and(|delta| !delta.is_empty())
        {
            self.pending.push_back(protocol::chat_frame(&frame));
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), GatewayError> {
        // 完整的结束事件与宿主调用结束都确认以后，才把文本提升成可执行调用。
        let failure = self
            .failure
            .lock()
            .map_err(|_| failed("网页调用终态无法确认，未返回工具调用"))?
            .clone();
        if let Some(error) = failure {
            return Err(error);
        }
        if !self.line.is_empty() || !self.data.is_empty() || !self.done {
            return Err(failed("网页工具响应被截断，未返回可执行调用"));
        }
        let mut terminal = self
            .terminal
            .take()
            .ok_or_else(|| failed("网页响应缺少真实终止帧"))?;
        let reason = terminal["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or("");
        let output = if reason == "stop" {
            parser::parse(&self.content, &self.context)?
        } else if self.context.required
            || self.content.contains(super::OPEN)
            || self.content.contains(super::CLOSE)
        {
            return Err(failed("网页模型未正常完成工具调用，未执行或自动续写"));
        } else {
            Output::Text(std::mem::take(&mut self.content))
        };
        match output {
            Output::Text(text) => {
                if !text.is_empty() {
                    self.pending
                        .push_back(self.delta(json!({ "content": text })));
                }
            }
            Output::Calls { text, calls } => {
                if !text.is_empty() {
                    self.pending
                        .push_back(self.delta(json!({ "content": text })));
                }
                self.pending
                    .push_back(self.delta(json!({ "tool_calls": calls })));
                terminal["choices"][0]["finish_reason"] = json!("tool_calls");
            }
        }
        terminal["choices"][0]["delta"] = json!({});
        self.pending.push_back(protocol::chat_frame(&terminal));
        for usage in std::mem::take(&mut self.usage) {
            self.pending.push_back(protocol::chat_frame(&usage));
        }
        self.pending
            .push_back(Bytes::from_static(b"data: [DONE]\n\n"));
        self.ended = true;
        self.source.take();
        Ok(())
    }

    fn delta(&self, delta: Value) -> Bytes {
        let mut frame = self.template.clone();
        frame["choices"] = json!([{ "index": 0, "delta": delta, "finish_reason": null }]);
        protocol::chat_frame(&frame)
    }

    fn fail(&mut self, error: GatewayError) {
        if !self.content.is_empty() {
            let preview = diagnostic_preview(&self.content, 480);
            crate::server::logging::log_with_level(
                "[WebTools]",
                &format!("{}（模型输出片段：{}）", error.message, preview),
                "warn",
            );
        }
        self.content.clear();
        self.terminal.take();
        self.usage.clear();
        self.telemetry.note_error(&error.message);
        self.pending
            .push_back(protocol::chat_frame(&error.payload()));
        self.pending
            .push_back(Bytes::from_static(b"data: [DONE]\n\n"));
        self.ended = true;
        self.source.take();
    }
}

fn diagnostic_preview(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    let compact: String = if count <= max_chars {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max_chars / 2).collect();
        let tail: String = trimmed
            .chars()
            .skip(count.saturating_sub(max_chars / 2))
            .collect();
        format!("{head} …(共 {count} 字符)… {tail}")
    };
    compact.replace('\r', "\\r").replace('\n', "\\n")
}
