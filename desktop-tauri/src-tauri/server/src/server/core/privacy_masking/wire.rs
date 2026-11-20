//! 协议字段白名单：层级明确，避免遍历 ID、签名、附件、认证或任意 metadata。
use serde_json::Value;

use super::{is_secret_field, Mask, Restore};

#[derive(Clone, Copy)]
pub(crate) enum Format {
    Chat,
    Responses,
    Messages,
}

type Result = std::result::Result<(), ()>;

fn each(value: Option<&mut Value>, mut visit: impl FnMut(&mut Value) -> Result) -> Result {
    if let Some(Value::Array(items)) = value {
        for item in items {
            visit(item)?;
        }
    }
    Ok(())
}

fn text(value: Option<&mut Value>, mut visit: impl FnMut(&mut String) -> Result) -> Result {
    if let Some(Value::String(value)) = value {
        visit(value)?;
    }
    Ok(())
}

fn kind(value: &Value) -> &str {
    value.get("type").and_then(Value::as_str).unwrap_or("")
}

enum Transform<'a, 'r> {
    Mask(&'a mut Mask<'r>),
    Restore(&'a mut Restore),
}

impl Transform<'_, '_> {
    fn text(&mut self, value: Option<&mut Value>) -> Result {
        text(value, |s| match self {
            Self::Mask(mask) => mask.apply(s, false),
            Self::Restore(restore) => restore.apply(s, false),
        })
    }

    fn arguments(&mut self, value: Option<&mut Value>) -> Result {
        let Some(value) = value else { return Ok(()) };
        if let Value::String(s) = value {
            if let Ok(mut decoded) = serde_json::from_str::<Value>(s) {
                let original = decoded.clone();
                self.tool_values(&mut decoded, false)?;
                if original != decoded {
                    *s = serde_json::to_string(&decoded).map_err(|_| ())?;
                }
            } else {
                self.text(Some(value))?;
            }
        } else {
            self.tool_values(value, false)?;
        }
        Ok(())
    }

    fn tool_values(&mut self, value: &mut Value, secret_field: bool) -> Result {
        match value {
            Value::String(s) => match self {
                Self::Mask(mask) => mask.apply(s, secret_field),
                Self::Restore(restore) => restore.apply(s, false),
            },
            Value::Array(items) => items
                .iter_mut()
                .try_for_each(|v| self.tool_values(v, secret_field)),
            Value::Object(fields) => fields
                .iter_mut()
                .try_for_each(|(key, v)| self.tool_values(v, secret_field || is_secret_field(key))),
            _ => Ok(()),
        }
    }

    fn content(&mut self, value: Option<&mut Value>) -> Result {
        match value {
            Some(value @ Value::String(_)) => self.text(Some(value)),
            Some(Value::Array(blocks)) => blocks.iter_mut().try_for_each(|block| self.block(block)),
            _ => Ok(()),
        }
    }

    fn block(&mut self, block: &mut Value) -> Result {
        match kind(block) {
            "text" | "input_text" | "output_text" => self.text(block.get_mut("text")),
            "refusal" => self.text(block.get_mut("refusal")),
            "tool_use" | "server_tool_use" | "mcp_tool_use" => {
                self.arguments(block.get_mut("input"))
            }
            "tool_result" | "mcp_tool_result" => self.content(block.get_mut("content")),
            "function_call" | "mcp_call" => self.arguments(block.get_mut("arguments")),
            "custom_tool_call" => self.text(block.get_mut("input")),
            "function_call_output" | "custom_tool_call_output" => {
                self.content(block.get_mut("output"))
            }
            "message" => self.message(block),
            "" if block.get("role").is_some() => self.message(block),
            // Images, audio, documents, thinking/signature/encrypted_content are opaque.
            _ => Ok(()),
        }
    }

    fn message(&mut self, message: &mut Value) -> Result {
        self.content(message.get_mut("content"))?;
        self.text(message.get_mut("refusal"))?;
        self.text(message.get_mut("reasoning_content"))?;
        self.arguments(message.pointer_mut("/function_call/arguments"))?;
        each(message.get_mut("tool_calls"), |call| {
            self.arguments(call.pointer_mut("/function/arguments"))?;
            self.text(call.pointer_mut("/custom/input"))
        })
    }

    fn tool_definition(&mut self, tool: &mut Value) -> Result {
        self.text(tool.get_mut("description"))?;
        self.text(tool.pointer_mut("/function/description"))?;
        for path in ["/parameters", "/input_schema", "/function/parameters"] {
            if let Some(schema) = tool.pointer_mut(path) {
                self.schema(schema)?;
            }
        }
        Ok(())
    }

    fn schema(&mut self, schema: &mut Value) -> Result {
        self.text(schema.get_mut("description"))?;
        self.text(schema.get_mut("title"))?;
        for key in [
            "properties",
            "patternProperties",
            "$defs",
            "definitions",
            "dependentSchemas",
        ] {
            if let Some(Value::Object(fields)) = schema.get_mut(key) {
                for field in fields.values_mut() {
                    self.schema(field)?;
                }
            }
        }
        for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
            each(schema.get_mut(key), |v| self.schema(v))?;
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
            if let Some(Value::Object(field)) = schema.get_mut(key) {
                self.schema_object(field)?;
            }
        }
        Ok(())
    }

