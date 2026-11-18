//! 原 JSON 文本先验物理嵌套与解码后重复键，再生成校验视图；上游仍发送原文。
//! Value 入参只有容器形状检查，真正可重复键的 round/turn/history/上下文均是文本。
use std::collections::BTreeSet;
use std::io::{self, Write};

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::error;

pub(super) const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 65_536;

pub(super) fn parse(text: &str, maximum: usize) -> Result<Value, proto::WireError> {
    if text.is_empty() || text.len() > maximum {
        return Err(error::budget());
    }
    scan(text)?;
    let value: Value = serde_json::from_str(text).map_err(|_| error::invalid())?;
    shape(&value, maximum)?;
    Ok(value)
}

pub(super) fn shape(value: &Value, maximum: usize) -> Result<(), proto::WireError> {
    fn walk(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), proto::WireError> {
        *nodes = nodes.checked_add(1).ok_or_else(error::budget)?;
        if depth > MAX_DEPTH || *nodes > MAX_NODES {
            return Err(error::budget());
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    walk(item, depth + 1, nodes)?;
                }
            }
            Value::Object(fields) => {
                for (key, item) in fields {
                    if key.len() > 256
                        || matches!(key.as_str(), "__proto__" | "prototype" | "constructor")
                    {
                        return Err(error::invalid());
                    }
                    walk(item, depth + 1, nodes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut nodes = 0;
    walk(value, 0, &mut nodes)?;
    let mut counter = Counter { bytes: 0, maximum };
    serde_json::to_writer(&mut counter, value).map_err(|_| error::budget())?;
    Ok(())
}

pub(super) fn encode(value: &Value, maximum: usize) -> Result<String, proto::WireError> {
    shape(value, maximum)?;
    serde_json::to_string(value).map_err(|_| error::invalid())
}

fn scan(text: &str) -> Result<(), proto::WireError> {
    let bytes = text.as_bytes();
    let mut stack: Vec<Option<BTreeSet<String>>> = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        match bytes[offset] {
            b'{' | b'[' => {
                if stack.len() >= MAX_DEPTH {
                    return Err(error::budget());
                }
                stack.push((bytes[offset] == b'{').then(BTreeSet::new));
                offset += 1;
            }
            b'}' | b']' => {
                if stack.pop().is_none() {
                    return Err(error::invalid());
                }
                offset += 1;
            }
            b'"' => {
                let begin = offset;
                offset += 1;
                let mut closed = false;
                while offset < bytes.len() {
                    match bytes[offset] {
                        b'\\' => offset = offset.checked_add(2).ok_or_else(error::invalid)?,
                        b'"' => {
                            offset += 1;
                            closed = true;
                            break;
                        }
                        _ => offset += 1,
                    }
                }
                if !closed {
                    return Err(error::invalid());
                }
                let mut after = offset;
                while bytes.get(after).is_some_and(u8::is_ascii_whitespace) {
                    after += 1;
                }
                if bytes.get(after) == Some(&b':') {
                    let quoted = text.get(begin..offset).ok_or_else(error::invalid)?;
                    let key: String = serde_json::from_str(quoted).map_err(|_| error::invalid())?;
                    if key.len() > 256
                        || matches!(key.as_str(), "__proto__" | "prototype" | "constructor")
                    {
                        return Err(error::invalid());
                    }
                    let Some(Some(keys)) = stack.last_mut() else {
                        return Err(error::invalid());
                    };
                    if !keys.insert(key) {
                        return Err(error::invalid());
                    }
                    if keys.len() > MAX_NODES {
                        return Err(error::budget());
                    }
                }
            }
            _ => offset += 1,
        }
    }
    if stack.is_empty() {
        Ok(())
    } else {
        Err(error::invalid())
    }
}

struct Counter {
    bytes: usize,
    maximum: usize,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|total| *total <= self.maximum)
            .ok_or_else(|| io::Error::other("catpaw_material_limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn fields<'a>(
    value: &'a Value,
    allowed: &[&str],
    required: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, proto::WireError> {
    value
        .as_object()
        .filter(|fields| {
            fields.keys().all(|key| allowed.contains(&key.as_str()))
                && required.iter().all(|key| fields.contains_key(*key))
        })
        .ok_or_else(error::invalid)
}

pub(super) fn identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
