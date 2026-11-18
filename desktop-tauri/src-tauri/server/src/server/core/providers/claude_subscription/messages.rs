//! Magpie renderClaudePrompt/renderClaudeTurn：CLI 接收用户内容块，旧历史明确标为
//! 历史而非重建签名消息。存活会话只追加新输入；工具结果走 MCP，不伪装成用户正文。
use super::{
    errors, media, process,
    tools::{ToolResult, ToolSet},
};
use crate::server::core::{protocol, proxies::ResolvedProxy};
use crate::server::errors::GatewayError;
use serde_json::{json, Value};

#[derive(Clone)]
pub(super) struct Request {
    pub model: String,
    pub messages: Vec<Value>,
    pub tools: ToolSet,
    pub effort: Option<String>,
    pub thinking_off: bool,
    pub system: String,
    pub identity: String,
    pub prefix_hashes: Vec<String>,
    pub latest_results: Vec<(String, Value, bool)>,
}

impl Request {
    pub fn parse(body: &Value) -> Result<Self, GatewayError> {
        for raw in body.get("messages").and_then(Value::as_array).into_iter().flatten() {
            if let Some(parts) = raw.get("content").and_then(Value::as_array) {
                if parts.iter().any(|part| part.get("type").and_then(Value::as_str) == Some("redacted_thinking")) {
                    // 公共 Chat 合同无法保真密封块；不能先转换掉再宣称支持。
                    return Err(GatewayError::bad_request(
                        "当前 Claude Chat 桥接不接收密封思考历史；未删除该内容继续回答",
                    ));
                }
            }
        }
        let native_messages = body.get("messages").and_then(Value::as_array).is_some_and(|messages| {
            messages.iter().any(|message| {
                message.get("content").and_then(Value::as_array).is_some_and(|parts| {
                    parts.iter().any(|part| {
                        matches!(part.get("type").and_then(Value::as_str), Some("tool_use" | "tool_result"))
                    })
                })
            })
        });
        let native_tools = body
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| tools.iter().any(|tool| tool.get("input_schema").is_some()));
        let body = if native_messages || native_tools || body.get("system").is_some() {
            let mut converted = protocol::anthropic::chat_from_anthropic(body)
                .map_err(|_| GatewayError::bad_request("Claude 原生消息无法转换"))?;
            for key in ["thinking", "output_config", "response_format"] {
                if let Some(value) = body.get(key) {
                    converted[key] = value.clone();
                }
            }
            converted
        } else if body.get("messages").is_none() && body.get("input").is_some() {
            protocol::responses::chat_from_responses(body)
                .map_err(|_| GatewayError::bad_request("Claude 请求的 Responses 消息无法转换"))?
        } else {
            body.clone()
        };
        if body.pointer("/response_format/type").and_then(Value::as_str).is_some_and(|kind| kind != "text")
            || body.get("output_config").and_then(|v| v.get("format")).is_some()
        {
            return Err(GatewayError::bad_request("此 Claude CLI 桥接尚未接入强制 JSON schema；未忽略该要求继续回答"));
        }
        if body.get("n").and_then(Value::as_u64).is_some_and(|n| n != 1) {
            return Err(GatewayError::bad_request("Claude CLI 一次只返回一个候选回复"));
        }
        let all = body
            .get("messages")
            .and_then(Value::as_array)
            .filter(|v| !v.is_empty() && v.len() <= 4096)
            .ok_or_else(|| GatewayError::bad_request("Claude 请求必须包含非空且不超过 4096 条的消息"))?;
        let mut messages = Vec::new();
        let mut system = String::new();
        if let Some(value) = body.get("system") {
            append_text(&mut system, &protocol::content_text(value));
        }
        for message in all {
            let role = message.get("role").and_then(Value::as_str).unwrap_or("");
            if matches!(role, "system" | "developer") {
                append_text(&mut system, &protocol::content_text(message.get("content").unwrap_or(&Value::Null)));
            } else if matches!(role, "user" | "assistant" | "tool") {
                messages.push(message.clone());
            } else {
                return Err(GatewayError::bad_request("Claude 消息角色无效"));
            }
        }
        if messages.is_empty() {
            return Err(GatewayError::bad_request("Claude 请求没有用户消息"));
        }
        let model =
            body.get("model").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("default").to_string();
        let effort = protocol::anthropic_thinking::effort(&body).map(|s| s.to_ascii_lowercase());
        let thinking_off = body.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled")
            || effort.as_deref().is_some_and(|s| matches!(s, "off" | "none"));
        let effort = effort.filter(|s| !matches!(s.as_str(), "off" | "none"));
        if effort.as_deref().is_some_and(|s| !matches!(s, "low" | "medium" | "high" | "xhigh" | "max")) {
            return Err(GatewayError::bad_request("Claude Code 不认识该思考等级，未静默改用默认值"));
        }
        let tools = ToolSet::parse(&body)?;
        let identity = process::digest(
            serde_json::to_string(&json!([model, system, tools.fingerprint, effort, thinking_off]))
                .unwrap_or_default()
                .as_bytes(),
        );
        let prefix_hashes = hashes(&messages);
        let last_reply = messages
            .iter()
            .rposition(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut latest_results = Vec::new();
        for message in &messages[last_reply..] {
            if message.get("role").and_then(Value::as_str) == Some("tool") {
                let id = message
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| GatewayError::bad_request("Claude 工具结果缺少 tool_call_id"))?;
                latest_results.push((
                    id.into(),
                    message.get("content").cloned().unwrap_or(Value::Null),
                    message
                        .get("is_error")
                        .or_else(|| message.get(protocol::FIELD_IS_ERROR))
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                ));
            }
            for part in message.get("content").and_then(Value::as_array).into_iter().flatten() {
                if part.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let id = part
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| GatewayError::bad_request("Claude 工具结果缺少 tool_use_id"))?;
                    latest_results.push((
                        id.into(),
                        part.get("content").cloned().unwrap_or(Value::Null),
                        part.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    ));
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        if latest_results.iter().any(|(id, _, _)| !seen.insert(id)) {
            return Err(GatewayError::bad_request("同一 Claude 工具结果重复提交"));
        }
        Ok(Self { model, messages, tools, effort, thinking_off, system, identity, prefix_hashes, latest_results })
    }

