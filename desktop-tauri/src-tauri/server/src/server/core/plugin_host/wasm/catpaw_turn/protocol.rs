//! 宿主原字节有界SSE观察，与插件ParsedTurn独立；只证明待结算材料。
//! 空行才派发，EOF残帧失败；不重连、不接受插件自报DONE或完成。
mod sse;
mod usage;

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{error, json_text, tool_evidence::{self, Observation, ToolEvidence}};
use sse::{Event, Sse};

// 字段只有原Protocol可填写；工具证据模块不能用自由Value或客户端历史补造来源。
pub(super) struct ReadAssistant<'a> { message: &'a Value }
impl ReadAssistant<'_> {
    pub(super) fn message(&self) -> &Value { self.message }
}

pub(super) struct Protocol {
    sse: Sse,
    state: State,
    failure: Option<proto::WireError>,
    eof: bool,
    tool_evidence: Option<ToolEvidence>,
}
struct State {
    conversation: String,
    observed: Option<String>,
    last_message: Option<Value>,
    usage: Option<Value>,
    done: bool,
    done_name: Option<String>,
    finished: bool,
    unknown_message: bool,
    tools: Observation,
}

impl Protocol {
    pub(super) fn new(conversation: &str) -> Self {
        Self { sse: Sse::new(), state: State { conversation: conversation.to_string(), observed: None, last_message: None, usage: None, done: false, done_name: None, finished: false, unknown_message: false, tools: Observation::new() }, failure: None, eof: false, tool_evidence: None }
    }
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(), proto::WireError> {
        if self.failure.is_some() || self.eof { self.interrupted(); return Err(error::protocol()); }
        let state = &mut self.state;
        let result = self.sse.push(bytes, |event| state.consume(event));
        if let Err(error) = &result { self.failure = Some(error.clone()); }
        result
    }
    pub(super) fn finish(&mut self) -> Result<(), proto::WireError> {
        if self.failure.is_some() || self.eof { self.interrupted(); return Err(error::protocol()); }
        self.eof = true;
        let result = self.sse.finish().and_then(|_| {
            if !self.state.done || !self.state.finished || self.state.unknown_message { return Err(error::protocol()); }
            let message = self.state.last_message.as_ref().ok_or_else(error::protocol)?;
            self.tool_evidence = self.state.tools.finish(&ReadAssistant { message })?;
            Ok(())
        });
        if let Err(error) = &result { self.failure = Some(error.clone()); }
        result
    }
    pub(super) fn complete(&self) -> bool { self.eof && self.failure.is_none() && self.state.done && self.state.finished && !self.state.unknown_message }
    pub(super) fn tool_evidence(&self) -> Result<Option<&ToolEvidence>, proto::WireError> {
        // 只证实原流完整工具材料，不证实原Owner/真实EOF交付或跨请求授权。
        if !self.complete() { return Err(tool_evidence::invalid()); }
        if self.state.tools.tools_seen() && self.tool_evidence.is_none() { return Err(tool_evidence::invalid()); }
        Ok(self.tool_evidence.as_ref())
    }
    /// 只借原完整工具证据；原请求声明与插件主张都不能补造真实响应事实。
    pub(super) fn checked_tool_evidence(
        &self,
        material: &super::super::conversation::CatPawMaterial,
        claims: &Value,
    ) -> Result<Option<&ToolEvidence>, proto::WireError> {
        let evidence = self.tool_evidence()?;
        if let Some(evidence) = evidence {
            evidence.verify_declaration(material)?;
            super::settlement::compare_claims(self, claims)?;
            if self.message() != Some(evidence.original_message()) {
                return Err(super::settlement::invalid_evidence());
            }
        }
        Ok(evidence)
    }
    /// 原Owner消费Record一次预约后按值移动；未完整或已移出时永久拒绝。
    pub(super) fn take_tool_evidence(&mut self) -> Result<ToolEvidence, proto::WireError> {
        if !self.complete() || !self.state.tools.tools_seen() {
            return Err(tool_evidence::invalid());
        }
        self.tool_evidence.take().ok_or_else(tool_evidence::invalid)
    }
    pub(super) fn interrupted(&mut self) {
        self.failure.get_or_insert_with(error::protocol);
    }
    // 只借原Owner这份有界证据；结算只能核对，不能以插件JSON重造。
    pub(super) fn message(&self) -> Option<&Value> { self.state.last_message.as_ref() }
    pub(super) fn usage(&self) -> Option<&Value> { self.state.usage.as_ref() }
    pub(super) fn observed_conversation(&self) -> Option<&str> { self.state.observed.as_deref() }
    pub(super) fn done_name(&self) -> Option<&str> { self.state.done_name.as_deref() }
    pub(super) fn wire_bytes(&self) -> usize { self.sse.wire_bytes() }
    pub(super) fn events(&self) -> usize { self.sse.events() }
    pub(super) fn normalized_usage(&self) -> Result<Option<Value>, proto::WireError> { usage::normalize(self.state.usage.as_ref()) }
    pub(super) fn check_no_tools(&self, constraints: &Value) -> Result<(), proto::WireError> {
        if !self.complete() || self.state.unknown_message || self.state.tools.tools_seen() || !matches!(constraints["tool_choice"].as_str(), Some("auto" | "none")) { return Err(super::settlement::invalid_evidence()); }
        no_tool_message(self.message().ok_or_else(super::settlement::invalid_evidence)?)
    }
}

