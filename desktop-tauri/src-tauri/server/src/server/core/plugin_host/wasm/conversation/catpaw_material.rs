//! CatPaw实际WASM入场body的封闭材料。只有原ConversationCall可配对，非来源授权。
//! 规范投影保留全部字段、键是否存在、数组顺序和原文本；不补默认、不trim或删reasoning。
//! 工厂须由原Runtime拿尚未改写的实际params.body调用；本模块不取得账号/代理或会话Owner。
use std::collections::BTreeSet;
use std::io::{self, Write};
use std::sync::{Arc, Weak};

use agent2api_provider_protocol as proto;
use serde_json::{Map, Value};

use super::{ConversationCall, Signal};
use crate::server::core::plugin_host::{wasm::context::Shared, Host};

const MAX_BODY: usize = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 65_536;
const MAX_MESSAGES: usize = 512;
const MAX_TOOLS: usize = 256;
const MAX_BLOCKS: usize = 1024;
const MAX_ARGUMENTS: usize = 256 * 1024;
const MAX_TEXT: usize = 1024 * 1024;
const MAX_INTEGER: u64 = 9_007_199_254_740_991;

/// 无Clone/Debug/serde。仅弱持原调用/Host/Runtime身份，不延长CallLease或签发会话许可。
pub(in crate::server::core::plugin_host::wasm) struct CatPawMaterial {
    signal: Weak<Signal>,
    host: Weak<Host>,
    shared: Weak<Shared>,
    generation: u64,
    rpc: i64,
    account: String,
    messages: Vec<Value>,
    model: Value,
    // 除model/messages外全部原键，含未知扩展；省略/null/空数组不折成同一状态。
    parameters: Map<String, Value>,
}

impl CatPawMaterial {
    /// 只在原Call真实绑定后、原SourceRef尚未签发且未进入发送准备前封存。
    /// 只读门不freeze、不发新来源；不能在已持SourceGate时反向调用本工厂。
    pub(in crate::server::core::plugin_host::wasm) fn from_call_body(call: &ConversationCall, body: &Value) -> Result<Self, proto::WireError> {
        check_call(call)?;
        validate_body(body)?;
        let object = body.as_object().ok_or_else(invalid)?;
        let messages = object.get("messages").and_then(Value::as_array).ok_or_else(invalid)?;
        let model = object.get("model").ok_or_else(invalid)?;
        let material = Self {
            signal: Arc::downgrade(&call.signal), host: call.host.clone(), shared: call.shared.clone(),
            generation: call.generation, rpc: call.rpc, account: call.account.clone(),
            messages: messages.clone(), model: model.clone(),
            parameters: object.iter().filter(|(key, _)| !matches!(key.as_str(), "model" | "messages"))
                .map(|(key, value)| (key.clone(), value.clone())).collect(),
        };
        // 大材料校验/复制期间若原Call结束，拒绝交付；不将其恢复成另一份有效调用。
        check_call(call)?;
        Ok(material)
    }

    /// 只核物体绑定，不作current准入检查；实际使用时仍须另过原调用最终门。
    pub(in crate::server::core::plugin_host::wasm) fn matches_call(&self, call: &ConversationCall) -> bool {
        call.provider == "catpaw" && !call.account.is_empty() && self.account == call.account
            && self.generation == call.generation && self.rpc == call.rpc
            && Weak::ptr_eq(&self.signal, &Arc::downgrade(&call.signal))
            && Weak::ptr_eq(&self.host, &call.host) && Weak::ptr_eq(&self.shared, &call.shared)
    }
    pub(in crate::server::core::plugin_host::wasm) fn messages(&self) -> &[Value] { &self.messages }
    pub(in crate::server::core::plugin_host::wasm) fn model(&self) -> &Value { &self.model }
    pub(in crate::server::core::plugin_host::wasm) fn parameters(&self) -> &Map<String, Value> { &self.parameters }
    pub(in crate::server::core::plugin_host::wasm) fn parameter(&self, name: &str) -> Option<&Value> { self.parameters.get(name) }