    pub async fn prompt(&self, start: usize, proxy: Option<&ResolvedProxy>) -> Result<Vec<Value>, GatewayError> {
        let mut text = String::new();
        let mut blocks = Vec::new();
        if start == 0 {
            text.push_str("<external_system_instructions>\n");
            text.push_str(&self.system);
            text.push_str("\nThe tools run on the caller's computer, not this gateway. Use only the supplied MCP tools.\n</external_system_instructions>\n\n");
        }
        if self.tools.required {
            text.push_str("You must call one of the supplied tools for this reply.\n\n");
        }
        let since = self.messages.get(start..).ok_or_else(|| errors::session("Claude 会话历史边界无效"))?;
        let history_end = since
            .iter()
            .rposition(|message| {
                message.get("role").and_then(Value::as_str) == Some("assistant") && !has_calls(message)
            })
            .map(|i| i + 1)
            .unwrap_or(0);
        if history_end > 0 {
            text.push_str("<conversation_history>\nThe following turns already happened; do not answer them again.\n");
        }
        let mut images = 0;
        for (index, message) in since.iter().enumerate() {
            if history_end > 0 && index == history_end {
                text.push_str("</conversation_history>\n<current_turn>\n");
            }
            let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
            if role == "tool" {
                text.push_str(&format!(
                    "[tool result id={}]\n",
                    message.get("tool_call_id").and_then(Value::as_str).unwrap_or("")
                ));
            } else {
                text.push_str(if role == "assistant" { "Assistant: " } else { "Human: " });
            }
            append_parts(message.get("content").unwrap_or(&Value::Null), &mut text, &mut blocks, &mut images, proxy)
                .await?;
            if role == "tool" {
                text.push_str("\n[end of tool result]\n");
            }
            if let Some(thinking) = message.get("reasoning_content").and_then(Value::as_str) {
                append_text(&mut text, thinking);
            }
            for call in message.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
                let id = call.get("id").and_then(Value::as_str).unwrap_or("");
                let args = call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}");
                text.push_str(&format!("\n[tool call {} id={id} args={args}]", self.tools.cli_name(name)));
            }
            text.push_str("\n\n");
        }
        if history_end > 0 {
            text.push_str("</current_turn>\n");
        }
        if since.iter().any(has_calls) {
            text.push_str("The tool calls above are past actions. Make new tool calls only through the supplied tools, not as text.\n");
        }
        flush(&mut text, &mut blocks);
        if blocks.is_empty() {
            return Err(GatewayError::bad_request("Claude 新一轮没有可提交内容"));
        }
        Ok(blocks)
    }

    pub async fn results(&self, proxy: Option<&ResolvedProxy>) -> Result<Vec<(String, ToolResult)>, GatewayError> {
        let mut result_images = Vec::new();
        let last = self
            .messages
            .iter()
            .rposition(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
            .map(|i| i + 1)
            .unwrap_or(0);
        for message in &self.messages[last..] {
            if message.get("role").and_then(Value::as_str) != Some("user") {
                continue;
            }
            if let Some(parts) = message.get("content").and_then(Value::as_array) {
                result_images.extend(
                    parts
                        .iter()
                        .filter(|part| {
                            matches!(
                                part.get("type").and_then(Value::as_str),
                                Some("image" | "image_url" | "input_image")
                            )
                        })
                        .cloned(),
                );
            }
        }
        let mut result = Vec::new();
        let mut total_size = 0usize;
        let mut images = 0;
        for (position, (id, content, is_error)) in self.latest_results.iter().enumerate() {
            let mut blocks = Vec::new();
            let mut text = String::new();
            append_parts(content, &mut text, &mut blocks, &mut images, proxy).await?;
            if position + 1 == self.latest_results.len() && !result_images.is_empty() {
                append_parts(&json!(result_images), &mut text, &mut blocks, &mut images, proxy).await?;
            }
            flush(&mut text, &mut blocks);
            let blocks: Vec<_> = blocks.into_iter().map(|block| {
                if block.get("type").and_then(Value::as_str) == Some("image") {
                    json!({"type":"image","mimeType":block.pointer("/source/media_type"),"data":block.pointer("/source/data")})
                } else { block }
            }).collect();
            total_size = total_size.saturating_add(blocks.iter().map(|block| block.to_string().len()).sum::<usize>());
            if total_size > process::MAX_LINE {
                return Err(GatewayError::bad_request("Claude 本轮工具结果超过 32MiB 限制"));
            }
            result.push((
                id.clone(),
                ToolResult {
                    content: if blocks.is_empty() { vec![json!({"type":"text","text":""})] } else { blocks },
                    is_error: *is_error,
                    tools: None,
                },
            ));
        }
        Ok(result)
    }

    /// 用户在工具执行期间追加的话仍为用户消息，不混进工具输出。
    pub async fn alongside_results(&self, proxy: Option<&ResolvedProxy>) -> Result<Vec<Value>, GatewayError> {
        let last = self
            .messages
            .iter()
            .rposition(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut blocks = Vec::new();
        let mut text = String::new();
        let mut images = 0;
        for message in &self.messages[last..] {
            if message.get("role").and_then(Value::as_str) != Some("user") {
                continue;
            }
            let content = message.get("content").unwrap_or(&Value::Null);
            let content = if let Some(parts) = content.as_array() {
                Value::Array(
                    parts
                        .iter()
                        .filter(|p| {
                            !matches!(
                                p.get("type").and_then(Value::as_str),
                                Some("tool_result" | "image" | "image_url" | "input_image")
                            )
                        })
                        .cloned()
                        .collect(),
                )
            } else {
                content.clone()
            };
            append_parts(&content, &mut text, &mut blocks, &mut images, proxy).await?;
        }
        flush(&mut text, &mut blocks);
        Ok(blocks)
    }
}

async fn append_parts(
    content: &Value,
    text: &mut String,
    blocks: &mut Vec<Value>,
    images: &mut usize,
    proxy: Option<&ResolvedProxy>,
) -> Result<(), GatewayError> {
    if let Some(value) = content.as_str() {
        text.push_str(value);
        return Ok(());
    }
    if content.is_null() {
        return Ok(());
    }
    let parts = content.as_array().ok_or_else(|| GatewayError::bad_request("Claude 消息内容必须是文本或内容块数组"))?;
    for part in parts {
        if let Some(value) = part.as_str() {
            text.push_str(value);
            continue;
        }
        match part.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" | "input_text" | "output_text" => {
                text.push_str(part.get("text").and_then(Value::as_str).unwrap_or(""))
            }
            "thinking" => text.push_str(part.get("thinking").and_then(Value::as_str).unwrap_or("")),
            "image" | "image_url" | "input_image" => {
                *images += 1;
                if *images > 20 {
                    return Err(GatewayError::bad_request("Claude 单次最多处理 20 张图片"));
                }
                flush(text, blocks);
                blocks.push(media::read(part, proxy).await?.anthropic());
            }
            "tool_use" => text.push_str(&format!(
                "[tool call {} id={} args={}]",
                part.get("name").and_then(Value::as_str).unwrap_or(""),
                part.get("id").and_then(Value::as_str).unwrap_or(""),
                part.get("input").unwrap_or(&Value::Null)
            )),
            "tool_result" => {
                text.push_str(&format!(
                    "[tool result id={}]\n",
                    part.get("tool_use_id").and_then(Value::as_str).unwrap_or("")
                ));
                // Anthropic 原生工具结果只在消息归一时处理一层；递归对象不执行。
                let content = part.get("content").unwrap_or(&Value::Null);
                if let Some(s) = content.as_str() {
                    text.push_str(s);
                } else if let Some(parts) = content.as_array() {
                    for piece in parts {
                        match piece.get("type").and_then(Value::as_str) {
                            Some("text") => text.push_str(piece.get("text").and_then(Value::as_str).unwrap_or("")),
                            Some("image") => {
                                *images += 1;
                                if *images > 20 {
                                    return Err(GatewayError::bad_request("Claude 图片数量超过限制"));
                                }
                                flush(text, blocks);
                                blocks.push(media::read(piece, proxy).await?.anthropic());
                            }
                            _ => return Err(GatewayError::bad_request("Claude 工具结果包含未支持的内容，未丢弃")),
                        }
                    }
                } else if !content.is_null() {
                    text.push_str(&content.to_string());
                }
                text.push_str("\n[end of tool result]\n");
            }
            "redacted_thinking" => {
                return Err(GatewayError::bad_request("新的 Claude CLI 会话不能导入其他会话的密封思考；请从原会话继续"))
            }
            _ => {
                return Err(GatewayError::bad_request("Claude 消息含未接入的文件、音频或内容类型，未丢弃内容继续回答"))
            }
        }
    }
    Ok(())
}

