//! 只恢复本次声明的工具；必须等正常终止帧，才能排除后续真实工具调用和截断。

use std::collections::HashSet;

use serde_json::{json, Map, Value};

const OPEN: &str = "<｜｜DSML｜｜ calls>";
const CLOSE: &str = "</｜｜DSML｜｜ calls>";
const INVOKE_END: &str = "</｜｜DSML｜｜ invoke>";
const PARAM_END: &str = "</｜｜DSML｜｜ parameter>";
const MAX_BUFFER: usize = 256 * 1024;

pub fn declared_tools(body: &Value) -> Vec<String> {
    let choice = body.get("tool_choice");
    if choice
        .and_then(Value::as_str)
        .or_else(|| {
            choice
                .and_then(|choice| choice.get("type"))
                .and_then(Value::as_str)
        })
        .is_some_and(|choice| choice.trim().eq_ignore_ascii_case("none"))
    {
        return Vec::new();
    }
    body.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("function"))
        .filter_map(|tool| tool.pointer("/function/name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Clone, Default)]
pub struct Repair {
    allowed: HashSet<String>,
    held: String,
    blocked: bool,
    finished: bool,
}

impl Repair {
    pub fn new(names: &[String]) -> Self {
        Self {
            allowed: names.iter().cloned().collect(),
            ..Default::default()
        }
    }

    pub fn enabled(&self) -> bool {
        !self.allowed.is_empty()
    }

    pub fn transform(&mut self, chunk: &Value) -> Vec<Value> {
        if !self.enabled() || self.finished {
            return vec![chunk.clone()];
        }
        let choices = chunk.get("choices").and_then(Value::as_array);
        let choice = choices.and_then(|choices| choices.first());
        let delta = choice
            .and_then(|choice| choice.get("delta"))
            .and_then(Value::as_object);
        let finish = choice
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let structured = delta.is_some_and(|fields| {
            fields
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|calls| !calls.is_empty())
                || fields
                    .get("function_call")
                    .is_some_and(|value| !value.is_null())
        });
        if !chunk.is_object()
            || chunk.get("error").is_some()
            || choices.is_some_and(|choices| choices.len() > 1)
            || choice
                .and_then(|choice| choice.get("index"))
                .and_then(Value::as_i64)
                .is_some_and(|index| index != 0)
            || structured
            || finish.is_some_and(|value| value != "stop")
            || delta
                .and_then(|fields| fields.get("content"))
                .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            self.blocked = true;
        }
        if self.blocked {
            let mut out = Vec::new();
            if let Some(frame) = self.flush(chunk) {
                out.push(frame);
            }
            out.push(chunk.clone());
            return out;
        }
        if choices.is_none_or(|choices| choices.is_empty()) {
            return vec![chunk.clone()];
        }
        let content = delta
            .and_then(|fields| fields.get("content"))
            .and_then(Value::as_str);
        if content.is_none() && finish.is_none() {
            return vec![chunk.clone()];
        }
        let mut source = std::mem::take(&mut self.held);
        source.push_str(content.unwrap_or(""));
        let mut visible = String::new();
        if finish == Some("stop") {
            self.finished = true;
            let (text, calls) = if source.len() <= MAX_BUFFER {
                recover(&source, &self.allowed).unwrap_or_else(|| (source, Vec::new()))
            } else {
                (source, Vec::new())
            };
            let mut terminal = chunk.clone();
            if content.is_some() {
                terminal["choices"][0]["delta"]["content"] = json!(&text);
            }
            let mut out = Vec::new();
            if !text.is_empty() && (content.is_none() || !calls.is_empty()) {
                out.push(content_frame(chunk, text));
            }
            if !calls.is_empty() {
                if content.is_some() {
                    terminal["choices"][0]["delta"]["content"] = Value::Null;
                }
                let tool_calls: Vec<Value> = calls.into_iter().enumerate().map(|(index, (name, arguments))| {
                    json!({
                        "index": index,
                        "id": format!("call_{}", super::request::new_message_id()),
                        "type": "function", "function": { "name": name, "arguments": arguments },
                    })
                }).collect();
                let mut tools = content_frame(chunk, String::new());
                tools["choices"][0]["delta"] = json!({ "tool_calls": tool_calls });
                out.push(tools);
                terminal["choices"][0]["finish_reason"] = json!("tool_calls");
            }
            out.push(terminal);
            return out;
        }
        if let Some(start) = source.find(OPEN) {
            visible.push_str(&source[..start]);
            self.held = source[start..].to_string();
        } else {
            let hold = partial_suffix(&source, OPEN);
            visible.push_str(&source[..source.len() - hold]);
            self.held = source[source.len() - hold..].to_string();
        }
        if self.held.len() > MAX_BUFFER {
            visible.push_str(&std::mem::take(&mut self.held));
            self.blocked = true;
        }
        let mut next = chunk.clone();
        next["choices"][0]["delta"]["content"] = json!(visible);
        vec![next]
    }

    pub fn flush(&mut self, template: &Value) -> Option<Value> {
        self.blocked = true;
        (!self.held.is_empty()).then(|| content_frame(template, std::mem::take(&mut self.held)))
    }
}