    /// 首片只接受严格完整原历史投影；不剥messageId/finished/reasoning、不合并或补消息。
    /// 新assistant投影和工具回灌匹配另片接；bool只作材料相等，不能恢复原Owner。
    pub(in crate::server::core::plugin_host::wasm) fn matches_history(&self, messages: &Value) -> Result<bool, proto::WireError> {
        check_budget(messages, MAX_BODY, &mut 0)?;
        let mut decoded_nodes = 0;
        validate_messages(messages, &mut decoded_nodes)?;
        Ok(messages.as_array().is_some_and(|messages| messages.as_slice() == self.messages.as_slice()))
    }
    /// 比较所有非历史字段，包括未知扩展与字段存在性，不据此判断发送或会话资格。
    pub(in crate::server::core::plugin_host::wasm) fn matches_constraints(&self, other: &Self) -> bool {
        self.model == other.model && self.parameters == other.parameters
    }
    pub(in crate::server::core::plugin_host::wasm) fn matches_body(&self, body: &Value) -> Result<bool, proto::WireError> {
        validate_body(body)?;
        let object = body.as_object().ok_or_else(invalid)?;
        Ok(object.get("model") == Some(&self.model)
            && object.get("messages").and_then(Value::as_array).is_some_and(|messages| messages.as_slice() == self.messages.as_slice())
            && object.len() == self.parameters.len() + 2
            && self.parameters.iter().all(|(key, value)| object.get(key) == Some(value)))
    }
}

fn check_call(call: &ConversationCall) -> Result<(), proto::WireError> {
    if call.provider != "catpaw" || call.account.is_empty() || call.account.len() > 512
        || call.account.chars().any(char::is_control) || call.rpc <= 0 || call.generation == 0
        || call.signal.ended.is_cancelled() { return Err(unavailable()); }
    let state = call.signal.state.try_lock().map_err(|_| unavailable())?;
    if !state.guard_issued || state.consumer || state.execution.is_some() || state.source_issued
        || state.task_issued || state.http_submission || state.record.is_some() { return Err(unavailable()); }
    call.with_current(|| Ok(())).map_err(|_| unavailable())
}

fn validate_body(body: &Value) -> Result<(), proto::WireError> {
    check_budget(body, MAX_BODY, &mut 0)?;
    let object = body.as_object().ok_or_else(invalid)?;
    // 这些是上游/账号/网络控制，不是请求上下文扩展；即便null也不作为材料来源。
    for field in ["conversationId", "turnRequestId", "modelType", "permissionMode", "source", "mode", "toolVersion",
        "toolConfigs", "availableTools", "repositories", "account", "provider", "proxy", "url", "headers", "credentials",
        "functions", "function_call", "toolCalls"] {
        if object.contains_key(field) { return Err(invalid()); }
    }
    match object.get("model") {
        Some(Value::String(model)) if text_valid(model, 1024, true) => {}
        Some(Value::Number(number)) if number.as_u64().is_some_and(|value| value > 0 && value <= MAX_INTEGER) => {}
        _ => return Err(invalid()),
    }
    let mut decoded_nodes = 0;
    validate_messages(object.get("messages").ok_or_else(invalid)?, &mut decoded_nodes)?;
    let names = validate_tools(object.get("tools"))?;
    validate_choice(object.get("tool_choice"), &names)?;
    for field in ["temperature", "top_p", "presence_penalty", "frequency_penalty"] {
        if let Some(value) = object.get(field).filter(|value| !value.is_null()) {
            if value.as_f64().is_none_or(|value| !value.is_finite()) { return Err(invalid()); }
        }
    }
    for field in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        if object.get(field).is_some_and(|value| !value.is_null() && value.as_u64().is_none_or(|value| value == 0 || value > MAX_INTEGER)) { return Err(invalid()); }
    }
    for field in ["stream", "parallel_tool_calls", "store", "logprobs"] {
        if object.get(field).is_some_and(|value| !value.is_null() && !value.is_boolean()) { return Err(invalid()); }
    }
    if object.get("n").is_some_and(|value| !value.is_null() && value.as_u64() != Some(1)) { return Err(invalid()); }
    if object.get("seed").is_some_and(|value| !value.is_null() && value.as_i64().is_none()) { return Err(invalid()); }
    for field in ["reasoning_effort", "reasoningEffort", "effort"] { optional_text(object.get(field), 64, true)?; }
    same_present_aliases(object, &["reasoning_effort", "reasoningEffort", "effort"])?;
    for field in ["instructions", "system_prompt", "systemPrompt", "rules_message", "rulesMessage", "reasoning_content", "reasoningContent"] {
        optional_text(object.get(field), MAX_TEXT, false)?;
    }
    for field in ["context_window", "contextWindow", "context_length", "contextLength"] { optional_context(object.get(field))?; }
    same_present_aliases(object, &["context_window", "contextWindow", "context_length", "contextLength"])?;
    for field in ["model_params", "modelParams", "request_context", "requestContext"] {
        if let Some(value) = object.get(field).filter(|value| !value.is_null()) { validate_context(value, 0)?; }
    }
    for field in ["reasoning", "thinking", "metadata", "stream_options", "systemPromptContext"] {
        if object.get(field).is_some_and(|value| !value.is_null() && !value.is_object()) { return Err(invalid()); }
    }
    if body.pointer("/stream_options/include_usage").is_some_and(|value| !value.is_boolean()) { return Err(invalid()); }
    if let Some(value) = object.get("stop").filter(|value| !value.is_null()) {
        match value { Value::String(_) => {}, Value::Array(values) if values.len() <= 256 && values.iter().all(Value::is_string) => {}, _ => return Err(invalid()) }
    }
    if let Some(value) = object.get("response_format").filter(|value| !value.is_null()) {
        if !value.is_object() || value["type"] != "text" { return Err(invalid()); }
    }
    if let Some(value) = object.get("logit_bias").filter(|value| !value.is_null()) {
        if value.as_object().is_none_or(|values| values.values().any(|value| value.as_f64().is_none_or(|value| !value.is_finite()))) { return Err(invalid()); }
    }
    // 全部其他字段保留在parameters；未知项不同就不匹配，不宣称插件已经支持。
    Ok(())
}

