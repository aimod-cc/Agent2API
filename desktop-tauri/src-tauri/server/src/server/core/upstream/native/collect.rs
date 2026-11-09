use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::server::core::protocol::native::WireProtocol;
use crate::server::errors::GatewayError;

pub(super) struct Collector {
    protocol: WireProtocol,
    response: Option<Value>,
    blocks: BTreeMap<i64, Value>,
    arguments: BTreeMap<i64, String>,
    open_blocks: BTreeSet<i64>,
    ended: bool,
    size: usize,
}

impl Collector {
    pub(super) fn new(protocol: WireProtocol) -> Self {
        Self {
            protocol,
            response: None,
            blocks: BTreeMap::new(),
            arguments: BTreeMap::new(),
            open_blocks: BTreeSet::new(),
            ended: false,
            size: 0,
        }
    }

    pub(super) fn is_done(&self) -> bool {
        self.ended
    }

    pub(super) fn push(&mut self, event: &Value, bytes: usize) -> Result<(), GatewayError> {
        if self.ended {
            return Ok(());
        }
        self.size = self.size.saturating_add(bytes);
        if self.size > 64 * 1024 * 1024 {
            return Err(GatewayError::with_status(
                502,
                "上游原生响应超过 64 MiB，已停止聚合",
            ));
        }
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "error" {
            let message = event
                .pointer("/error/message")
                .or_else(|| event.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("上游原生响应返回错误");
            return Err(GatewayError::with_status(502, message));
        }
        match self.protocol {
            WireProtocol::Responses => {
                if matches!(
                    kind,
                    "response.completed" | "response.incomplete" | "response.failed"
                ) {
                    self.response = event
                        .get("response")
                        .filter(|value| value.is_object())
                        .cloned();
                    self.ended = self.response.is_some();
                } else if let Some(response) =
                    event.get("response").filter(|value| value.is_object())
                {
                    self.response = Some(response.clone());
                }
            }
            WireProtocol::Anthropic => self.anthropic(event, kind)?,
            WireProtocol::Chat => {
                return Err(GatewayError::with_status(
                    502,
                    "Chat 响应应使用 Chat 聚合器",
                ))
            }
        }
        Ok(())
    }

    fn anthropic(&mut self, event: &Value, kind: &str) -> Result<(), GatewayError> {
        let index = event
            .get("index")
            .and_then(Value::as_i64)
            .filter(|index| *index >= 0);
        if matches!(
            kind,
            "content_block_start" | "content_block_delta" | "content_block_stop"
        ) && index.is_none()
        {
            return Err(GatewayError::with_status(502, "上游内容块缺少有效索引"));
        }
        let index = index.unwrap_or(0);
        match kind {
            "message_start" => {
                if self.response.is_some() || !event.get("message").is_some_and(Value::is_object) {
                    return Err(GatewayError::with_status(502, "上游消息起始事件无效或重复"));
                }
                self.response = event.get("message").cloned();
            }
            "content_block_start" => {
                let block = event
                    .get("content_block")
                    .filter(|value| value.is_object())
                    .ok_or_else(|| GatewayError::with_status(502, "上游缺少有效内容块"))?;
                if self.response.is_none() || self.blocks.contains_key(&index) {
                    return Err(GatewayError::with_status(
                        502,
                        "上游内容块缺少消息起始或索引重复",
                    ));
                }
                self.blocks.insert(index, block.clone());
                self.open_blocks.insert(index);
            }
            "content_block_delta" => {
                if !self.open_blocks.contains(&index) {
                    return Err(GatewayError::with_status(
                        502,
                        "上游内容增量没有对应的未结束内容块",
                    ));
                }
                let delta = event
                    .get("delta")
                    .filter(|value| value.is_object())
                    .ok_or_else(|| GatewayError::with_status(502, "上游内容增量无效"))?;
                let kind = delta.get("type").and_then(Value::as_str).unwrap_or("");
                if kind == "input_json_delta" {
                    if let Some(text) = delta.get("partial_json").and_then(Value::as_str) {
                        self.arguments.entry(index).or_default().push_str(text);
                    }
                } else if let Some(block) = self.blocks.get_mut(&index) {
                    let field = match kind {
                        "text_delta" => "text",
                        "thinking_delta" => "thinking",
                        "signature_delta" => "signature",
                        "citations_delta" => {
                            let citations = block
                                .as_object_mut()
                                .map(|block| block.entry("citations").or_insert_with(|| json!([])));
                            if let (Some(Value::Array(items)), Some(citation)) =
                                (citations, delta.get("citation"))
                            {
                                items.push(citation.clone());
                            }
                            return Ok(());
                        }
                        _ => {
                            return Err(GatewayError::with_status(
                                502,
                                format!(
                                    "非流式聚合尚不支持原生事件 {kind}；已停止，未丢弃字段继续返回"
                                ),
                            ))
                        }
                    };
                    if let Some(text) = delta.get(field).and_then(Value::as_str) {
                        if let Some(object) = block.as_object_mut() {
                            let value = object
                                .entry(field)
                                .or_insert_with(|| Value::String(String::new()));
                            if let Value::String(current) = value {
                                current.push_str(text);
                            }
                        }
                    }
                }
            }
            "content_block_stop" => {
                if !self.open_blocks.remove(&index) {
                    return Err(GatewayError::with_status(
                        502,
                        "上游内容块结束事件没有对应起始事件",
                    ));
                }
                if let Some(arguments) = self.arguments.remove(&index) {
                    let input = serde_json::from_str::<Value>(&arguments)
                        .map_err(|_| GatewayError::with_status(502, "上游工具参数不是有效 JSON"))?;
                    if let Some(block) = self.blocks.get_mut(&index) {
                        block["input"] = input;
                    }
                }
            }
            "message_delta" => {
                if let Some(response) = self.response.as_mut().and_then(Value::as_object_mut) {
                    if let Some(delta) = event.get("delta").and_then(Value::as_object) {
                        response.extend(
                            delta
                                .iter()
                                .map(|(key, value)| (key.clone(), value.clone())),
                        );
                    }
                    if let Some(usage) = event.get("usage").and_then(Value::as_object) {
                        if let Some(current) = response
                            .entry("usage")
                            .or_insert_with(|| json!({}))
                            .as_object_mut()
                        {
                            current.extend(
                                usage
                                    .iter()
                                    .map(|(key, value)| (key.clone(), value.clone())),
                            );
                        }
                    }
                }
            }
            "message_stop" => self.ended = true,
            _ => {}
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<Value, GatewayError> {
        if !self.ended || !self.arguments.is_empty() || !self.open_blocks.is_empty() {
            return Err(GatewayError::with_status(
                502,
                "上游原生响应未完整结束，未返回不完整的消息",
            ));
        }
        let Some(mut response) = self.response.take() else {
            return Err(GatewayError::with_status(502, "上游没有返回原生消息"));
        };
        if self.protocol == WireProtocol::Anthropic {
            response["content"] = Value::Array(self.blocks.into_values().collect());
        }
        Ok(response)
    }
}
