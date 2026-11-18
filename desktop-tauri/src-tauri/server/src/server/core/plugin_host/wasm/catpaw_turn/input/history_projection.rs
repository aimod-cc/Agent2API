//! 原封存Chat消息 -> 已有CatPaw消息/指令确定投影；只核材料，不选择会话或发送。
//! 依据插件messages.rs/blocks.rs：每个非指令原消息消费一枚宿主ID，再按原规则合并。
use std::collections::{BTreeMap, BTreeSet};

use agent2api_provider_protocol as proto;
use serde_json::{json, Map, Value};

use super::{json_text, mismatch, unexpressed, validate_tool_json};
use super::super::{Correlation, PreparedInput};

const MAX_BLOCKS: usize = 256;
const MAX_MERGED: usize = 1024;
const MAX_ARGUMENTS: usize = 256 * 1024;
const MAX_RESULT: usize = 1024 * 1024;
const MAX_DIRECTIVE: usize = 1024 * 1024;

pub(super) struct Projection {
    pub(super) history: Value,
    pub(super) directives: Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role { User, Assistant, Tool }
impl Role {
    fn wire(self) -> &'static str { match self { Self::User => "user", Self::Assistant => "assistant", Self::Tool => "tool" } }
}
struct Message {
    role: Role,
    id: String,
    content: Vec<Value>,
    finished: bool,
    calls: Vec<(String, String)>,
}

pub(super) fn project(raw: &[Value], prepared: &PreparedInput, ids: &Correlation) -> Result<Projection, proto::WireError> {
    if raw.is_empty() || raw.len() > 512 || ids.messages.len() != prepared.message_count { return Err(mismatch()); }
    let mut seen = BTreeSet::new();
    for id in &ids.messages {
        if !json_text::identifier(id, 256) || id == &ids.conversation || id == &ids.turn || id == &ids.response || !seen.insert(id) { return Err(mismatch()); }
    }
    let mut used = 0;
    let mut stage = Vec::new();
    let mut system = Vec::new();
    let mut rules = Vec::new();
    for message in raw {
        let object = json_text::fields(message, &["role", "content", "id", "messageId", "finished", "name", "reasoning_content", "reasoningContent", "tool_calls", "tool_call_id", "toolCalls", "toolCallId", "function_call"], &["role"])?;
        // 仅宿主原池能提供本次messageId；客户端自带ID不成为转译来源。
        if ["id", "messageId"].iter().any(|name| object.get(*name).is_some_and(|value| !value.is_null())) { return Err(mismatch()); }
        let role = object.get("role").and_then(Value::as_str).ok_or_else(mismatch)?;
        if matches!(role, "system" | "developer") {
            reject_nonnull(object, &["finished", "name", "reasoning_content", "reasoningContent", "tool_calls", "tool_call_id", "toolCalls", "toolCallId", "function_call"])?;
            let content = content(object.get("content"))?;
            if content.iter().any(|block| block.get("reasoningContent").is_some()) { return Err(unexpressed()); }
            let text = content.iter().filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n");
            if text.trim().is_empty() || text.len() > MAX_DIRECTIVE { return Err(mismatch()); }
            if role == "system" { system.push(text); } else { rules.push(text); }
            continue;
        }
        let id = ids.messages.get(used).ok_or_else(mismatch)?.clone();
        used += 1;
        let finished = match object.get("finished") { None => true, Some(Value::Bool(value)) => *value, _ => return Err(mismatch()) };
        let next = match role {
            "user" => {
                reject_nonnull(object, &["name", "reasoning_content", "reasoningContent", "tool_calls", "tool_call_id", "toolCalls", "toolCallId", "function_call"])?;
                let content = content(object.get("content"))?;
                if content.is_empty() { return Err(mismatch()); }
                Message { role: Role::User, id, content, finished, calls: Vec::new() }
            }
            "assistant" => assistant(object, id, finished)?,
            "tool" => tool(object, id, finished)?,
            _ => return Err(mismatch()),
        };
        merge(&mut stage, next)?;
    }
    if used != ids.messages.len() || used != prepared.message_count || stage.is_empty() { return Err(mismatch()); }
    pair(&mut stage)?;
    let last = stage.last().ok_or_else(mismatch)?;
    let tail = last.role;
    if prepared.continuation != (tail == Role::Tool)
        || !prepared.continuation && prepared.constraints["client_stream"] == true && tail != Role::User {
        return Err(mismatch());
    }
    let history = Value::Array(stage.into_iter().map(|message| json!({"type": message.role.wire(), "messageId": message.id, "content": message.content, "finished": message.finished})).collect());
    json_text::shape(&history, json_text::MAX_BYTES)?;
    let directives = json!({"system_prompt": join(system)?, "rules_message": join(rules)?});
    Ok(Projection { history, directives })
}

fn assistant(object: &Map<String, Value>, id: String, finished: bool) -> Result<Message, proto::WireError> {
    reject_nonnull(object, &["name", "tool_call_id", "toolCallId", "toolCalls", "function_call"])?;
    let mut content = content(object.get("content"))?;
    let reasoning = same_reasoning(object)?;
    if let Some(reasoning) = reasoning.filter(|reasoning| !reasoning.is_empty()) {
        if let Some(first) = content.first_mut() {
            // 原算法挂到首块；两个不同原思考文本不可在核对中被覆盖丢掉。
            if first.get("reasoningContent").is_some_and(|value| value.as_str() != Some(reasoning)) { return Err(mismatch()); }
            first["reasoningContent"] = json!(reasoning);
        } else { content.push(json!({"type": "text", "text": "", "reasoningContent": reasoning})); }
    }
    let items = match object.get("tool_calls") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(items)) if items.len() <= 256 => items.as_slice(),
        _ => return Err(mismatch()),
    };
    let mut seen = BTreeSet::new();
    let mut calls = Vec::with_capacity(items.len());
    for call in items {
        json_text::fields(call, &["id", "type", "function"], &["id", "type", "function"])?;
        if call["type"] != "function" { return Err(mismatch()); }
        let id = call["id"].as_str().filter(|id| json_text::identifier(id, 256)).ok_or_else(mismatch)?;
        if !seen.insert(id) { return Err(mismatch()); }
        let function = json_text::fields(&call["function"], &["name", "arguments"], &["name", "arguments"])?;
        let name = function.get("name").and_then(Value::as_str).filter(|name| tool_name(name)).ok_or_else(mismatch)?;
        let arguments = match function.get("arguments") {
            Some(Value::String(text)) => {
                let parsed = json_text::parse(text, MAX_ARGUMENTS)?;
                if !parsed.is_object() { return Err(mismatch()); }
                validate_tool_json(&parsed, false)?;
                text.clone()
            }
            Some(value @ Value::Object(_)) => {
                validate_tool_json(value, false)?;
                // 插件blocks::arguments对象分支明确serde_json::to_string；两端当前均
                // 未启用preserve_order。只认当前确定编码，不解析后接受另一文本重排。
                json_text::encode(value, MAX_ARGUMENTS)?
            }
            _ => return Err(mismatch()),
        };
        content.push(json!({"type": "tool_use", "toolCallId": id, "toolName": name, "toolParams": arguments}));
        calls.push((id.to_string(), name.to_string()));
    }
    if content.len() > MAX_MERGED { return Err(mismatch()); }
    Ok(Message { role: Role::Assistant, id, content, finished, calls })
}

