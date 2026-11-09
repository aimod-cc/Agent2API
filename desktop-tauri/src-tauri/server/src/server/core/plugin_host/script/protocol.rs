use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};

use crate::server::core::protocol::{
    anthropic_outbound::ChatFromAnthropicStream, responses_outbound::ChatFromResponsesStream,
    antigravity_stream::ChatFromAntigravityStream,
    SseLineBuffer,
};

enum Machine {
    Anthropic(ChatFromAnthropicStream),
    Responses(ChatFromResponsesStream),
    Gemini(ChatFromAntigravityStream),
    Chat,
}

struct Conversion {
    machine: Machine,
    lines: SseLineBuffer,
    terminal: bool,
    chat_choices: HashMap<u64, bool>,
}

impl Conversion {
    fn observe_chat(&mut self, value: &Value) {
        let Some(choices) = value["choices"].as_array() else { return; };
        for (position, choice) in choices.iter().enumerate() {
            let index = match choice.get("index") {
                None | Some(Value::Null) => position as u64,
                Some(index) => match index.as_u64() {
                    Some(index) => index,
                    None => continue,
                },
            };
            let finished = choice["finish_reason"]
                .as_str()
                .is_some_and(|reason| !reason.trim().is_empty());
            self.chat_choices.insert(index, finished);
        }
    }

    fn chat_finished(&self) -> bool {
        !self.chat_choices.is_empty() && self.chat_choices.values().all(|finished| *finished)
    }
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
        Some("gemini") => Machine::Gemini(ChatFromAntigravityStream::new(model, params["session"].as_str().unwrap_or(""))),
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
            chat_choices: HashMap::new(),
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
        let terminal = match event {
            None => matches!(item.machine, Machine::Chat),
            Some(text) => serde_json::from_str::<Value>(&text).ok().is_some_and(|value| {
                if matches!(item.machine, Machine::Chat) {
                    item.observe_chat(&value);
                }
                match &item.machine {
                    Machine::Anthropic(_) => {
                        matches!(value["type"].as_str(), Some("message_stop" | "error"))
                    }
                    Machine::Responses(_) => matches!(
                        value["type"].as_str(),
                        Some("response.completed" | "response.failed" | "response.incomplete" | "error")
                    ),
                    Machine::Gemini(_) => value.get("error").is_some_and(|error| !error.is_null())
                        || value.get("candidates").and_then(Value::as_array).is_some_and(|items| items.iter()
                            .any(|item| item["finishReason"].as_str().is_some_and(|reason| !reason.is_empty()))),
                    Machine::Chat => value.get("error").is_some_and(|error| !error.is_null()),
                }
            }),
        };
        item.terminal |= terminal;
    }
    // finish_reason 只结束对应 choice；等自然 EOF 才收尾，保留后续 choice 与用量帧。
    let close_chat = finish && !item.terminal && matches!(item.machine, Machine::Chat) && item.chat_finished();
    if finish && !item.terminal && !close_chat {
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
        Machine::Gemini(machine) => if finish { machine.finish() } else { machine.push(&bytes) },
        Machine::Chat => {
            let mut frames = vec![bytes::Bytes::from(bytes)];
            if close_chat {
                frames.push(bytes::Bytes::from_static(b"\n\ndata: [DONE]\n\n"));
            }
            frames
        }
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
