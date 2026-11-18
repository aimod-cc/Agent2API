//! 固定回程转换的完整正文核对；不同协议只改变参数表示，不改变原工具批次。

use agent2api_provider_protocol::WireError;
use serde_json::Value;

use super::{invalid, json_text, OutputObservation};
use crate::server::core::protocol::{freeform, tool_plan};

pub(in crate::server::core::plugin_host::wasm::catpaw_turn) fn verify_responses(original: &OutputObservation, body: &Value, request: &Value, model: &str)
    -> Result<(), WireError> {
    if body["object"] != "response" || body["status"] != "completed" || !body["error"].is_null()
        || body["id"].as_str() != Some(original._identity.id.as_str()) || body["model"].as_str() != Some(model)
        || body["created_at"].as_u64() != Some(original._identity.created) {
        return Err(invalid());
    }
    let items = body["output"].as_array().ok_or_else(invalid)?;
    let plan = tool_plan::plan_tools(request);
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = 0;
    for item in items {
        match item["type"].as_str() {
            Some("message") if item["role"] == "assistant" && item["status"] == "completed" => {
                for block in item["content"].as_array().ok_or_else(invalid)? {
                    if block["type"] != "output_text" { return Err(invalid()); }
                    text.push_str(block["text"].as_str().ok_or_else(invalid)?);
                }
            }
            Some("reasoning") => {
                for block in item["summary"].as_array().ok_or_else(invalid)? {
                    if block["type"] != "summary_text" { return Err(invalid()); }
                    reasoning.push_str(block["text"].as_str().ok_or_else(invalid)?);
                }
            }
            Some("function_call" | "custom_tool_call") if item["status"] == "completed" => {
                let expected = original.message.tools.get(calls).ok_or_else(invalid)?;
                if item["call_id"].as_str() != Some(expected.id.as_str())
                    || plan.call_name(item).map_err(|_| invalid())? != expected.name { return Err(invalid()); }
                if plan.is_custom(&expected.name) {
                    if item["type"] != "custom_tool_call" || item["input"].as_str()
                        != Some(freeform::unwrap_freeform_input(&expected.arguments).as_str()) {
                        return Err(invalid());
                    }
                } else if item["type"] != "function_call" || item["arguments"].as_str() != Some(expected.arguments.as_str()) {
                    return Err(invalid());
                }
                calls += 1;
            }
            _ => return Err(invalid()),
        }
    }
    if text != original.message.content || reasoning != original.message.reasoning || calls != original.message.tools.len() {
        return Err(invalid());
    }
    verify_usage(original, &body["usage"], true)
}

pub(in crate::server::core::plugin_host::wasm::catpaw_turn) fn verify_anthropic(original: &OutputObservation, body: &Value, model: &str) -> Result<(), WireError> {
    if body["type"] != "message" || body["role"] != "assistant"
        || body["id"].as_str() != Some(original._identity.id.as_str()) || body["model"].as_str() != Some(model)
        || body["stop_reason"] != if original.finish == "tool_calls" { "tool_use" } else { "end_turn" } {
        return Err(invalid());
    }
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = 0;
    for block in body["content"].as_array().ok_or_else(invalid)? {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().ok_or_else(invalid)?),
            Some("thinking") => reasoning.push_str(block["thinking"].as_str().ok_or_else(invalid)?),
            Some("tool_use") => {
                let expected = original.message.tools.get(calls).ok_or_else(invalid)?;
                let parameters = json_text::parse(&expected.arguments, 256 * 1024)?;
                if block["id"].as_str() != Some(expected.id.as_str()) || block["name"].as_str() != Some(expected.name.as_str())
                    || block["input"] != parameters { return Err(invalid()); }
                calls += 1;
            }
            _ => return Err(invalid()),
        }
    }
    if text != original.message.content || reasoning != original.message.reasoning || calls != original.message.tools.len() {
        return Err(invalid());
    }
    verify_usage(original, &body["usage"], false)
}

fn verify_usage(original: &OutputObservation, usage: &Value, responses: bool) -> Result<(), WireError> {
    let Some(expected) = &original._usage else {
        return if usage.is_null() { Ok(()) } else { Err(invalid()) };
    };
    if usage["input_tokens"].as_u64() != Some(expected._prompt) || usage["output_tokens"].as_u64() != Some(expected._completion)
        || responses && usage["total_tokens"].as_u64() != Some(expected._total) {
        return Err(invalid());
    }
    Ok(())
}
