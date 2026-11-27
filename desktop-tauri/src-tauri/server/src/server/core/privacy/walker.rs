//! 协议字段白名单遍历与还原：
//! - 覆盖 Chat / Anthropic / Responses 的对话、工具参数、工具结果、MCP 块、拒绝回复与工具 Schema 描述；
//! - 工具参数和工具结果若包含 JSON，解析后按敏感字段名整值遮罩；
//! - 支持 JSON 结构化输出（`json_object` / `json_schema`）按 JSON 值还原再重序列化，防原值引号/换行破坏结构。

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::server::core::protocol::native::WireProtocol;

use super::detectors::{
    is_secret_field, mask_text, mask_text_with_field, MaskTextResult, PrivacyHitRecord,
};
use super::rules::{CompiledPrivacySettings, PrivacyAction};
use super::vault::{sorted_counts, PrivacyVault, MAX_RESTORE_EXPANSION};

#[derive(Clone, Debug)]
pub enum PrivacyBodyOutcome {
    Unchanged,
    Warned {
        hits: Vec<PrivacyHitRecord>,
    },
    Masked {
        body: Value,
        hits: Vec<PrivacyHitRecord>,
    },
    Blocked {
        message: String,
        hits: Vec<PrivacyHitRecord>,
    },
}

/// 判断客户端请求是否声明了结构化 JSON 输出模式（用于非流式与流式还原时保持合法 JSON）。
pub fn request_expects_json_output(body: &Value, protocol: WireProtocol) -> bool {
    match protocol {
        WireProtocol::Chat => body
            .pointer("/response_format/type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "json_object" || kind == "json_schema"),
        WireProtocol::Responses => body
            .pointer("/text/format/type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "json_object" || kind == "json_schema"),
        WireProtocol::Anthropic => body
            .pointer("/output_config/format/type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "json_schema"),
    }
}

/// 判断 Responses 请求是否引用了上游有状态会话历史。
pub fn is_stateful_responses_request(body: &Value) -> bool {
    ["previous_response_id", "conversation"]
        .iter()
        .any(|key| body.get(*key).is_some_and(|v| !v.is_null()))
}

#[derive(Default)]
struct HitAccumulator {
    counts: HashMap<(String, PrivacyAction), usize>,
    blocked_message: Option<String>,
    masked_any: bool,
}

impl HitAccumulator {
    fn absorb(
        &mut self,
        hits: Vec<PrivacyHitRecord>,
        blocked_message: Option<String>,
        masked: bool,
    ) {
        for hit in hits {
            *self.counts.entry((hit.kind, hit.action)).or_insert(0) += hit.count;
        }
        if self.blocked_message.is_none() {
            self.blocked_message = blocked_message;
        }
        self.masked_any |= masked;
    }

    fn into_hits(self) -> Vec<PrivacyHitRecord> {
        let mut list: Vec<PrivacyHitRecord> = self
            .counts
            .into_iter()
            .map(|((kind, action), count)| PrivacyHitRecord {
                kind,
                count,
                action,
            })
            .collect();
        list.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.kind.cmp(&right.kind))
        });
        list
    }
}