    fn schema_object(&mut self, value: &mut serde_json::Map<String, Value>) -> Result {
        let mut object = Value::Object(std::mem::take(value));
        self.schema(&mut object)?;
        if let Value::Object(inner) = object {
            *value = inner;
        }
        Ok(())
    }
}

pub(crate) fn request(format: Format, payload: &mut Value, mask: &mut Mask<'_>) -> Result {
    let mut transform = Transform::Mask(mask);
    match format {
        Format::Chat | Format::Messages => {
            each(payload.get_mut("messages"), |message| {
                transform.message(message)
            })?;
            if matches!(format, Format::Messages) {
                transform.content(payload.get_mut("system"))?;
            }
            each(payload.get_mut("functions"), |tool| {
                transform.tool_definition(tool)
            })?;
        }
        Format::Responses => {
            transform.content(payload.get_mut("input"))?;
            transform.text(payload.get_mut("instructions"))?;
        }
    }
    each(payload.get_mut("tools"), |tool| {
        transform.tool_definition(tool)
    })
}

pub(crate) fn response(format: Format, payload: &mut Value, restore: &mut Restore) -> Result {
    let mut transform = Transform::Restore(restore);
    match format {
        Format::Chat => each(payload.get_mut("choices"), |choice| {
            match choice.get_mut("message") {
                Some(message) => transform.message(message),
                None => Ok(()),
            }
        }),
        Format::Messages => {
            transform.content(payload.get_mut("content"))?;
            transform.content(payload.pointer_mut("/message/content"))?;
            if let Some(block) = payload.get_mut("content_block") {
                transform.block(block)?;
            }
            Ok(())
        }
        Format::Responses => {
            transform.content(payload.get_mut("output"))?;
            transform.content(payload.pointer_mut("/response/output"))?;
            for key in ["item", "part"] {
                if let Some(block) = payload.get_mut(key) {
                    transform.block(block)?;
                }
            }
            match kind(payload) {
                "response.output_text.done" => transform.text(payload.get_mut("text")),
                "response.refusal.done" => transform.text(payload.get_mut("refusal")),
                "response.function_call_arguments.done" => {
                    transform.arguments(payload.get_mut("arguments"))
                }
                "response.custom_tool_call_input.done" => transform.text(payload.get_mut("input")),
                _ => Ok(()),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Track(u64, u64, &'static str);

pub(super) struct Delta {
    pub(super) track: Track,
    pub(super) path: String,
    pub(super) json: bool,
}

fn index(value: &Value, field: &str, fallback: u64) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(fallback)
}

pub(super) fn deltas(format: Format, event: &Value) -> Vec<Delta> {
    let mut result = Vec::new();
    let mut add = |track: Track, path: String, json: bool| {
        if event.pointer(&path).is_some_and(Value::is_string) {
            result.push(Delta { track, path, json });
        }
    };
    match format {
        Format::Chat => {
            if let Some(Value::Array(choices)) = event.get("choices") {
                for (i, choice) in choices.iter().enumerate() {
                    let n = index(choice, "index", i as u64);
                    let path = format!("/choices/{i}/delta");
                    for field in ["content", "refusal", "reasoning_content"] {
                        add(Track(n, 0, field), format!("{path}/{field}"), false);
                    }
                    add(
                        Track(n, 0, "legacy_arguments"),
                        format!("{path}/function_call/arguments"),
                        true,
                    );
                    if let Some(Value::Array(calls)) = choice.pointer("/delta/tool_calls") {
                        for (j, call) in calls.iter().enumerate() {
                            let c = index(call, "index", j as u64);
                            add(
                                Track(n, c, "arguments"),
                                format!("{path}/tool_calls/{j}/function/arguments"),
                                true,
                            );
                            add(
                                Track(n, c, "custom"),
                                format!("{path}/tool_calls/{j}/custom/input"),
                                false,
                            );
                        }
                    }
                }
            }
        }
        Format::Messages if kind(event) == "content_block_delta" => {
            let n = index(event, "index", 0);
            match event.pointer("/delta/type").and_then(Value::as_str) {
                Some("text_delta") => add(Track(n, 0, "text"), "/delta/text".into(), false),
                Some("input_json_delta") => {
                    add(Track(n, 0, "arguments"), "/delta/partial_json".into(), true)
                }
                _ => {}
            }
        }
        Format::Responses => {
            let n = index(event, "output_index", 0);
            let c = index(event, "content_index", 0);
            match kind(event) {
                "response.output_text.delta" => add(Track(n, c, "text"), "/delta".into(), false),
                "response.refusal.delta" => add(Track(n, c, "refusal"), "/delta".into(), false),
                "response.function_call_arguments.delta" => {
                    add(Track(n, c, "arguments"), "/delta".into(), true)
                }
                "response.custom_tool_call_input.delta" => {
                    add(Track(n, c, "custom"), "/delta".into(), false)
                }
                _ => {}
            }
        }
        _ => {}
    }
    result
}
