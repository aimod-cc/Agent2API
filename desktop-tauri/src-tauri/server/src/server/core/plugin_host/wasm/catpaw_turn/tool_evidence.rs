//! 原Protocol观察到的全批工具证据；无Clone/serde/公开Value构造器。
//! 原消息、ID、名称及参数只借原流，不授予账号/会话/发送或跨请求移交许可。
mod declaration;

use std::collections::BTreeSet;

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{json_text, protocol::ReadAssistant};

const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_BLOCKS: usize = 1024;
const MAX_CALLS: usize = 256;
const MAX_ARGUMENTS: usize = 256 * 1024;
const MAX_PARAMETER_NODES: usize = 65_536;
const MAX_RESULT: usize = 1024 * 1024;
const MAX_RESULT_GROUP: usize = 8 * 1024 * 1024;

pub(super) struct ToolEvidence {
    original_message: Value,
    calls: Vec<ToolCall>,
}

pub(super) struct ToolCall {
    id: String,
    name: String,
    arguments: String,
    parameters: Value,
}

impl ToolCall {
    pub(super) fn id(&self) -> &str { &self.id }
    pub(super) fn name(&self) -> &str { &self.name }
    /// 原toolParams逐字保留，解析对象只用于核对，不能重新编码替换原参数。
    pub(super) fn arguments(&self) -> &str { &self.arguments }
    pub(super) fn parameters(&self) -> &Value { &self.parameters }
}

impl ToolEvidence {
    pub(super) fn original_message(&self) -> &Value { &self.original_message }
    pub(super) fn calls(&self) -> &[ToolCall] { &self.calls }

    /// 只核原封存请求声明；成功不消费机会，也不授予交接或发送许可。
    pub(super) fn verify_declaration(
        &self,
        material: &crate::server::core::plugin_host::wasm::conversation::CatPawMaterial,
    ) -> Result<(), proto::WireError> {
        declaration::verify(self, material)
    }

    /// 有界完整结果组仅为材料候选，不消费调用、不生成消息ID、不授权下一次turn。
    /// 候选借原证据，不能脱离这份证据变成可跨请求恢复的所有者。
    pub(super) fn candidate_results<'a>(&'a self, message_json: &str) -> Result<ResultCandidate<'a>, proto::WireError> {
        let message = json_text::parse(message_json, MAX_RESULT_GROUP).map_err(|_| candidate_error())?;
        json_text::fields(&message, &["type", "messageId", "content", "finished"], &["type", "messageId", "content", "finished"]).map_err(|_| candidate_error())?;
        let message_id = message["messageId"].as_str().filter(|id| json_text::identifier(id, 256)).ok_or_else(candidate_error)?;
        if message["type"] != "tool" || message["finished"] != true
            || self.original_message["messageId"].as_str() == Some(message_id) { return Err(candidate_error()); }
        let blocks = message["content"].as_array().filter(|blocks| blocks.len() == self.calls.len()).ok_or_else(candidate_error)?;
        let mut seen = BTreeSet::new();
        for block in blocks {
            json_text::fields(block, &["type", "toolCallId", "toolName", "toolResult"], &["type", "toolCallId", "toolName", "toolResult"]).map_err(|_| candidate_error())?;
            let id = block["toolCallId"].as_str().ok_or_else(candidate_error)?;
            let original = self.calls.iter().find(|call| call.id() == id).ok_or_else(candidate_error)?;
            if !seen.insert(id) || block["type"] != "tool_result"
                || block["toolName"].as_str() != Some(original.name()) { return Err(candidate_error()); }
            // 并行工具结果可以按完成顺序回传；完整逐ID配对，但不重排实际结果材料。
            // 空串只接受输入明确提供的空串，不补任何结果；未知结果不可当成功。
            block["toolResult"].as_str().filter(|text| text.len() <= MAX_RESULT && !text.contains('\0')).ok_or_else(candidate_error)?;
        }
        Ok(ResultCandidate { evidence: self, message })
    }
}

pub(super) struct ResultCandidate<'a> {
    evidence: &'a ToolEvidence,
    message: Value,
}
impl ResultCandidate<'_> {
    pub(super) fn evidence(&self) -> &ToolEvidence { self.evidence }
    pub(super) fn message(&self) -> &Value { &self.message }
}

/// 增量快照只由Protocol私有读消息见证喂入；不是ToolEvidence，也不构成完成事实。
pub(super) struct Observation {
    previous: Option<Snapshot>,
    tools_seen: bool,
}
impl Observation {
    pub(super) fn new() -> Self { Self { previous: None, tools_seen: false } }
    pub(super) fn tools_seen(&self) -> bool { self.tools_seen }

    pub(super) fn observe(&mut self, original: &ReadAssistant<'_>) -> Result<(), proto::WireError> {
        let next = Snapshot::parse(original.message())?;
        if let Some(previous) = &self.previous {
            if next.message_id != previous.message_id || previous.finished && !next.finished
                || next.calls.len() < previous.calls.len() || !next.text.starts_with(previous.text.as_str())
                || !next.reasoning.starts_with(previous.reasoning.as_str()) { return Err(invalid()); }
            for (old, current) in previous.calls.iter().zip(&next.calls) {
                if old.id != current.id || old.name != current.name || !current.arguments.starts_with(old.arguments.as_str()) { return Err(invalid()); }
            }
        }
        self.tools_seen |= !next.calls.is_empty();
        self.previous = Some(next);
        Ok(())
    }