fn has_calls(message: &Value) -> bool {
    message.get("tool_calls").and_then(Value::as_array).is_some_and(|v| !v.is_empty())
        || message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|v| v.iter().any(|p| p.get("type").and_then(Value::as_str) == Some("tool_use")))
}
fn append_text(text: &mut String, value: &str) {
    if !value.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(value);
    }
}
fn flush(text: &mut String, blocks: &mut Vec<Value>) {
    if !text.is_empty() {
        blocks.push(json!({"type":"text","text":std::mem::take(text)}));
    }
}

pub(super) fn hashes(messages: &[Value]) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut hashes = Vec::with_capacity(messages.len());
    for message in messages {
        // 只含对话字段；忽略客户端附加的统计/时间戳，不靠字符串拼接猜边界。
        let content = match message.get("content").filter(|c| !c.is_null()) {
            Some(Value::Array(parts))
                if parts.iter().all(|part| part.get("type").and_then(Value::as_str) == Some("text")) =>
            {
                Value::String(
                    parts
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(""),
                )
            }
            Some(value) => value.clone(),
            None => json!(""),
        };
        let normalized = json!({"role":message.get("role"),"content":content,
            "tool_calls": message.get("tool_calls").filter(|v| v.as_array().is_some_and(|v| !v.is_empty())),
            "tool_call_id":message.get("tool_call_id")});
        let bytes = serde_json::to_vec(&normalized).unwrap_or_default();
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
        hashes.push(hash.clone().finalize().iter().map(|b| format!("{b:02x}")).collect());
    }
    hashes
}
