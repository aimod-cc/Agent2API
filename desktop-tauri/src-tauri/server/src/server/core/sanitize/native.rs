use serde_json::Value;

use crate::server::core::protocol::native::WireProtocol;

use super::{sanitize_string, Hits, RuleSet, SanitizeRole, TextScope};

pub fn sanitize_native_body(
    body: &Value,
    protocol: WireProtocol,
    rules: &RuleSet,
) -> Option<(Value, Vec<(String, usize)>)> {
    if rules.entries().iter().all(|rule| !rule.enabled) {
        return None;
    }
    if protocol == WireProtocol::Chat {
        return super::sanitize_body(body, rules);
    }
    let mut next = body.clone();
    let mut hits = Hits::default();
    let mut changed = false;
    let (system, messages) = match protocol {
        WireProtocol::Anthropic => ("system", "messages"),
        WireProtocol::Responses => ("instructions", "input"),
        WireProtocol::Chat => return None,
    };
    if let Some(value) = next.get_mut(system) {
        changed |= content(value, Some(SanitizeRole::System), rules, &mut hits);
    }
    if let Some(input) = next.get_mut(messages) {
        match input {
            Value::String(text) => {
                changed |= sanitize_string(
                    text,
                    rules,
                    &mut hits,
                    scope(Some(SanitizeRole::User), true),
                )
            }
            Value::Array(items) => {
                for item in items {
                    let role = item
                        .get("role")
                        .and_then(Value::as_str)
                        .and_then(SanitizeRole::parse);
                    let kind = item
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if let Some(value) = item.get_mut("content") {
                        changed |= content(value, role, rules, &mut hits);
                    }
                    match kind.as_str() {
                        "function_call" | "custom_tool_call" => {
                            let key = if kind == "function_call" {
                                "arguments"
                            } else {
                                "input"
                            };
                            if let Some(Value::String(text)) = item.get_mut(key) {
                                changed |= sanitize_string(
                                    text,
                                    rules,
                                    &mut hits,
                                    scope(Some(SanitizeRole::Assistant), false),
                                );
                            }
                        }
                        "function_call_output" | "custom_tool_call_output" => {
                            if let Some(value) = item.get_mut("output") {
                                changed |=
                                    content(value, Some(SanitizeRole::Tool), rules, &mut hits);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    changed.then(|| (next, hits.into_sorted()))
}

fn scope(role: Option<SanitizeRole>, content: bool) -> TextScope {
    TextScope { role, content }
}

fn content(
    value: &mut Value,
    role: Option<SanitizeRole>,
    rules: &RuleSet,
    hits: &mut Hits,
) -> bool {
    match value {
        Value::String(text) => sanitize_string(text, rules, hits, scope(role, true)),
        Value::Array(parts) => {
            let mut changed = false;
            for part in parts {
                if let Value::String(text) = part {
                    changed |= sanitize_string(text, rules, hits, scope(role, true));
                    continue;
                }
                match part.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" | "input_text" | "output_text" => {
                        if let Some(Value::String(text)) = part.get_mut("text") {
                            changed |= sanitize_string(text, rules, hits, scope(role, true));
                        }
                    }
                    "tool_result" => {
                        if let Some(value) = part.get_mut("content") {
                            changed |= content(value, Some(SanitizeRole::Tool), rules, hits);
                        }
                    }
                    "tool_use" => {
                        if let Some(input) = part.get_mut("input") {
                            changed |= arguments(input, rules, hits);
                        }
                    }
                    // 思考签名、加密块和未知原生块不参与文本清理。
                    _ => {}
                }
            }
            changed
        }
        _ => false,
    }
}

fn arguments(value: &mut Value, rules: &RuleSet, hits: &mut Hits) -> bool {
    match value {
        Value::String(text) => sanitize_string(
            text,
            rules,
            hits,
            scope(Some(SanitizeRole::Assistant), false),
        ),
        Value::Array(items) => items.iter_mut().fold(false, |changed, value| {
            arguments(value, rules, hits) || changed
        }),
        Value::Object(object) => object.values_mut().fold(false, |changed, value| {
            arguments(value, rules, hits) || changed
        }),
        _ => false,
    }
}