impl State {
    fn consume(&mut self, event: Event) -> Result<(), proto::WireError> {
        if event.name.as_deref().is_some_and(|name| name.eq_ignore_ascii_case("error")) { return Err(error::protocol()); }
        if !matches!(event.name.as_deref(), None | Some("" | "message")) { return Err(error::protocol()); }
        let data = event.data.trim();
        if data == "[DONE]" {
            if self.done || !self.finished || self.last_message.is_none() { return Err(error::protocol()); }
            self.done = true;
            self.done_name = event.name;
            return Ok(());
        }
        if self.done || data.is_empty() { return Err(error::protocol()); }
        let parsed = json_text::parse(data, 1024 * 1024).map_err(|_| error::protocol())?;
        inspect(&parsed, &self.conversation, 0)?;
        let payload = parsed.get("data").unwrap_or(&parsed);
        if !parsed.is_object() || !payload.is_object() { return Err(error::protocol()); }
        if has_conversation(&parsed) { self.observed = Some(self.conversation.clone()); }
        let mut message: Option<&Value> = None;
        for candidate in [payload.get("message"), payload.pointer("/data/message"), payload.pointer("/result/message")].into_iter().flatten() {
            if message.is_some_and(|old| old != candidate) { return Err(error::protocol()); }
            message = Some(candidate);
        }
        let direct = payload.get("usage").filter(|v| !v.is_null());
        let nested = payload.pointer("/contextInfo/usage").filter(|v| !v.is_null());
        if direct.is_some() && nested.is_some() && direct != nested { return Err(error::protocol()); }
        let usage = direct.or(nested);
        if usage.is_some_and(|v| !v.is_object()) || message.is_none() && usage.is_none() { return Err(error::protocol()); }
        if let Some(message) = message {
            inspect(message, &self.conversation, 0)?;
            if !message.is_object() || message["type"] != "assistant" || !message["content"].is_array() { return Err(error::protocol()); }
            let finished = match message.get("finished") { None => false, Some(Value::Bool(value)) => *value, _ => return Err(error::protocol()) };
            // 合法tool_use只生成原观察材料，未知/缺字段/工具冲突永久锁死原协议。
            if let Err(failure) = self.tools.observe(&ReadAssistant { message }) {
                self.unknown_message = true;
                return Err(failure);
            }
            if let Some(previous) = &self.last_message {
                if previous.get("messageId") != message.get("messageId") || self.finished && previous != message { return Err(error::protocol()); }
            }
            self.last_message = Some(message.clone());
            self.finished = finished;
        }
        // 任一原事件用量格式无效即锁死协议，不能让后续事件覆盖成成功；缺字段仍未知。
        usage::normalize(usage).map_err(|_| error::protocol())?;
        // 缺用量保持未知，原值只供后续逐字核对，不补零或自行转换。
        self.usage = usage.cloned();
        Ok(())
    }
}

fn no_tool_message(message: &Value) -> Result<(), proto::WireError> {
    let invalid = super::settlement::invalid_evidence;
    super::message::fields(message).map_err(|_| invalid())?;
    if message["type"] != "assistant" || message.get("finished").is_some_and(|value| !value.is_boolean()) { return Err(invalid()); }
    if message.get("messageId").is_some_and(|value| value.as_str().is_none_or(|id| !json_text::identifier(id, 256))) { return Err(invalid()); }
    let blocks = message["content"].as_array().filter(|v| v.len() <= 1024).ok_or_else(invalid)?;
    for block in blocks {
        // 只允许已有原生text/reasoningContent，tool_use和未知内容/字段不静默过滤。
        json_text::fields(block, &["type", "text", "reasoningContent"], &["type"]).map_err(|_| invalid())?;
        if block["type"] != "text" { return Err(invalid()); }
        let mut present = false;
        for name in ["text", "reasoningContent"] {
            if let Some(value) = block.get(name) { present = true; if !value.is_string() { return Err(invalid()); } }
        }
        if !present { return Err(invalid()); }
    }
    Ok(())
}

fn inspect(value: &Value, conversation: &str, depth: usize) -> Result<(), proto::WireError> {
    if depth > 8 { return Err(error::protocol()); }
    let Some(object) = value.as_object() else { return Ok(()) };
    if object.get("error").is_some_and(|v| !v.is_null()) || value["type"] == "error" || value["success"] == false || value["failed"] == true || value["status"] == "failed" { return Err(error::protocol()); }
    if let Some(code) = object.get("code") {
        if !matches!(code.as_i64(), Some(0 | 200)) { return Err(error::protocol()); }
    }
    if object.get("success").is_some_and(|v| v.as_bool() != Some(true)) || object.get("failed").is_some_and(|v| !v.is_boolean()) { return Err(error::protocol()); }
    if object.get("conversationId").is_some_and(|v| v.as_str() != Some(conversation)) { return Err(error::protocol()); }
    for name in ["data", "result"] {
        if let Some(inner) = object.get(name) {
            if !inner.is_object() { return Err(error::protocol()); }
            inspect(inner, conversation, depth + 1)?;
        }
    }
    Ok(())
}

fn has_conversation(value: &Value) -> bool {
    value.get("conversationId").is_some() || ["data", "result"].iter().any(|name| value.get(*name).is_some_and(has_conversation))
}