fn process_string_inner(
    text: &mut String,
    secret_field: Option<&str>,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> (bool, bool) {
    let result = match secret_field {
        Some(field) => mask_text_with_field(text, Some(field), compiled, vault),
        None => mask_text(text, compiled, vault),
    };
    match result {
        MaskTextResult::Unchanged => (false, false),
        MaskTextResult::Handled {
            text: replacement,
            hits,
            blocked_message,
        } => {
            let changed = replacement.is_some();
            if let Some(next) = replacement {
                *text = next;
            }
            acc.absorb(hits, blocked_message, changed);
            (acc.blocked_message.is_some(), changed)
        }
    }
}

fn process_string(
    text: &mut String,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    process_string_inner(text, None, compiled, vault, acc).0
}

/// 递归处理工具参数或工具结果中的 JSON 值：键名命中敏感字段时整值遮罩。
/// 返回 `(blocked, modified)`。
fn process_tool_json_value(
    value: &mut Value,
    field: Option<&str>,
    in_secret: bool,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> (bool, bool) {
    let secret_ctx = in_secret || field.is_some_and(is_secret_field);
    match value {
        Value::String(text) => {
            let effective_field = if secret_ctx {
                field.or(Some("secret"))
            } else {
                None
            };
            process_string_inner(text, effective_field, compiled, vault, acc)
        }
        Value::Array(items) => {
            let mut modified = false;
            for item in items {
                let (blocked, changed) =
                    process_tool_json_value(item, field, secret_ctx, compiled, vault, acc);
                modified |= changed;
                if blocked {
                    return (true, modified);
                }
            }
            (false, modified)
        }
        Value::Object(map) => {
            let mut modified = false;
            for (key, val) in map.iter_mut() {
                let (blocked, changed) =
                    process_tool_json_value(val, Some(key.as_str()), secret_ctx, compiled, vault, acc);
                modified |= changed;
                if blocked {
                    return (true, modified);
                }
            }
            (false, modified)
        }
        _ => (false, false),
    }
}

/// 尝试将工具参数/工具结果字符串按 JSON 解析并做字段名感知脱敏；非 JSON 时退回普通文本脱敏。
fn process_arguments_or_json_string(
    value: &mut Value,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    match value {
        Value::String(s) => {
            let trimmed = s.trim_start();
            if trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.starts_with('"') {
                if let Ok(mut decoded @ (Value::Object(_) | Value::Array(_) | Value::String(_))) =
                    serde_json::from_str::<Value>(s)
                {
                    let (blocked, modified) =
                        process_tool_json_value(&mut decoded, None, false, compiled, vault, acc);
                    if blocked {
                        return true;
                    }
                    if modified {
                        if let Ok(encoded) = serde_json::to_string(&decoded) {
                            *s = encoded;
                        }
                    }
                    return false;
                }
            }
            process_string(s, compiled, vault, acc)
        }
        other => process_tool_json_value(other, None, false, compiled, vault, acc).0,
    }
}

fn process_result_content(
    value: &mut Value,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    match value {
        Value::String(_) => process_arguments_or_json_string(value, compiled, vault, acc),
        Value::Array(parts) => {
            for part in parts {
                if part.is_string() {
                    if process_arguments_or_json_string(part, compiled, vault, acc) {
                        return true;
                    }
                    continue;
                }
                let kind = part
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if matches!(kind.as_str(), "" | "text" | "input_text" | "output_text") {
                    if let Some(text) = part.get_mut("text") {
                        if process_arguments_or_json_string(text, compiled, vault, acc) {
                            return true;
                        }
                    }
                } else if process_block(part, compiled, vault, acc) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn process_text_parts(
    value: &mut Value,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    match value {
        Value::String(text) => process_string(text, compiled, vault, acc),
        Value::Array(parts) => {
            for part in parts {
                if let Value::String(text) = part {
                    if process_string(text, compiled, vault, acc) {
                        return true;
                    }
                    continue;
                }
                if process_block(part, compiled, vault, acc) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn process_block(
    block: &mut Value,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    let kind = block
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match kind.as_str() {
        "text" | "input_text" | "output_text" => {
            if let Some(Value::String(text)) = block.get_mut("text") {
                return process_string(text, compiled, vault, acc);
            }
        }
        "refusal" => {
            if let Some(Value::String(text)) = block.get_mut("refusal") {
                return process_string(text, compiled, vault, acc);
            }
        }
        "tool_use" | "server_tool_use" | "mcp_tool_use" => {
            if let Some(input) = block.get_mut("input") {
                return process_arguments_or_json_string(input, compiled, vault, acc);
            }
        }
        "mcp_approval_request" | "function_call" | "mcp_call" => {
            if let Some(args) = block.get_mut("arguments") {
                return process_arguments_or_json_string(args, compiled, vault, acc);
            }
        }
        "custom_tool_call" => {
            if let Some(Value::String(text)) = block.get_mut("input") {
                return process_string(text, compiled, vault, acc);
            }
        }
        "tool_result" | "mcp_tool_result" => {
            if let Some(content) = block.get_mut("content") {
                return process_result_content(content, compiled, vault, acc);
            }
        }
        "function_call_output" | "custom_tool_call_output" => {
            if let Some(output) = block.get_mut("output") {
                return process_result_content(output, compiled, vault, acc);
            }
        }
        item if item.eq_ignore_ascii_case("additional_tools") || item == "tool_search_output" => {
            if let Some(tools) = block.get_mut("tools").and_then(Value::as_array_mut) {
                for tool in tools {
                    if process_tool_definition(tool, compiled, vault, acc) {
                        return true;
                    }
                }
            }
        }
        "message" => return process_chat_message(block, compiled, vault, acc),
        "" if block.get("role").is_some() => {
            return process_chat_message(block, compiled, vault, acc);
        }
        "" => {
            if let Some(Value::String(text)) = block.get_mut("text") {
                return process_string(text, compiled, vault, acc);
            }
        }
        // thinking / redacted_thinking / reasoning / image / document / signature 保持原样
        _ => {}
    }
    false
}

fn process_chat_message(
    message: &mut Value,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    let is_tool_role = matches!(
        message.get("role").and_then(Value::as_str),
        Some("tool" | "function")
    );
    let Some(obj) = message.as_object_mut() else {
        return false;
    };
    if let Some(content) = obj.get_mut("content") {
        let blocked = if is_tool_role {
            process_result_content(content, compiled, vault, acc)
        } else {
            process_text_parts(content, compiled, vault, acc)
        };
        if blocked {
            return true;
        }
    }
    if let Some(Value::String(refusal)) = obj.get_mut("refusal") {
        if process_string(refusal, compiled, vault, acc) {
            return true;
        }
    }
    if !obj.contains_key("signature") {
        if let Some(Value::String(reasoning)) = obj.get_mut("reasoning_content") {
            if process_string(reasoning, compiled, vault, acc) {
                return true;
            }
        }
    }
    if let Some(args) = obj
        .get_mut("function_call")
        .and_then(|fc| fc.get_mut("arguments"))
    {
        if process_arguments_or_json_string(args, compiled, vault, acc) {
            return true;
        }
    }
    if let Some(calls) = obj.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for call in calls {
            if let Some(args) = call
                .get_mut("function")
                .and_then(|f| f.get_mut("arguments"))
            {
                if process_arguments_or_json_string(args, compiled, vault, acc) {
                    return true;
                }
            }
            if let Some(Value::String(input)) =
                call.get_mut("custom").and_then(|c| c.get_mut("input"))
            {
                if process_string(input, compiled, vault, acc) {
                    return true;
                }
            }
        }
    }
    false
}

fn process_tool_definition(
    tool: &mut Value,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    if let Some(Value::String(desc)) = tool.get_mut("description") {
        if process_string(desc, compiled, vault, acc) {
            return true;
        }
    }
    if let Some(Value::String(desc)) = tool.pointer_mut("/function/description") {
        if process_string(desc, compiled, vault, acc) {
            return true;
        }
    }
    for path in ["/parameters", "/input_schema", "/function/parameters"] {
        if let Some(schema) = tool.pointer_mut(path) {
            if process_schema(schema, compiled, vault, acc) {
                return true;
            }
        }
    }
    if tool
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t.eq_ignore_ascii_case("namespace"))
    {
        if let Some(children) = tool.get_mut("tools").and_then(Value::as_array_mut) {
            for child in children {
                if process_tool_definition(child, compiled, vault, acc) {
                    return true;
                }
            }
        }
    }
    false
}

fn process_schema(
    schema: &mut Value,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    if let Some(Value::String(desc)) = schema.get_mut("description") {
        if process_string(desc, compiled, vault, acc) {
            return true;
        }
    }
    if let Some(Value::String(title)) = schema.get_mut("title") {
        if process_string(title, compiled, vault, acc) {
            return true;
        }
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(Value::Object(fields)) = schema.get_mut(key) {
            for field in fields.values_mut() {
                if process_schema(field, compiled, vault, acc) {
                    return true;
                }
            }
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(Value::Array(items)) = schema.get_mut(key) {
            for item in items {
                if process_schema(item, compiled, vault, acc) {
                    return true;
                }
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "not",
        "if",
        "then",
        "else",
        "contains",
    ] {
        if let Some(sub) = schema.get_mut(key) {
            if sub.is_object() && process_schema(sub, compiled, vault, acc) {
                return true;
            }
        }
    }
    false
}

pub fn mask_chat_body(
    body: &Value,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
) -> PrivacyBodyOutcome {
    if !compiled.settings.enabled || !body.get("messages").is_some_and(Value::is_array) {
        return PrivacyBodyOutcome::Unchanged;
    }
    let mut next = body.clone();
    let mut acc = HitAccumulator::default();
    if let Some(messages) = next.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            if process_chat_message(message, compiled, vault, &mut acc) {
                return finalize_outcome(next, acc);
            }
        }
    }
    for key in ["tools", "functions"] {
        if let Some(tools) = next.get_mut(key).and_then(Value::as_array_mut) {
            for tool in tools {
                if process_tool_definition(tool, compiled, vault, &mut acc) {
                    return finalize_outcome(next, acc);
                }
            }
        }
    }
    finalize_outcome(next, acc)
}

pub fn mask_native_body(
    body: &Value,
    protocol: WireProtocol,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
) -> PrivacyBodyOutcome {
    if !compiled.settings.enabled {
        return PrivacyBodyOutcome::Unchanged;
    }
    if protocol == WireProtocol::Chat {
        return mask_chat_body(body, compiled, vault);
    }
    let mut next = body.clone();
    let mut acc = HitAccumulator::default();
    match protocol {
        WireProtocol::Anthropic => {
            if let Some(system) = next.get_mut("system") {
                if process_text_parts(system, compiled, vault, &mut acc) {
                    return finalize_outcome(next, acc);
                }
            }
            if let Some(messages) = next.get_mut("messages").and_then(Value::as_array_mut) {
                for message in messages {
                    if let Some(content) = message.get_mut("content") {
                        if process_text_parts(content, compiled, vault, &mut acc) {
                            return finalize_outcome(next, acc);
                        }
                    }
                }
            }
        }
        WireProtocol::Responses => {
            if let Some(instructions) = next.get_mut("instructions") {
                if process_text_parts(instructions, compiled, vault, &mut acc) {
                    return finalize_outcome(next, acc);
                }
            }
            if let Some(input) = next.get_mut("input") {
                if process_text_parts(input, compiled, vault, &mut acc) {
                    return finalize_outcome(next, acc);
                }
            }
        }
        WireProtocol::Chat => {}
    }
    if let Some(tools) = next.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            if process_tool_definition(tool, compiled, vault, &mut acc) {
                return finalize_outcome(next, acc);
            }
        }
    }
    finalize_outcome(next, acc)
}

fn finalize_outcome(next: Value, mut acc: HitAccumulator) -> PrivacyBodyOutcome {
    if let Some(message) = acc.blocked_message.take() {
        return PrivacyBodyOutcome::Blocked {
            message,
            hits: acc.into_hits(),
        };
    }
    let masked = acc.masked_any;
    let hits = acc.into_hits();
    if hits.is_empty() {
        PrivacyBodyOutcome::Unchanged
    } else if masked {
        PrivacyBodyOutcome::Masked { body: next, hits }
    } else {
        PrivacyBodyOutcome::Warned { hits }
    }
}

// ─── 响应侧安全还原（含结构化 JSON 输出模式）─────────────────────

fn restore_string_slot(
    text: &mut String,
    vault: &PrivacyVault,
    json_string_escape: bool,
    counts: &mut HashMap<String, usize>,
    budget: &mut usize,
) -> bool {
    if !text.contains("{{") {
        return false;
    }
    let (restored, hits) = vault.restore_text_with_budget(text, json_string_escape, budget);
    if !hits.is_empty() {
        *text = restored;
        for (kind, count) in hits {
            *counts.entry(kind).or_insert(0) += count;
        }
        true
    } else {
        false
    }
}

/// 递归还原已解析 JSON 树中的字符串值与对象键名；若发生还原后键名碰撞返回 `Err(())`。
fn restore_json_tree(
    value: &mut Value,
    vault: &PrivacyVault,
    counts: &mut HashMap<String, usize>,
    budget: &mut usize,
) -> Result<bool, ()> {
    match value {
        Value::String(text) => Ok(restore_string_slot(text, vault, false, counts, budget)),
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= restore_json_tree(item, vault, counts, budget)?;
            }
            Ok(changed)
        }
        Value::Object(fields) => {
            let old = std::mem::take(fields);
            let mut restored = Map::with_capacity(old.len());
            let mut changed = false;
            for (mut key, mut item) in old {
                changed |= restore_string_slot(&mut key, vault, false, counts, budget);
                changed |= restore_json_tree(&mut item, vault, counts, budget)?;
                if restored.insert(key, item).is_some() {
                    return Err(());
                }
            }
            *fields = restored;
            Ok(changed)
        }
        _ => Ok(false),
    }
}

/// 还原承载 JSON 内容的字符串（结构化 JSON 输出或工具调用 arguments）：
/// 优先解析后按 JSON 值还原再重序列化，保证含引号/反斜杠/换行的原值不破坏 JSON 语法；
/// 解析失败或键名冲突时回退到 JSON 字符串转义替换。
pub(super) fn restore_json_encoded_string(
    text: &mut String,
    vault: &PrivacyVault,
    counts: &mut HashMap<String, usize>,
    budget: &mut usize,
) -> bool {
    if !text.contains("{{") {
        return false;
    }
    if let Ok(mut decoded) = serde_json::from_str::<Value>(text) {
        let mut local_counts = HashMap::new();
        let mut local_budget = *budget;
        if let Ok(changed) = restore_json_tree(&mut decoded, vault, &mut local_counts, &mut local_budget)
        {
            if !changed {
                return false;
            }
            if let Ok(encoded) = serde_json::to_string(&decoded) {
                *text = encoded;
                *budget = local_budget;
                for (kind, count) in local_counts {
                    *counts.entry(kind).or_insert(0) += count;
                }
                return true;
            }
        }
    }
    restore_string_slot(text, vault, true, counts, budget)
}

fn restore_output_text(
    text: &mut String,
    vault: &PrivacyVault,
    json_output: bool,
    counts: &mut HashMap<String, usize>,
    budget: &mut usize,
) {
    if json_output {
        let _ = restore_json_encoded_string(text, vault, counts, budget);
    } else {
        let _ = restore_string_slot(text, vault, false, counts, budget);
    }
}

pub fn unmask_json_response(
    body: &mut Value,
    protocol: WireProtocol,
    vault: &PrivacyVault,
) -> Vec<(String, usize)> {
    unmask_json_response_with_mode(body, protocol, vault, false)
}

pub fn unmask_json_response_with_mode(
    body: &mut Value,
    protocol: WireProtocol,
    vault: &PrivacyVault,
    json_output: bool,
) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut budget = MAX_RESTORE_EXPANSION;
    match protocol {
        WireProtocol::Chat => {
            if let Some(choices) = body.get_mut("choices").and_then(Value::as_array_mut) {
                for choice in choices {
                    if let Some(msg) = choice.get_mut("message").and_then(Value::as_object_mut) {
                        unmask_chat_message(msg, vault, json_output, &mut counts, &mut budget);
                    }
                    if let Some(delta) = choice.get_mut("delta").and_then(Value::as_object_mut) {
                        unmask_chat_message(delta, vault, json_output, &mut counts, &mut budget);
                    }
                }
            }
        }
        WireProtocol::Anthropic => {
            if let Some(parts) = body.get_mut("content").and_then(Value::as_array_mut) {
                unmask_anthropic_blocks(parts, vault, json_output, &mut counts, &mut budget);
            }
            if let Some(parts) = body
                .pointer_mut("/message/content")
                .and_then(Value::as_array_mut)
            {
                unmask_anthropic_blocks(parts, vault, json_output, &mut counts, &mut budget);
            }
        }
        WireProtocol::Responses => {
            if let Some(Value::String(text)) = body.get_mut("output_text") {
                restore_output_text(text, vault, json_output, &mut counts, &mut budget);
            }
            if let Some(Value::String(inst)) = body.get_mut("instructions") {
                let _ = restore_string_slot(inst, vault, false, &mut counts, &mut budget);
            }
            if let Some(output) = body.get_mut("output").and_then(Value::as_array_mut) {
                for item in output {
                    unmask_responses_item(item, vault, json_output, &mut counts, &mut budget);
                }
            }
        }
    }
    sorted_counts(counts)
}

fn unmask_anthropic_blocks(
    parts: &mut [Value],
    vault: &PrivacyVault,
    json_output: bool,
    counts: &mut HashMap<String, usize>,
    budget: &mut usize,
) {
    for part in parts {
        let kind = part
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match kind.as_str() {
            "text" | "output_text" => {
                if let Some(Value::String(text)) = part.get_mut("text") {
                    restore_output_text(text, vault, json_output, counts, budget);
                }
            }
            "refusal" => {
                if let Some(Value::String(text)) = part.get_mut("refusal") {
                    let _ = restore_string_slot(text, vault, false, counts, budget);
                }
            }
            "tool_use" | "server_tool_use" | "mcp_tool_use" => {
                if let Some(input) = part.get_mut("input") {
                    let _ = restore_json_tree(input, vault, counts, budget);
                }
            }
            _ => {}
        }
    }
}

pub(super) fn unmask_responses_item(
    item: &mut Value,
    vault: &PrivacyVault,
    json_output: bool,
    counts: &mut HashMap<String, usize>,
    budget: &mut usize,
) {
    let kind = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if kind == "reasoning" {
        return;
    }
    if let Some(parts) = item.get_mut("content").and_then(Value::as_array_mut) {
        for part in parts {
            let part_kind = part.get("type").and_then(Value::as_str).unwrap_or("");
            match part_kind {
                "" | "text" | "output_text" => {
                    if let Some(Value::String(text)) = part.get_mut("text") {
                        restore_output_text(text, vault, json_output, counts, budget);
                    }
                }
                "input_text" => {
                    if let Some(Value::String(text)) = part.get_mut("text") {
                        let _ = restore_string_slot(text, vault, false, counts, budget);
                    }
                }
                "refusal" => {
                    if let Some(Value::String(text)) = part.get_mut("refusal") {
                        let _ = restore_string_slot(text, vault, false, counts, budget);
                    }
                }
                _ => {}
            }
        }
    }
    match kind.as_str() {
        "function_call" | "mcp_call" | "mcp_approval_request" => {
            if let Some(Value::String(args)) = item.get_mut("arguments") {
                let _ = restore_json_encoded_string(args, vault, counts, budget);
            }
        }
        "custom_tool_call" => {
            if let Some(Value::String(input)) = item.get_mut("input") {
                let _ = restore_string_slot(input, vault, false, counts, budget);
            }
        }
        _ => {}
    }
}

fn unmask_chat_message(
    msg: &mut Map<String, Value>,
    vault: &PrivacyVault,
    json_output: bool,
    counts: &mut HashMap<String, usize>,
    budget: &mut usize,
) {
    if let Some(content) = msg.get_mut("content") {
        match content {
            Value::String(text) => restore_output_text(text, vault, json_output, counts, budget),
            Value::Array(parts) => {
                for part in parts {
                    let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
                    match kind {
                        "" | "text" | "output_text" => {
                            if let Some(Value::String(text)) = part.get_mut("text") {
                                restore_output_text(text, vault, json_output, counts, budget);
                            }
                        }
                        "refusal" => {
                            if let Some(Value::String(text)) = part.get_mut("refusal") {
                                let _ = restore_string_slot(text, vault, false, counts, budget);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(Value::String(refusal)) = msg.get_mut("refusal") {
        let _ = restore_string_slot(refusal, vault, false, counts, budget);
    }
    if !msg.contains_key("signature") {
        if let Some(Value::String(reasoning)) = msg.get_mut("reasoning_content") {
            let _ = restore_string_slot(reasoning, vault, false, counts, budget);
        }
    }
    if let Some(Value::String(args)) = msg
        .get_mut("function_call")
        .and_then(|fc| fc.get_mut("arguments"))
    {
        let _ = restore_json_encoded_string(args, vault, counts, budget);
    }
    if let Some(calls) = msg.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for call in calls {
            if let Some(Value::String(args)) = call
                .get_mut("function")
                .and_then(|f| f.get_mut("arguments"))
            {
                let _ = restore_json_encoded_string(args, vault, counts, budget);
            }
            if let Some(Value::String(input)) =
                call.get_mut("custom").and_then(|c| c.get_mut("input"))
            {
                let _ = restore_string_slot(input, vault, false, counts, budget);
            }
        }
    }
}