fn validate_messages(value: &Value, decoded_nodes: &mut usize) -> Result<(), proto::WireError> {
    let messages = value.as_array().filter(|messages| !messages.is_empty() && messages.len() <= MAX_MESSAGES).ok_or_else(invalid)?;
    for message in messages {
        let object = message.as_object().ok_or_else(invalid)?;
        let role = object.get("role").and_then(Value::as_str).filter(|role| matches!(*role, "system" | "developer" | "user" | "assistant" | "tool")).ok_or_else(invalid)?;
        for field in ["id", "messageId", "name"] { optional_text(object.get(field), 256, true)?; }
        if object.get("finished").is_some_and(|value| !value.is_boolean()) { return Err(invalid()); }
        for field in ["reasoning_content", "reasoningContent"] { optional_text(object.get(field), MAX_TEXT, false)?; }
        same_present_aliases(object, &["reasoning_content", "reasoningContent"])?;
        match object.get("content") {
            None | Some(Value::Null) if role == "assistant" => {}
            Some(Value::String(_)) => {}
            Some(Value::Array(blocks)) if blocks.len() <= MAX_BLOCKS => {
                for block in blocks {
                    match block {
                        Value::String(_) => {}
                        Value::Object(block) => {
                            if block.get("type").and_then(Value::as_str) == Some("image_url") {
                                if role != "user" { return Err(invalid()); }
                                super::super::catpaw_images::source(&Value::Object(block.clone()))?;
                                continue;
                            }
                            if !matches!(block.get("type").and_then(Value::as_str), Some("text" | "output_text")) || !block.get("text").is_some_and(Value::is_string) { return Err(invalid()); }
                            for field in ["reasoning_content", "reasoningContent"] { optional_text(block.get(field), MAX_TEXT, false)?; }
                            same_present_aliases(block, &["reasoning_content", "reasoningContent"])?;
                        }
                        _ => return Err(invalid()),
                    }
                }
            }
            _ => return Err(invalid()),
        }
        // 不把其他角色携带的思考/工具字段当成可忽略扩展。
        if role != "assistant" && ["reasoning_content", "reasoningContent"].iter().any(|name| object.get(*name).is_some_and(|value| !value.is_null())) { return Err(invalid()); }
        let calls = match object.get("tool_calls") {
            None | Some(Value::Null) => &[][..],
            Some(Value::Array(calls)) if role == "assistant" && calls.len() <= MAX_TOOLS => calls.as_slice(),
            _ => return Err(invalid()),
        };
        let mut ids = BTreeSet::new();
        for call in calls {
            if !call.is_object() || call["type"] != "function" { return Err(invalid()); }
            let id = call["id"].as_str().filter(|id| text_valid(id, 256, true)).ok_or_else(invalid)?;
            if !ids.insert(id) { return Err(invalid()); }
            let function = call["function"].as_object().ok_or_else(invalid)?;
            function.get("name").and_then(Value::as_str).filter(|name| tool_name(name)).ok_or_else(invalid)?;
            match function.get("arguments") {
                Some(Value::String(arguments)) => { parse_object(arguments, decoded_nodes)?; }
                Some(value @ Value::Object(_)) => { check_budget(value, MAX_ARGUMENTS, &mut 0)?; }
                _ => return Err(invalid()),
            }
        }
        if role == "tool" {
            object.get("tool_call_id").and_then(Value::as_str).filter(|id| text_valid(id, 256, true)).ok_or_else(invalid)?;
        } else if object.get("tool_call_id").is_some_and(|value| !value.is_null()) { return Err(invalid()); }
        if object.get("toolCalls").is_some_and(|value| !value.is_null()) || object.get("function_call").is_some_and(|value| !value.is_null())
            || object.get("toolCallId").is_some_and(|value| !value.is_null()) { return Err(invalid()); }
        // 本片只封存完整实际消息，不建立待响应集合、不补toolName/ID/结果。
    }
    Ok(())
}

