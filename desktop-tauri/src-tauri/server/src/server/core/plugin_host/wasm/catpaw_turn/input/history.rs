//! 新轮次只接受已完整配对的历史；工具尾项不能伪装为新 user 轮次。
use std::collections::{BTreeMap, BTreeSet};

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{error, json_text, tool_name, Correlation, PreparedInput};

pub(super) fn check(
    value: &Value,
    prepared: &PreparedInput,
    ids: &Correlation,
) -> Result<(), proto::WireError> {
    let messages = value
        .as_array()
        .filter(|items| {
            !items.is_empty() && items.len() <= prepared.message_count
        })
        .ok_or_else(error::invalid)?;
    let mut message_ids = BTreeSet::new();
    let mut known = BTreeSet::new();
    let mut pending = BTreeMap::<String, String>::new();
    for message in messages {
        json_text::fields(
            message,
            &["type", "messageId", "content", "finished"],
            &["type", "messageId", "content", "finished"],
        )?;
        let id = message["messageId"]
            .as_str()
            .filter(|id| json_text::identifier(id, 256))
            .ok_or_else(error::invalid)?;
        if id == ids.conversation
            || id == ids.turn
            || id == ids.response
            || !message_ids.insert(id)
            || !message["finished"].is_boolean()
        {
            return Err(error::invalid());
        }
        let role = message["type"]
            .as_str()
            .filter(|role| matches!(*role, "user" | "assistant" | "tool"))
            .ok_or_else(error::invalid)?;
        if role != "tool" && !pending.is_empty() {
            return Err(error::invalid());
        }
        let content = message["content"]
            .as_array()
            .filter(|items| items.len() <= 1024)
            .ok_or_else(error::invalid)?;
        if role != "assistant" && content.is_empty() {
            return Err(error::invalid());
        }
        for block in content {
            match block["type"].as_str() {
                Some("text") if role != "tool" => {
                    json_text::fields(
                        block,
                        &["type", "text", "reasoningContent"],
                        &["type", "text"],
                    )?;
                    if !block["text"].is_string()
                        || block
                            .get("reasoningContent")
                            .is_some_and(|value| !value.is_string())
                    {
                        return Err(error::invalid());
                    }
                }
                Some("image_url") if role == "user" => {
                    super::super::super::catpaw_images::projected(block)?;
                }
                Some("tool_use") if role == "assistant" => {
                    json_text::fields(
                        block,
                        &["type", "toolCallId", "toolName", "toolParams"],
                        &["type", "toolCallId", "toolName", "toolParams"],
                    )?;
                    let id = block["toolCallId"]
                        .as_str()
                        .filter(|id| json_text::identifier(id, 256))
                        .ok_or_else(error::invalid)?;
                    let name = block["toolName"]
                        .as_str()
                        .filter(|name| tool_name(name))
                        .ok_or_else(error::invalid)?;
                    let arguments = block["toolParams"].as_str().ok_or_else(error::invalid)?;
                    if !json_text::parse(arguments, 256 * 1024)?.is_object()
                        || !known.insert(id.to_string())
                        || known.len() > 4096
                    {
                        return Err(error::invalid());
                    }
                    pending.insert(id.to_string(), name.to_string());
                }
                Some("tool_result") if role == "tool" => {
                    json_text::fields(
                        block,
                        &["type", "toolCallId", "toolName", "toolResult"],
                        &["type", "toolCallId", "toolName", "toolResult"],
                    )?;
                    let id = block["toolCallId"].as_str().ok_or_else(error::invalid)?;
                    let name = block["toolName"].as_str().ok_or_else(error::invalid)?;
                    let result = block["toolResult"]
                        .as_str()
                        .filter(|text| text.len() <= 1024 * 1024)
                        .ok_or_else(error::invalid)?;
                    let expected = pending.remove(id).ok_or_else(error::invalid)?;
                    if expected != name || result.contains('\0') {
                        return Err(error::invalid());
                    }
                }
                _ => return Err(error::invalid()),
            }
        }
        if role == "tool" && !pending.is_empty() {
            return Err(error::invalid());
        }
    }
    if !pending.is_empty() {
        return Err(error::invalid());
    }
    let last = messages.last().ok_or_else(error::invalid)?;
    let tail = last["type"].as_str().ok_or_else(error::invalid)?;
    if prepared.continuation != (tail == "tool")
        || !prepared.continuation && prepared.constraints["client_stream"] == true && tail != "user" {
        return Err(error::busy());
    }
    Ok(())
}
