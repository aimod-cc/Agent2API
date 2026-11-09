//! 原生协议请求与路由所需的 Chat 投影分开保存；投影不作为同协议出口的发送体。

use std::sync::Arc;

use serde_json::{json, Value};

use super::{anthropic, responses};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireProtocol {
    Chat,
    Responses,
    Anthropic,
}

impl WireProtocol {
    pub fn from_provider(value: &str) -> Option<Self> {
        match value {
            "chat_completions" => Some(Self::Chat),
            "responses" => Some(Self::Responses),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat_completions",
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
        }
    }
}

#[derive(Clone)]
pub struct NativeRequest {
    pub protocol: WireProtocol,
    pub body: Arc<Value>,
    pub translation_error: Option<String>,
}

impl NativeRequest {
    pub fn prepare(protocol: WireProtocol, body: Value) -> Result<(Self, Value), String> {
        let translated = match protocol {
            WireProtocol::Chat => Ok(body.clone()),
            WireProtocol::Anthropic => anthropic::chat_from_anthropic(&body),
            WireProtocol::Responses => responses::chat_from_responses(&body),
        };
        let (projection, translation_error) = match translated {
            Ok(chat) => (chat, None),
            Err(error) if protocol == WireProtocol::Responses => {
                // 服务端状态只能由原生出口承接，路由投影不代表这些字段可以跨协议转换。
                let mut stateless = body.clone();
                if let Some(object) = stateless.as_object_mut() {
                    for key in [
                        "previous_response_id",
                        "conversation",
                        "prompt",
                        "background",
                    ] {
                        object.remove(key);
                    }
                }
                (responses::chat_from_responses(&stateless)?, Some(error))
            }
            Err(error) => return Err(error),
        };
        Ok((
            Self {
                protocol,
                body: Arc::new(body),
                translation_error,
            },
            projection,
        ))
    }

    pub fn requires_native(&self) -> bool {
        self.translation_error.is_some()
    }

    pub fn routed(&self, projection: &Value) -> Value {
        let mut body = (*self.body).clone();
        if let Some(object) = body.as_object_mut() {
            object.retain(|key, _| !key.starts_with("_wb_"));
            if let Some(model) = projection.get("model") {
                object.insert("model".to_string(), model.clone());
            }
        }
        body
    }
}

pub fn rewrite_response_model(protocol: WireProtocol, body: &mut Value, model: &str) -> bool {
    if model.is_empty() {
        return false;
    }
    let mut changed = replace_model(body, model);
    match protocol {
        WireProtocol::Anthropic => {
            if let Some(message) = body.get_mut("message") {
                changed |= replace_model(message, model);
            }
        }
        WireProtocol::Responses => {
            if let Some(response) = body.get_mut("response") {
                changed |= replace_model(response, model);
            }
        }
        WireProtocol::Chat => {}
    }
    changed
}

fn replace_model(body: &mut Value, model: &str) -> bool {
    let Some(value) = body.get_mut("model") else {
        return false;
    };
    if value.as_str() == Some(model) {
        return false;
    }
    *value = Value::String(model.to_string());
    true
}

pub fn error_frame(protocol: WireProtocol, message: &str, sequence: i64) -> bytes::Bytes {
    match protocol {
        WireProtocol::Anthropic => super::event_frame(
            "error",
            &json!({
                "type": "error", "error": { "type": "api_error", "message": message },
            }),
        ),
        WireProtocol::Responses => super::event_frame(
            "error",
            &json!({
                "type": "error", "code": "upstream_error", "message": message, "param": Value::Null,
                "sequence_number": sequence,
            }),
        ),
        WireProtocol::Chat => super::chat_frame(&json!({
            "error": { "type": "upstream_error", "message": message },
        })),
    }
}