fn validate_tools(value: Option<&Value>) -> Result<BTreeSet<String>, proto::WireError> {
    let tools = match value { None | Some(Value::Null) => &[][..], Some(Value::Array(tools)) if tools.len() <= MAX_TOOLS => tools.as_slice(), _ => return Err(invalid()) };
    let mut names = BTreeSet::new();
    for tool in tools {
        if !tool.is_object() || tool["type"] != "function" { return Err(invalid()); }
        let function = tool["function"].as_object().ok_or_else(invalid)?;
        let name = function.get("name").and_then(Value::as_str).filter(|name| tool_name(name)).ok_or_else(invalid)?;
        if !names.insert(name.to_string()) { return Err(invalid()); }
        if function.get("strict").is_some_and(|value| !value.is_null() && value.as_bool() != Some(false)) { return Err(invalid()); }
        optional_text(function.get("description"), MAX_TEXT, false)?;
        if function.get("description").and_then(Value::as_str).is_some_and(|text| text.chars().count() > 8000) { return Err(invalid()); }
        if function.get("parameters").is_some_and(|value| !value.is_null() && !value.is_object()) { return Err(invalid()); }
        // 原定义全部字段仍保存。这里不填默认schema，不按choice裁剪原tools。
    }
    Ok(names)
}

fn validate_choice(value: Option<&Value>, names: &BTreeSet<String>) -> Result<(), proto::WireError> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(value)) if matches!(value.as_str(), "auto" | "none") => Ok(()),
        Some(Value::String(value)) if value == "required" && !names.is_empty() => Ok(()),
        Some(Value::Object(choice)) if choice.get("type").and_then(Value::as_str) == Some("function") => {
            let name = choice.get("function").and_then(Value::as_object).and_then(|function| function.get("name")).and_then(Value::as_str).ok_or_else(invalid)?;
            if tool_name(name) && names.contains(name) { Ok(()) } else { Err(invalid()) }
        }
        _ => Err(invalid()),
    }
}

fn validate_context(value: &Value, depth: usize) -> Result<(), proto::WireError> {
    if depth > MAX_DEPTH { return Err(limit()); }
    let object = value.as_object().ok_or_else(invalid)?;
    optional_context(object.get("context"))?;
    for field in ["effort", "reasoning_effort", "reasoningEffort"] { optional_text(object.get(field), 64, true)?; }
    for field in ["modelParams", "model_params", "declarativeParams", "declarative_params"] {
        if let Some(value) = object.get(field).filter(|value| !value.is_null()) { validate_context(value, depth + 1)?; }
    }
    // 不丢嵌套未知上下文项，它们原样参与参数投影比较。
    Ok(())
}

fn same_present_aliases(object: &Map<String, Value>, names: &[&str]) -> Result<(), proto::WireError> {
    let mut first = None;
    for name in names {
        if let Some(value) = object.get(*name).filter(|value| !value.is_null()) {
            if first.is_some_and(|previous| previous != value) { return Err(invalid()); }
            first = Some(value);
        }
    }
    // 不据已知别名做trim/大小写/数值换算；保守拒绝歧义，所有原键仍完整保存。
    Ok(())
}

