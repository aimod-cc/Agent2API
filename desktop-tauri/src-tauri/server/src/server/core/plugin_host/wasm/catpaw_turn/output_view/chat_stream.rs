//! 固定 Chat SSE 的逐帧观察；空行才消费，不把 EOF 残帧补成终态。
//! 本模块只累计最终投影与观察事实，不调用宿主、HTTP、工具或账号接口。

use agent2api_provider_protocol::WireError;
use serde_json::Value;

use super::{invalid, json_text, Identity, OutputObservation, Projection, Usage, MAX_EVENT};

const MAX_EVENTS: usize = 16_384;

pub(super) struct Stream {
    line: Vec<u8>,
    data: String,
    name: Option<String>,
    has_data: bool,
    first_line: bool,
    pending_cr: bool,
    event_bytes: usize,
    events: usize,
    state: State,
}

struct Event {
    name: Option<String>,
    data: String,
}

struct State {
    identity: Option<Identity>,
    message: Projection,
    role_seen: bool,
    finished: bool,
    finish: Option<&'static str>,
    usage: Option<Usage>,
    done: bool,
}

impl Stream {
    pub(super) fn new() -> Self {
        Self {
            line: Vec::new(),
            data: String::new(),
            name: None,
            has_data: false,
            first_line: true,
            pending_cr: false,
            event_bytes: 0,
            events: 0,
            state: State {
                identity: None,
                message: Projection::new(),
                role_seen: false,
                finished: false,
                finish: None,
                usage: None,
                done: false,
            },
        }
    }

    pub(super) fn terminal_seen(&self) -> bool { self.state.finished }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(), WireError> {
        for &byte in bytes {
            if self.pending_cr {
                self.pending_cr = false;
                if byte == b'\n' {
                    // CRLF 的两个物理字节先完整计费，再派发可能的终态空行。
                    self.charge_byte()?;
                    self.consume_line()?;
                    continue;
                }
                self.consume_line()?;
            }
            // DONE 之后仅允许完整空行；包括注释、第二次 DONE 在内的非空数据都拒绝。
            if self.state.done && !matches!(byte, b'\r' | b'\n') {
                return Err(invalid());
            }
            self.charge_byte()?;
            match byte {
                b'\r' => self.pending_cr = true,
                b'\n' => self.consume_line()?,
                _ => self.line.push(byte),
            }
        }
        Ok(())
    }

    fn charge_byte(&mut self) -> Result<(), WireError> {
        self.event_bytes = self
            .event_bytes
            .checked_add(1)
            .filter(|bytes| *bytes <= MAX_EVENT)
            .ok_or_else(invalid)?;
        Ok(())
    }

