use std::collections::HashSet;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{json, Map, Number, Value};

use crate::server::errors::GatewayError;

use super::{failed, request::Context, CLOSE, MAX_ARGUMENT_BYTES, MAX_CALLS, OPEN};

pub enum Output {
    Text(String),
    Calls { text: String, calls: Vec<Value> },
}

pub fn parse(text: &str, context: &Context) -> Result<Output, GatewayError> {
    if !context.active() {
        return Ok(Output::Text(text.to_string()));
    }
    let Some((lead_text, blocks)) = extract_blocks(text)? else {
        return no_call(text, context);
    };
    let mut normalized = Vec::new();
    for inner in blocks {
        let json_text = strip_inner_code_fence(inner);
        let payload = parse_json(json_text)
            .map_err(|_| failed("网页模型的工具调用不是完整有效的 JSON，未执行"))?;
        let object = payload
            .as_object()
            .ok_or_else(|| failed("网页工具调用必须是对象"))?;
        if object.len() != 2
            || object.get("nonce").and_then(Value::as_str) != Some(context.nonce.as_str())
        {
            return Err(failed("网页模型的工具调用校验码无效或结构不完整，未执行"));
        }
        let calls = object
            .get("calls")
            .and_then(Value::as_array)
            .filter(|calls| !calls.is_empty() && calls.len() <= MAX_CALLS)
            .ok_or_else(|| failed("网页工具调用列表为空或数量过多"))?;
        for call in calls {
            if normalized.len() >= MAX_CALLS {
                return Err(failed("网页工具调用列表为空或数量过多"));
            }
            let (name, arguments) = extract_call_fields(call)?;
            if context
                .quoted
                .iter()
                .any(|(quoted_name, quoted_arguments)| {
                    quoted_name == name && quoted_arguments == &arguments
                })
            {
                // 随机值只能关联本次请求，不能证明模型没有复制输入中的调用示例。
                return Err(failed("网页模型重复了对话中的工具示例，未作为新调用执行"));
            }
            let validator = context
                .tools
                .get(name)
                .ok_or_else(|| failed("网页模型调用了本次未允许的工具，未执行"))?;
            let encoded = arguments.to_string();
            if encoded.len() > MAX_ARGUMENT_BYTES || !validator.is_valid(&arguments) {
                return Err(failed(format!(
                    "网页模型给工具 {name} 的参数不符合声明或超过大小上限，未执行"
                )));
            }
            let index = normalized.len();
            normalized.push(json!({
                "index": index,
                "id": format!("call_{}_{index}", context.nonce),
                "type": "function",
                "function": { "name": name, "arguments": encoded }
            }));
        }
    }
    if !context.parallel && normalized.len() != 1 {
        return Err(failed("网页模型未遵守禁止并行调用的设置，未执行"));
    }
    Ok(Output::Calls {
        text: lead_text,
        calls: normalized,
    })
}

fn strip_inner_code_fence(inner: &str) -> &str {
    let trimmed = inner.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(newline) = after_open.find('\n') else {
        return trimmed;
    };
    let info = after_open[..newline].trim();
    if !info
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+'))
    {
        return trimmed;
    }
    let body = &after_open[newline + 1..];
    let Some(before_close) = body.trim_end().strip_suffix("```") else {
        return trimmed;
    };
    before_close.trim()
}

/// 解析单个工具调用项：支持标准 `{name, arguments}` 以及常见 OpenAI 包装形态
/// （允许附带 `id`、`type: "function"`、嵌套 `function: {name, arguments}`，以及字符串编码的 JSON 对象 `arguments`），
/// 仍严格拒绝任何未知字段，避免静默丢弃被模型写到 `arguments` 之外的真实参数。
fn extract_call_fields(call: &Value) -> Result<(&str, Value), GatewayError> {
    let fields = call
        .as_object()
        .ok_or_else(|| failed("网页工具调用包含无效字段"))?;

    let target = if let Some(func_val) = fields.get("function") {
        for (key, val) in fields {
            match key.as_str() {
                "function" => {}
                "id" if val.is_string() => {}
                "type" if val.as_str() == Some("function") => {}
                _ => return Err(failed("网页工具调用包含无效字段")),
            }
        }
        let func = func_val
            .as_object()
            .filter(|obj| obj.len() == 2)
            .ok_or_else(|| failed("网页工具调用包含无效字段"))?;
        func
    } else {
        for (key, val) in fields {
            match key.as_str() {
                "name" | "arguments" => {}
                "id" if val.is_string() => {}
                "type" if val.as_str() == Some("function") => {}
                _ => return Err(failed("网页工具调用包含无效字段")),
            }
        }
        fields
    };

    let name = target
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| failed("网页工具调用缺少名称"))?;
    let raw_args = target
        .get("arguments")
        .ok_or_else(|| failed("网页工具参数必须是对象"))?;
    let arguments = match raw_args {
        Value::Object(_) => raw_args.clone(),
        Value::String(text) if text.len() <= MAX_ARGUMENT_BYTES => {
            let parsed =
                parse_json(text).map_err(|_| failed("网页工具参数必须是对象"))?;
            if !parsed.is_object() {
                return Err(failed("网页工具参数必须是对象"));
            }
            parsed
        }
        _ => return Err(failed("网页工具参数必须是对象")),
    };
    Ok((name, arguments))
}

