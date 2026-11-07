use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};

use crate::server::core::protocol::{
    anthropic_outbound::ChatFromAnthropicStream, responses_outbound::ChatFromResponsesStream,
    SseLineBuffer,
};

enum Machine {
    Anthropic(ChatFromAnthropicStream),
    Responses(ChatFromResponsesStream),
    Chat,
}

struct Conversion {
    machine: Machine,
    lines: SseLineBuffer,
    terminal: bool,
}

fn conversions() -> &'static Mutex<HashMap<String, Conversion>> {
    static ITEMS: OnceLock<Mutex<HashMap<String, Conversion>>> = OnceLock::new();
    ITEMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn error(message: &str) -> proto::WireError {
    proto::WireError::with_code(message, proto::WireError::INTERNAL)
}

pub fn open(params: &Value) -> Result<Value, proto::WireError> {
    let model = params["model"].as_str().unwrap_or("");
    let machine = match params["protocol"].as_str() {
        Some("anthropic") => Machine::Anthropic(ChatFromAnthropicStream::new(model)),
        Some("responses") => Machine::Responses(ChatFromResponsesStream::new(model)),
        Some("chat") => Machine::Chat,
        _ => return Err(error("脚本插件使用了尚未支持的响应协议")),
    };
    let mut items = conversions()
        .lock()
        .map_err(|_| error("脚本响应转换不可用"))?;
    if items.len() >= 128 {
        return Err(error("脚本响应转换数量已达上限"));
    }
    let id = crate::server::core::protocol::random_id("convert");
    items.insert(
        id.clone(),
        Conversion {
            machine,
            lines: SseLineBuffer::new(),
            terminal: false,
        },
    );
    Ok(json!({ "id": id }))
}

pub fn push(params: &Value) -> Result<Value, proto::WireError> {
    let id = params["id"]
        .as_str()
        .ok_or_else(|| error("缺少脚本响应转换会话"))?;
    let mut items = conversions()
        .lock()
        .map_err(|_| error("脚本响应转换不可用"))?;
    let item = items
        .get_mut(id)
        .ok_or_else(|| error("脚本响应转换会话已结束"))?;
    let encoded = params["data"].as_str().unwrap_or("");
    if encoded.len() > 96 * 1024 {
        return Err(error("脚本响应分片过大"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| error("脚本响应编码无效"))?;
    let finish = params["finish"].as_bool().unwrap_or(false);
    let mut events = item.lines.push(&bytes);
    if finish {
        events.extend(item.lines.finish());
    }
    for event in events {
        if event.is_none() && !matches!(item.machine, Machine::Chat) && !item.terminal {
            items.remove(id);
            return Err(error("上游在协议结束事件前返回 DONE，未将截断内容视为成功"));
        }
        item.terminal |=
            match event {
                None => matches!(item.machine, Machine::Chat),
                Some(text) => serde_json::from_str::<Value>(&text)
                    .ok()
                    .is_some_and(|value| match &item.machine {
                        Machine::Anthropic(_) => {
                            matches!(value["type"].as_str(), Some("message_stop" | "error"))
                        }
                        Machine::Responses(_) => matches!(
                            value["type"].as_str(),
                            Some(
                                "response.completed"
                                    | "response.failed"
                                    | "response.incomplete"
                                    | "error"
                            )
                        ),
                        Machine::Chat => value.get("error").is_some_and(|error| !error.is_null()),
                    }),
            };
    }
    if finish && !item.terminal {
        items.remove(id);
        return Err(error("上游流未返回完整结束事件，未将截断内容视为成功"));
    }
    let frames = match &mut item.machine {
        Machine::Anthropic(machine) => {
            if finish {
                machine.finish()
            } else {
                machine.push(&bytes)
            }
        }
        Machine::Responses(machine) => {
            if finish {
                machine.finish()
            } else {
                machine.push(&bytes)
            }
        }
        Machine::Chat => vec![bytes::Bytes::from(bytes)],
    };
    let data: Vec<u8> = frames
        .into_iter()
        .flat_map(|frame| frame.to_vec())
        .collect();
    if finish {
        items.remove(id);
    }
    Ok(json!({ "data": base64::engine::general_purpose::STANDARD.encode(data) }))
}

pub fn close(params: &Value) {
    if let Some(id) = params["id"].as_str() {
        if let Ok(mut items) = conversions().lock() {
            items.remove(id);
        }
    }
}

pub fn clear() {
    if let Ok(mut items) = conversions().lock() {
        items.clear();
    }
}