fn tool(object: &Map<String, Value>, id: String, finished: bool) -> Result<Message, proto::WireError> {
    reject_nonnull(object, &["reasoning_content", "reasoningContent", "tool_calls", "toolCalls", "toolCallId", "function_call"])?;
    let call = object.get("tool_call_id").and_then(Value::as_str).filter(|id| json_text::identifier(id, 256)).ok_or_else(mismatch)?;
    let mut block = json!({"type": "tool_result", "toolCallId": call, "toolResult": tool_result(object.get("content").ok_or_else(mismatch)?)?});
    match object.get("name") {
        None | Some(Value::Null) => {},
        Some(Value::String(name)) if name.trim().is_empty() => {},
        Some(Value::String(name)) if tool_name(name.trim()) => block["toolName"] = json!(name.trim()),
        _ => return Err(mismatch()),
    }
    Ok(Message { role: Role::Tool, id, content: vec![block], finished, calls: Vec::new() })
}

fn content(value: Option<&Value>) -> Result<Vec<Value>, proto::WireError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(vec![json!({"type": "text", "text": text})]),
        Some(Value::Array(items)) if items.len() <= MAX_BLOCKS => items.iter().map(|item| {
            if let Value::String(text) = item { return Ok(json!({"type": "text", "text": text})); }
            if item["type"] == "image_url" {
                return super::super::super::super::catpaw_images::source(item);
            }
            let object = json_text::fields(item, &["type", "text", "reasoningContent", "reasoning_content"], &["type", "text"])?;
            if !matches!(item["type"].as_str(), Some("text" | "output_text")) || !item["text"].is_string() { return Err(unexpressed()); }
            let mut block = json!({"type": "text", "text": item["text"]});
            if let Some(reasoning) = same_reasoning(object)? { block["reasoningContent"] = json!(reasoning); }
            Ok(block)
        }).collect(),
        _ => Err(unexpressed()),
    }
}