    fn consume_line(&mut self) -> Result<(), WireError> {
        let raw = std::mem::take(&mut self.line);
        let bytes = if self.first_line {
            raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&raw)
        } else {
            &raw
        };
        self.first_line = false;
        let line = std::str::from_utf8(bytes).map_err(|_| invalid())?;
        if line.is_empty() {
            self.events = self
                .events
                .checked_add(1)
                .filter(|events| *events <= MAX_EVENTS)
                .ok_or_else(invalid)?;
            if self.has_data || self.name.is_some() {
                let event = Event {
                    name: self.name.take(),
                    data: std::mem::take(&mut self.data),
                };
                self.has_data = false;
                self.state.consume(event)?;
            }
            self.event_bytes = 0;
            return Ok(());
        }
        if line.starts_with(':') {
            // 普通完整注释不产生内容；工具终帧后不接受额外控制事件。
            if self.state.finished {
                return Err(invalid());
            }
            return Ok(());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            "event" => {
                if self.name.is_some() || value.len() > 128 || value.chars().any(char::is_control) {
                    return Err(invalid());
                }
                self.name = Some(value.to_string());
            }
            // 固定出站不发 id/retry；不把额外元数据当作无害控制材料静默过滤。
            _ => return Err(invalid()),
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<OutputObservation, WireError> {
        if self.pending_cr {
            // 单独 CR 也是完整行终止符；仅结清真实收到的 CR，不补换行或事件。
            self.pending_cr = false;
            self.consume_line()?;
        }
        if !self.line.is_empty()
            || self.has_data
            || self.name.is_some()
            || self.event_bytes != 0
            || !self.state.done
            || !self.state.finished
            || !self.state.role_seen
        {
            return Err(invalid());
        }
        let finish = self.state.finish.ok_or_else(invalid)?;
        if finish == "tool_calls" { self.state.message.complete_tools()?; }
        if finish == "stop" && !self.state.message.tools.is_empty() { return Err(invalid()); }
        let identity = self.state.identity.ok_or_else(invalid)?;
        Ok(OutputObservation {
            message: self.state.message,
            _identity: identity,
            _usage: self.state.usage,
            finish,
        })
    }
}

impl State {
    fn consume(&mut self, event: Event) -> Result<(), WireError> {
        if !matches!(event.name.as_deref(), None | Some("" | "message")) {
            return Err(invalid());
        }
        // 不 trim 终态标记，更不 trim 内容或工具参数；必须是真实完整空行派发。
        if event.data == "[DONE]" {
            if self.done || !self.finished || !self.role_seen {
                return Err(invalid());
            }
            self.done = true;
            return Ok(());
        }
        if self.done || event.data.is_empty() {
            return Err(invalid());
        }
        let value = json_text::parse(&event.data, MAX_EVENT).map_err(|_| invalid())?;
        let identity = Identity::parse(&value, "chat.completion.chunk")?;
        if let Some(previous) = &self.identity {
            if !previous.same(&identity) {
                return Err(invalid());
            }
        } else {
            self.identity = Some(identity);
        }
        let choices = value["choices"].as_array().ok_or_else(invalid)?;
        if self.finished {
            // 工具 finish 后只允许一次有完整真实计数的 usage 帧，随后必须 DONE。
            if !choices.is_empty() || self.usage.is_some() {
                return Err(invalid());
            }
            self.usage = Some(Usage::parse(value.get("usage").ok_or_else(invalid)?)?);
            return Ok(());
        }
        if value.get("usage").is_some_and(|usage| !usage.is_null()) || choices.len() != 1 {
            return Err(invalid());
        }
        let choice = &choices[0];
        json_text::fields(
            choice,
            &["index", "delta", "finish_reason"],
            &["index", "delta", "finish_reason"],
        )
        .map_err(|_| invalid())?;
        if choice["index"].as_u64() != Some(0) {
            return Err(invalid());
        }
        let delta = json_text::fields(
            &choice["delta"],
            &["role", "content", "reasoning_content", "tool_calls"],
            &[],
        )
        .map_err(|_| invalid())?;
        if !self.role_seen {
            if delta.len() != 1
                || delta.get("role").and_then(Value::as_str) != Some("assistant")
                || !choice["finish_reason"].is_null()
            {
                return Err(invalid());
            }
            self.role_seen = true;
            return Ok(());
        }
        if delta.contains_key("role") {
            return Err(invalid());
        }
        match &choice["finish_reason"] {
            Value::Null => {
                if delta.is_empty() {
                    return Err(invalid());
                }
                if let Some(content) = delta.get("content") {
                    let text = content
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .ok_or_else(invalid)?;
                    self.message.append_content(text)?;
                }
                if let Some(reasoning) = delta.get("reasoning_content") {
                    let text = reasoning
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .ok_or_else(invalid)?;
                    self.message.append_reasoning(text)?;
                }
                if let Some(tools) = delta.get("tool_calls") {
                    self.observe_tools(tools)?;
                }
            }
            Value::String(reason) if reason == "tool_calls" && delta.is_empty() => {
                self.message.complete_tools()?;
                self.finish = Some("tool_calls");
                self.finished = true;
            }
            Value::String(reason) if reason == "stop" && delta.is_empty() && self.message.tools.is_empty() => {
                self.finish = Some("stop");
                self.finished = true;
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }

    fn observe_tools(&mut self, value: &Value) -> Result<(), WireError> {
        let tools = value
            .as_array()
            .filter(|tools| !tools.is_empty() && tools.len() <= super::MAX_TOOLS)
            .ok_or_else(invalid)?;
        let mut previous = None;
        for tool in tools {
            json_text::fields(
                tool,
                &["index", "id", "type", "function"],
                &["index", "function"],
            )
            .map_err(|_| invalid())?;
            let index = tool["index"]
                .as_u64()
                .filter(|index| *index < super::MAX_TOOLS as u64)
                .ok_or_else(invalid)? as usize;
            // 同帧顺序必须严格递增；首次调用索引必须与已累积工具数量完全连续。
            if previous.is_some_and(|old| index <= old) || index > self.message.tools.len() {
                return Err(invalid());
            }
            previous = Some(index);
            let function = &tool["function"];
            if index == self.message.tools.len() {
                json_text::fields(
                    tool,
                    &["index", "id", "type", "function"],
                    &["index", "id", "type", "function"],
                )
                .map_err(|_| invalid())?;
                json_text::fields(function, &["name", "arguments"], &["name", "arguments"])
                    .map_err(|_| invalid())?;
                if tool["type"] != "function" {
                    return Err(invalid());
                }
                let id = tool["id"].as_str().ok_or_else(invalid)?;
                let name = function["name"].as_str().ok_or_else(invalid)?;
                let arguments = function["arguments"].as_str().ok_or_else(invalid)?;
                self.message.add_tool(id, name, arguments)?;
            } else {
                // 插件固定增量只含原索引和参数后缀；不接受再次首见、改名或补 ID。
                json_text::fields(tool, &["index", "function"], &["index", "function"])
                    .map_err(|_| invalid())?;
                json_text::fields(function, &["arguments"], &["arguments"])
                    .map_err(|_| invalid())?;
                let arguments = function["arguments"].as_str().ok_or_else(invalid)?;
                self.message.append_arguments(index, arguments)?;
            }
        }
        Ok(())
    }
}