fn optional_context(value: Option<&Value>) -> Result<(), proto::WireError> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(value)) if text_valid(value, 64, true) => Ok(()),
        Some(Value::Number(value)) if value.as_u64().is_some_and(|value| value > 0 && value <= MAX_INTEGER) => Ok(()),
        _ => Err(invalid()),
    }
}
fn optional_text(value: Option<&Value>, maximum: usize, nonblank: bool) -> Result<(), proto::WireError> {
    match value { None | Some(Value::Null) => Ok(()), Some(Value::String(value)) if text_valid(value, maximum, nonblank) => Ok(()), _ => Err(invalid()) }
}
fn text_valid(value: &str, maximum: usize, nonblank: bool) -> bool {
    value.len() <= maximum && (!nonblank || !value.trim().is_empty() && !value.chars().any(char::is_control))
}
fn tool_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn check_budget(value: &Value, maximum: usize, nodes: &mut usize) -> Result<(), proto::WireError> {
    walk(value, 0, nodes)?;
    let mut writer = Counter { bytes: 0, maximum };
    serde_json::to_writer(&mut writer, value).map_err(|_| limit())?;
    Ok(())
}
fn walk(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), proto::WireError> {
    *nodes = (*nodes).checked_add(1).filter(|nodes| *nodes <= MAX_NODES).ok_or_else(limit)?;
    if depth > MAX_DEPTH { return Err(limit()); }
    match value {
        Value::Array(items) => { for item in items { walk(item, depth + 1, nodes)?; } }
        Value::Object(object) => {
            for (key, item) in object {
                if key.len() > 256 || matches!(key.as_str(), "__proto__" | "prototype" | "constructor") { return Err(invalid()); }
                walk(item, depth + 1, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}
struct Counter { bytes: usize, maximum: usize }
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.checked_add(bytes.len()).filter(|size| *size <= self.maximum).ok_or_else(|| io::Error::other("catpaw_material_limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

/// body已是Value，原工具参数仍是文本；先核物理深度和解码后重复键再解析，原文不重写。
fn parse_object(text: &str, decoded_nodes: &mut usize) -> Result<Value, proto::WireError> {
    if text.is_empty() || text.len() > MAX_ARGUMENTS { return Err(invalid()); }
    let bytes = text.as_bytes();
    let mut stack: Vec<Option<BTreeSet<String>>> = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        match bytes[offset] {
            b'{' | b'[' => { if stack.len() >= MAX_DEPTH { return Err(limit()); } stack.push((bytes[offset] == b'{').then(BTreeSet::new)); offset += 1; }
            b'}' | b']' => { if stack.pop().is_none() { return Err(invalid()); } offset += 1; }
            b'"' => {
                let begin = offset;
                offset += 1;
                let mut closed = false;
                while offset < bytes.len() {
                    match bytes[offset] { b'\\' => offset = offset.checked_add(2).ok_or_else(invalid)?, b'"' => { offset += 1; closed = true; break; }, _ => offset += 1 }
                }
                if !closed { return Err(invalid()); }
                let mut after = offset;
                while bytes.get(after).is_some_and(u8::is_ascii_whitespace) { after += 1; }
                if bytes.get(after) == Some(&b':') {
                    let key: String = serde_json::from_str(text.get(begin..offset).ok_or_else(invalid)?).map_err(|_| invalid())?;
                    let Some(Some(keys)) = stack.last_mut() else { return Err(invalid()); };
                    if !keys.insert(key) || keys.len() > MAX_NODES { return Err(invalid()); }
                }
            }
            _ => offset += 1,
        }
    }
    if !stack.is_empty() { return Err(invalid()); }
    let value: Value = serde_json::from_str(text).map_err(|_| invalid())?;
    if !value.is_object() { return Err(invalid()); }
    walk(&value, 0, decoded_nodes)?;
    Ok(value)
}

fn invalid() -> proto::WireError {
    proto::WireError::with_code("CatPaw 实际请求材料结构或约束无效，未省略字段、补默认或签发会话许可", "catpaw_call_material_invalid")
}
fn limit() -> proto::WireError {
    proto::WireError::with_code("CatPaw 实际请求材料超过16MiB、深度或节点预算，未截断封存", "catpaw_call_material_limit")
}
fn unavailable() -> proto::WireError {
    proto::WireError::with_code("CatPaw 原实际调用已失效或尚未正确绑定，未用JSON重建请求材料来源", "catpaw_call_material_source_unavailable")
}