fn same_reasoning(object: &Map<String, Value>) -> Result<Option<&str>, proto::WireError> {
    let snake = object.get("reasoning_content").filter(|value| !value.is_null());
    let camel = object.get("reasoningContent").filter(|value| !value.is_null());
    if snake.is_some() && camel.is_some() && snake != camel { return Err(mismatch()); }
    snake.or(camel).map(|value| value.as_str().ok_or_else(mismatch)).transpose()
}

fn tool_result(value: &Value) -> Result<String, proto::WireError> {
    let result = match value {
        Value::String(text) => text.clone(),
        Value::Array(items) if items.len() <= MAX_BLOCKS => {
            if items.iter().all(Value::is_string) { items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n") }
            else if items.iter().all(|item| matches!(item["type"].as_str(), Some("text" | "output_text")) && item["text"].is_string()) {
                for item in items { json_text::fields(item, &["type", "text"], &["type", "text"])?; }
                items.iter().filter_map(|item| item["text"].as_str()).collect::<Vec<_>>().join("\n")
            } else {
                // 原算法混合块编码为JSON文本；只容许已支持文本，未知动作/图片不猜。
                for item in items {
                    if !item.is_string() {
                        json_text::fields(item, &["type", "text"], &["type", "text"])?;
                        if !matches!(item["type"].as_str(), Some("text" | "output_text")) || !item["text"].is_string() { return Err(unexpressed()); }
                    }
                }
                validate_tool_json(value, false)?;
                json_text::encode(value, MAX_ARGUMENTS)?
            }
        }
        _ => return Err(unexpressed()),
    };
    if result.len() > MAX_RESULT || result.contains('\0') { return Err(mismatch()); }
    Ok(result)
}

fn merge(stage: &mut Vec<Message>, next: Message) -> Result<(), proto::WireError> {
    let mergeable = stage.last().is_some_and(|previous| previous.role == next.role
        && (next.role == Role::Tool || next.role == Role::Assistant && (previous.calls.is_empty() || next.calls.is_empty())));
    if !mergeable { stage.push(next); return Ok(()); }
    let previous = stage.last_mut().ok_or_else(mismatch)?;
    if previous.content.len().checked_add(next.content.len()).is_none_or(|size| size > MAX_MERGED) || previous.finished != next.finished { return Err(mismatch()); }
    previous.content.extend(next.content);
    previous.calls.extend(next.calls);
    // 原算法只合并内容，保留第一条ID；后续原消息仍已消费本次池中的ID。
    Ok(())
}

fn pair(messages: &mut [Message]) -> Result<(), proto::WireError> {
    let mut pending = BTreeMap::new();
    let mut known = BTreeSet::new();
    for message in messages {
        match message.role {
            Role::Assistant => {
                if !pending.is_empty() { return Err(mismatch()); }
                for (id, name) in &message.calls {
                    if !known.insert(id.clone()) || known.len() > 4096 { return Err(mismatch()); }
                    pending.insert(id.clone(), name.clone());
                }
            }
            Role::Tool => {
                for block in &mut message.content {
                    let id = block["toolCallId"].as_str().ok_or_else(mismatch)?;
                    let name = pending.remove(id).ok_or_else(mismatch)?;
                    if block["toolName"].as_str().is_some_and(|actual| actual != name) { return Err(mismatch()); }
                    // 名称只按已配对原历史ID回填，不用名字补ID或产生远端许可。
                    block["toolName"] = json!(name);
                }
                if !pending.is_empty() { return Err(mismatch()); }
            }
            Role::User => { if !pending.is_empty() { return Err(mismatch()); } }
        }
    }
    if pending.is_empty() { Ok(()) } else { Err(mismatch()) }
}

fn join(parts: Vec<String>) -> Result<Option<String>, proto::WireError> {
    if parts.is_empty() { return Ok(None); }
    let size = parts.iter().try_fold(0usize, |size, text| size.checked_add(text.len())).and_then(|size| size.checked_add((parts.len() - 1) * 2)).ok_or_else(mismatch)?;
    if size > MAX_DIRECTIVE { return Err(mismatch()); }
    Ok(Some(parts.join("\n\n")))
}
fn reject_nonnull(object: &Map<String, Value>, names: &[&str]) -> Result<(), proto::WireError> {
    if names.iter().any(|name| object.get(*name).is_some_and(|value| !value.is_null())) { Err(unexpressed()) } else { Ok(()) }
}
fn tool_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}