fn recover(source: &str, allowed: &HashSet<String>) -> Option<(String, Vec<(String, String)>)> {
    let mut rest = source;
    let mut text = String::new();
    let mut calls = Vec::new();
    while let Some(start) = rest.find(OPEN) {
        text.push_str(&rest[..start]);
        let inner = &rest[start + OPEN.len()..];
        let end = inner.find(CLOSE)?;
        calls.extend(parse_block(&inner[..end], allowed)?);
        rest = &inner[end + CLOSE.len()..];
    }
    if partial_suffix(rest, OPEN) > 0 {
        return None;
    }
    text.push_str(rest);
    Some((text, calls))
}

fn content_frame(template: &Value, text: String) -> Value {
    let mut frame = Map::new();
    for key in ["id", "model", "created", "object", "system_fingerprint"] {
        if let Some(value) = template.get(key) {
            frame.insert(key.to_string(), value.clone());
        }
    }
    frame.insert(
        "choices".into(),
        json!([{ "index": 0, "delta": { "content": text }, "finish_reason": null }]),
    );
    Value::Object(frame)
}

fn partial_suffix(text: &str, needle: &str) -> usize {
    (1..needle.len().min(text.len() + 1))
        .rev()
        .find(|length| {
            text.is_char_boundary(text.len() - length)
                && needle.is_char_boundary(*length)
                && text.ends_with(&needle[..*length])
        })
        .unwrap_or(0)
}

fn attributes(tag: &str) -> Option<Map<String, Value>> {
    let mut rest = tag.trim();
    let mut out = Map::new();
    while !rest.is_empty() {
        let split = rest.find('=')?;
        let key = rest[..split].trim();
        if key.is_empty() || out.contains_key(key) {
            return None;
        }
        rest = rest[split + 1..].trim_start().strip_prefix('"')?;
        let end = rest.find('"')?;
        out.insert(key.to_string(), json!(&rest[..end]));
        rest = rest[end + 1..].trim_start();
    }
    Some(out)
}

fn parse_block(mut source: &str, allowed: &HashSet<String>) -> Option<Vec<(String, String)>> {
    let mut calls = Vec::new();
    while !source.trim().is_empty() {
        source = source.trim_start().strip_prefix("<｜｜DSML｜｜ invoke ")?;
        let end = source.find('>')?;
        let attrs = attributes(&source[..end])?;
        let name = attrs.get("name")?.as_str()?;
        if attrs.len() != 1 || !allowed.contains(name) {
            return None;
        }
        source = &source[end + 1..];
        let invoke_end = source.find(INVOKE_END)?;
        let mut params = &source[..invoke_end];
        let mut arguments = Map::new();
        while !params.trim().is_empty() {
            params = params
                .trim_start()
                .strip_prefix("<｜｜DSML｜｜ parameter ")?;
            let head_end = params.find('>')?;
            let attrs = attributes(&params[..head_end])?;
            let key = attrs.get("name")?.as_str()?;
            if key.is_empty()
                || arguments.contains_key(key)
                || attrs.keys().any(|key| key != "name" && key != "string")
            {
                return None;
            }
            params = &params[head_end + 1..];
            let value_end = params.find(PARAM_END)?;
            let raw = &params[..value_end];
            let value = match attrs.get("string").and_then(Value::as_str) {
                Some("true") => json!(raw),
                Some("false") | None => serde_json::from_str(raw.trim()).ok()?,
                _ => return None,
            };
            arguments.insert(key.to_string(), value);
            params = &params[value_end + PARAM_END.len()..];
        }
        calls.push((name.to_string(), serde_json::to_string(&arguments).ok()?));
        source = &source[invoke_end + INVOKE_END.len()..];
    }
    (!calls.is_empty()).then_some(calls)
}