fn extract_blocks(text: &str) -> Result<Option<(String, Vec<&str>)>, GatewayError> {
    let mut cursor = 0usize;
    let mut first_before: Option<&str> = None;
    let mut blocks = Vec::new();
    while let Some(rel_open) = text[cursor..].find(OPEN) {
        let open_start = cursor + rel_open;
        if text[cursor..open_start].contains(CLOSE) {
            return Err(failed("网页模型的工具调用标记不完整，未返回可执行调用"));
        }
        if first_before.is_none() {
            first_before = Some(&text[..open_start]);
        }
        let inner_start = open_start + OPEN.len();
        let Some(rel_close) = text[inner_start..].find(CLOSE) else {
            return Err(failed("网页模型的工具调用未完整结束，未返回可执行调用"));
        };
        let inner_end = inner_start + rel_close;
        let inner = &text[inner_start..inner_end];
        if inner.contains(OPEN) {
            return Err(failed("网页模型的工具调用标记格式无效，未执行"));
        }
        blocks.push(inner);
        cursor = inner_end + CLOSE.len();
    }
    if text[cursor..].contains(CLOSE) {
        return Err(failed("网页模型的工具调用标记不完整，未返回可执行调用"));
    }
    if blocks.is_empty() {
        return Ok(None);
    }
    Ok(Some((
        strip_leading_prose(first_before.unwrap_or("")),
        blocks,
    )))
}

fn strip_leading_prose(before: &str) -> String {
    let trimmed = before.trim();
    let line_start = trimmed.rfind('\n').map_or(0, |index| index + 1);
    let last_line = trimmed[line_start..].trim();
    let is_fence = last_line.strip_prefix("```").is_some_and(|info| {
        info.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+'))
    });
    if is_fence {
        let prior_fences = trimmed[..line_start]
            .lines()
            .filter(|line| line.trim_start().starts_with("```"))
            .count();
        if prior_fences % 2 == 0 {
            return trimmed[..line_start].trim_end().to_string();
        }
    }
    trimmed.to_string()
}

fn no_call(text: &str, context: &Context) -> Result<Output, GatewayError> {
    if context.required {
        Err(failed(
            "网页模型未按要求调用指定范围内的工具，未伪造调用或自动重发",
        ))
    } else {
        Ok(Output::Text(text.to_string()))
    }
}

pub fn collect_quoted(text: &str, quoted: &mut Vec<(String, Value)>) {
    let mut remaining = text;
    while let Some(start) = remaining.find(OPEN) {
        let inner = &remaining[start + OPEN.len()..];
        let Some(end) = inner.find(CLOSE) else {
            break;
        };
        if let Ok(value) = parse_json(&inner[..end]) {
            collect_value(&value, quoted, 0);
        }
        remaining = &inner[end + CLOSE.len()..];
    }
    // JSON 示例及日志即使没有调用标签，也不能被模型加标签后提升成执行请求。
    let mut start = None;
    let mut depth = 0usize;
    let mut string = false;
    let mut escape = false;
    for (at, ch) in text.char_indices() {
        if string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                string = false;
            }
            continue;
        }
        match ch {
            '"' if depth > 0 => string = true,
            '{' | '[' => {
                if depth == 0 {
                    start = Some(at);
                }
                depth += 1;
            }
            '}' | ']' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = start.take() {
                        if let Ok(value) = parse_json(&text[start..at + 1]) {
                            collect_value(&value, quoted, 0);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_value(value: &Value, quoted: &mut Vec<(String, Value)>, depth: usize) {
    if depth > 128 || quoted.len() >= 1024 {
        return;
    }
    match value {
        Value::Object(fields) => {
            let function = fields.get("function").unwrap_or(value);
            if let (Some(name), Some(arguments)) = (
                function.get("name").and_then(Value::as_str),
                function.get("arguments"),
            ) {
                let arguments = if let Some(text) = arguments.as_str() {
                    parse_json(text).ok()
                } else {
                    Some(arguments.clone())
                };
                if let Some(arguments) = arguments.filter(Value::is_object) {
                    if !quoted
                        .iter()
                        .any(|pair| pair.0 == name && pair.1 == arguments)
                    {
                        quoted.push((name.to_string(), arguments));
                    }
                }
            }
            for value in fields.values() {
                collect_value(value, quoted, depth + 1);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_value(value, quoted, depth + 1);
            }
        }
        _ => {}
    }
}

pub fn parse_json(text: &str) -> Result<Value, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = UniqueValue::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value.0)
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(json!(value)))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(json!(value)))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(json!(value)))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        Number::from_f64(value)
            .map(|number| UniqueValue(Value::Number(number)))
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueValue(json!(value)))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(json!(value)))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueValue>()? {
            values.push(value.0);
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut source: A) -> Result<Self::Value, A::Error> {
        let mut keys = HashSet::new();
        let mut values = Map::new();
        while let Some(key) = source.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            let value = source.next_value::<UniqueValue>()?;
            values.insert(key, value.0);
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}
