use std::collections::HashMap;

use serde_json::Value;

use crate::server::core::protocol::native::WireProtocol;

use super::detectors::{mask_text, MaskTextResult, PrivacyHitRecord};
use super::rules::{CompiledPrivacySettings, PrivacyAction};
use super::vault::{sorted_counts, PrivacyVault};

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

#[derive(Default)]
struct HitAccumulator {
    counts: HashMap<(String, PrivacyAction), usize>,
    blocked_message: Option<String>,
    masked_any: bool,
}

impl HitAccumulator {
    fn absorb(&mut self, hits: Vec<PrivacyHitRecord>, blocked_message: Option<String>, masked: bool) {
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

fn process_string(
    text: &mut String,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    match mask_text(text, compiled, vault) {
        MaskTextResult::Unchanged => false,
        MaskTextResult::Handled {
            text: replacement,
            hits,
            blocked_message,
        } => {
            let masked = replacement.is_some();
            if let Some(next) = replacement {
                *text = next;
            }
            acc.absorb(hits, blocked_message, masked);
            acc.blocked_message.is_some()
        }
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
                let Some(obj) = part.as_object_mut() else {
                    continue;
                };
                let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(kind, "" | "text" | "input_text" | "output_text") {
                    if let Some(Value::String(text)) = obj.get_mut("text") {
                        if process_string(text, compiled, vault, acc) {
                            return true;
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

fn process_value_strings(
    value: &mut Value,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
    acc: &mut HitAccumulator,
) -> bool {
    match value {
        Value::String(text) => process_string(text, compiled, vault, acc),
        Value::Array(items) => {
            for item in items {
                if process_value_strings(item, compiled, vault, acc) {
                    return true;
                }
            }
            false
        }
        Value::Object(map) => {
            for val in map.values_mut() {
                if process_value_strings(val, compiled, vault, acc) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
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
            let Some(obj) = message.as_object_mut() else {
                continue;
            };
            if let Some(content) = obj.get_mut("content") {
                if process_text_parts(content, compiled, vault, &mut acc) {
                    break;
                }
            }
            if !obj.contains_key("signature") {
                if let Some(Value::String(reasoning)) = obj.get_mut("reasoning_content") {
                    if process_string(reasoning, compiled, vault, &mut acc) {
                        break;
                    }
                }
            }
            if let Some(calls) = obj.get_mut("tool_calls").and_then(Value::as_array_mut) {
                let mut stop = false;
                for call in calls {
                    if let Some(Value::String(args)) = call
                        .get_mut("function")
                        .and_then(|f| f.get_mut("arguments"))
                    {
                        if process_string(args, compiled, vault, &mut acc) {
                            stop = true;
                            break;
                        }
                    }
                }
                if stop {
                    break;
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
                    let Some(content) = message.get_mut("content") else {
                        continue;
                    };
                    if mask_anthropic_content(content, compiled, vault, &mut acc) {
                        return finalize_outcome(next, acc);
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
                match input {
                    Value::String(text) => {
                        let _ = process_string(text, compiled, vault, &mut acc);
                    }
                    Value::Array(items) => {
                        for item in items {
                            let kind = item
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if kind == "reasoning" {
                                continue;
                            }
                            if let Some(content) = item.get_mut("content") {
                                if process_text_parts(content, compiled, vault, &mut acc) {
                                    return finalize_outcome(next, acc);
                                }
                            }
                            match kind.as_str() {
                                "function_call" => {
                                    if let Some(Value::String(args)) = item.get_mut("arguments") {
                                        if process_string(args, compiled, vault, &mut acc) {
                                            return finalize_outcome(next, acc);
                                        }
                                    }
                                }
                                "custom_tool_call" => {
                                    if let Some(Value::String(text)) = item.get_mut("input") {
                                        if process_string(text, compiled, vault, &mut acc) {
                                            return finalize_outcome(next, acc);
                                        }
                                    }
                                }
                                "function_call_output" | "custom_tool_call_output" => {
                                    if let Some(output) = item.get_mut("output") {
                                        if process_text_parts(output, compiled, vault, &mut acc) {
                                            return finalize_outcome(next, acc);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        WireProtocol::Chat => {}
    }
    finalize_outcome(next, acc)
}

fn mask_anthropic_content(
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
                let kind = part
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                match kind.as_str() {
                    "text" | "input_text" | "output_text" => {
                        if let Some(Value::String(text)) = part.get_mut("text") {
                            if process_string(text, compiled, vault, acc) {
                                return true;
                            }
                        }
                    }
                    "tool_result" => {
                        if let Some(inner) = part.get_mut("content") {
                            if mask_anthropic_content(inner, compiled, vault, acc) {
                                return true;
                            }
                        }
                    }
                    "tool_use" => {
                        if let Some(input) = part.get_mut("input") {
                            if process_value_strings(input, compiled, vault, acc) {
                                return true;
                            }
                        }
                    }
                    // thinking / redacted_thinking / image / document 一律保持原样
                    _ => {}
                }
            }
            false
        }
        _ => false,
    }
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

fn restore_string_slot(
    text: &mut String,
    vault: &PrivacyVault,
    json_string_escape: bool,
    counts: &mut HashMap<String, usize>,
) {
    if !text.contains("{{") {
        return;
    }
    let (restored, hits) = vault.restore_text(text, json_string_escape);
    if !hits.is_empty() {
        *text = restored;
        for (kind, count) in hits {
            *counts.entry(kind).or_insert(0) += count;
        }
    }
}

fn restore_value_strings(
    value: &mut Value,
    vault: &PrivacyVault,
    counts: &mut HashMap<String, usize>,
) {
    match value {
        Value::String(text) => restore_string_slot(text, vault, false, counts),
        Value::Array(items) => {
            for item in items {
                restore_value_strings(item, vault, counts);
            }
        }
        Value::Object(map) => {
            for val in map.values_mut() {
                restore_value_strings(val, vault, counts);
            }
        }
        _ => {}
    }
}

pub fn unmask_json_response(
    body: &mut Value,
    protocol: WireProtocol,
    vault: &PrivacyVault,
) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    match protocol {
        WireProtocol::Chat => {
            if let Some(choices) = body.get_mut("choices").and_then(Value::as_array_mut) {
                for choice in choices {
                    if let Some(msg) = choice.get_mut("message").and_then(Value::as_object_mut) {
                        unmask_chat_message(msg, vault, &mut counts);
                    }
                    if let Some(delta) = choice.get_mut("delta").and_then(Value::as_object_mut) {
                        unmask_chat_message(delta, vault, &mut counts);
                    }
                }
            }
        }
        WireProtocol::Anthropic => {
            if let Some(parts) = body.get_mut("content").and_then(Value::as_array_mut) {
                for part in parts {
                    let kind = part
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    match kind.as_str() {
                        "text" | "output_text" => {
                            if let Some(Value::String(text)) = part.get_mut("text") {
                                restore_string_slot(text, vault, false, &mut counts);
                            }
                        }
                        "tool_use" => {
                            if let Some(input) = part.get_mut("input") {
                                restore_value_strings(input, vault, &mut counts);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        WireProtocol::Responses => {
            if let Some(Value::String(text)) = body.get_mut("output_text") {
                restore_string_slot(text, vault, false, &mut counts);
            }
            if let Some(output) = body.get_mut("output").and_then(Value::as_array_mut) {
                for item in output {
                    let kind = item
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if kind == "reasoning" {
                        continue;
                    }
                    if let Some(parts) = item.get_mut("content").and_then(Value::as_array_mut) {
                        for part in parts {
                            let part_kind = part.get("type").and_then(Value::as_str).unwrap_or("");
                            if matches!(part_kind, "" | "text" | "output_text" | "input_text") {
                                if let Some(Value::String(text)) = part.get_mut("text") {
                                    restore_string_slot(text, vault, false, &mut counts);
                                }
                            }
                        }
                    }
                    match kind.as_str() {
                        "function_call" => {
                            if let Some(Value::String(args)) = item.get_mut("arguments") {
                                restore_string_slot(args, vault, true, &mut counts);
                            }
                        }
                        "custom_tool_call" => {
                            if let Some(Value::String(input)) = item.get_mut("input") {
                                restore_string_slot(input, vault, false, &mut counts);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    sorted_counts(counts)
}

fn unmask_chat_message(
    msg: &mut serde_json::Map<String, Value>,
    vault: &PrivacyVault,
    counts: &mut HashMap<String, usize>,
) {
    if let Some(content) = msg.get_mut("content") {
        match content {
            Value::String(text) => restore_string_slot(text, vault, false, counts),
            Value::Array(parts) => {
                for part in parts {
                    let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
                    if matches!(kind, "" | "text" | "output_text") {
                        if let Some(Value::String(text)) = part.get_mut("text") {
                            restore_string_slot(text, vault, false, counts);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !msg.contains_key("signature") {
        if let Some(Value::String(reasoning)) = msg.get_mut("reasoning_content") {
            restore_string_slot(reasoning, vault, false, counts);
        }
    }
    if let Some(calls) = msg.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for call in calls {
            if let Some(Value::String(args)) = call
                .get_mut("function")
                .and_then(|f| f.get_mut("arguments"))
            {
                restore_string_slot(args, vault, true, counts);
            }
        }
    }
}