    /// 仅Protocol完整DONE后传原最后消息；构造器不接受自由Value或客户端历史。
    pub(super) fn finish(&self, original: &ReadAssistant<'_>) -> Result<Option<ToolEvidence>, proto::WireError> {
        let snapshot = self.previous.as_ref().filter(|snapshot| snapshot.finished).ok_or_else(invalid)?;
        if !self.tools_seen {
            if !snapshot.calls.is_empty() { return Err(invalid()); }
            return Ok(None);
        }
        let message = original.message();
        if snapshot.calls.is_empty() || message["finished"] != true
            || message["messageId"].as_str() != snapshot.message_id.as_deref() { return Err(invalid()); }
        let mut nodes = 0;
        let mut calls = Vec::with_capacity(snapshot.calls.len());
        for call in &snapshot.calls {
            let parameters = json_text::parse(&call.arguments, MAX_ARGUMENTS).map_err(|_| invalid())?;
            if !parameters.is_object() { return Err(invalid()); }
            charge_nodes(&parameters, 0, &mut nodes)?;
            calls.push(ToolCall { id: call.id.clone(), name: call.name.clone(), arguments: call.arguments.clone(), parameters });
        }
        // 原消息仍按流事件预算有界，所有工具完整保存，不截到第一项。
        json_text::shape(message, MAX_MESSAGE).map_err(|_| invalid())?;
        Ok(Some(ToolEvidence { original_message: message.clone(), calls }))
    }
}

struct Snapshot {
    message_id: Option<String>,
    finished: bool,
    text: String,
    reasoning: String,
    calls: Vec<ObservedCall>,
}
struct ObservedCall { id: String, name: String, arguments: String }

impl Snapshot {
    fn parse(message: &Value) -> Result<Self, proto::WireError> {
        json_text::shape(message, MAX_MESSAGE).map_err(|_| invalid())?;
        super::message::fields(message).map_err(|_| invalid())?;
        if message["type"] != "assistant" { return Err(invalid()); }
        let message_id = message.get("messageId").map(|value| value.as_str().filter(|id| json_text::identifier(id, 256)).map(str::to_string).ok_or_else(invalid)).transpose()?;
        let finished = match message.get("finished") { None => false, Some(Value::Bool(value)) => *value, _ => return Err(invalid()) };
        let blocks = message["content"].as_array().filter(|blocks| blocks.len() <= MAX_BLOCKS).ok_or_else(invalid)?;
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut calls = Vec::new();
        let mut ids = BTreeSet::new();
        let mut parameter_nodes = 0;
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => {
                    json_text::fields(block, &["type", "text", "reasoningContent"], &["type"]).map_err(|_| invalid())?;
                    let mut present = false;
                    for (name, target) in [("text", &mut text), ("reasoningContent", &mut reasoning)] {
                        if let Some(value) = block.get(name) { present = true; target.push_str(value.as_str().ok_or_else(invalid)?); }
                    }
                    if !present { return Err(invalid()); }
                }
                Some("tool_use") => {
                    json_text::fields(block, &["type", "toolCallId", "toolName", "toolParams"], &["type", "toolCallId", "toolName", "toolParams"]).map_err(|_| invalid())?;
                    let id = block["toolCallId"].as_str().filter(|id| json_text::identifier(id, 256)).ok_or_else(invalid)?;
                    let name = block["toolName"].as_str().filter(|name| valid_name(name)).ok_or_else(invalid)?;
                    let arguments = block["toolParams"].as_str().filter(|arguments| arguments.len() <= MAX_ARGUMENTS).ok_or_else(invalid)?;
                    if !ids.insert(id) || calls.len() >= MAX_CALLS || message_id.is_none() { return Err(invalid()); }
                    if finished {
                        let parameters = json_text::parse(arguments, MAX_ARGUMENTS).map_err(|_| invalid())?;
                        if !parameters.is_object() { return Err(invalid()); }
                        charge_nodes(&parameters, 0, &mut parameter_nodes)?;
                    }
                    calls.push(ObservedCall { id: id.to_string(), name: name.to_string(), arguments: arguments.to_string() });
                }
                _ => return Err(invalid()),
            }
        }
        Ok(Self { message_id, finished, text, reasoning, calls })
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn charge_nodes(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), proto::WireError> {
    *nodes = (*nodes).checked_add(1).filter(|nodes| *nodes <= MAX_PARAMETER_NODES).ok_or_else(invalid)?;
    if depth > 64 { return Err(invalid()); }
    match value {
        Value::Array(items) => { for item in items { charge_nodes(item, depth + 1, nodes)?; } }
        Value::Object(object) => { for item in object.values() { charge_nodes(item, depth + 1, nodes)?; } }
        _ => {}
    }
    Ok(())
}

pub(super) fn invalid() -> proto::WireError {
    proto::WireError::with_code("CatPaw 原工具消息不完整、重复、冲突、超预算或含未知内容，未签发工具证据或续接许可", "catpaw_turn_tool_evidence_invalid")
}
fn candidate_error() -> proto::WireError {
    proto::WireError::with_code("CatPaw 本次完整工具结果候选与原全部调用ID或名称不一致，未补结果或发送续接", "catpaw_turn_tool_result_candidate_invalid")
}
