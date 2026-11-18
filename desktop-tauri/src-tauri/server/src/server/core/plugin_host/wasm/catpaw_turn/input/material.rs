//! 锁外核对插件转译是否来自原封存Chat body；不签发来源/账号/网络/续接许可。
//! verify_projection只核材料，宿主真实目录见证必须另核；verify保留未核目录硬门。
#[path = "history_projection.rs"]
mod history_projection;

use agent2api_provider_protocol as proto;
use serde_json::{json, Map, Value};

use super::{Correlation, PreparedInput, StartInput};
use super::super::json_text;
use crate::server::core::plugin_host::wasm::conversation::CatPawMaterial;

/// 保守总门：不注册为执行消费者；只有另接真实模型见证才可启用发送。
pub(in crate::server::core::plugin_host::wasm) fn verify(
    material: &CatPawMaterial,
    prepared: &PreparedInput,
    ids: &Correlation,
    start: &StartInput,
) -> Result<(), proto::WireError> {
    verify_projection(material, prepared, ids, start)?;
    Err(model_unconfirmed())
}

/// 仅证明已绑定原材料的确定投影，返回Ok不代表目录/账号/发送许可。
/// 调用方须在原SourceGate/原行锁之外调用，并在round之前另强制宿主目录见证。
pub(in crate::server::core::plugin_host::wasm) fn verify_projection(
    material: &CatPawMaterial,
    prepared: &PreparedInput,
    ids: &Correlation,
    start: &StartInput,
) -> Result<(), proto::WireError> {
    json_text::shape(&prepared.constraints, 3 * 1024 * 1024)?;
    json_text::fields(&prepared.constraints, &["model_type", "model_name", "effort", "context", "tool_configs", "tool_choice", "parallel_requested", "client_stream"], &["model_type", "model_name", "effort", "context", "tool_configs", "tool_choice", "parallel_requested", "client_stream"])?;
    if prepared.constraints["client_stream"].as_bool().is_none() || prepared.constraints["model_type"].as_i64().is_none_or(|model| model <= 0) { return Err(mismatch()); }
    prepared.constraints["model_name"].as_str().filter(|name| json_text::identifier(name, 1024)).ok_or_else(mismatch)?;
    match material.model() {
        // 不猜canonical名/别名映射。原文字相等路径可核；其他合法原模型路径只留
        // 待真实目录见证复核，verify_projection返回Ok也绝不证明这两者对应。
        Value::String(requested) if json_text::identifier(requested, 1024) => {}
        Value::Number(number) if number.as_i64().is_some_and(|number| number > 0) => {}
        _ => return Err(mismatch()),
    }
    let total = [&start.round, &start.turn, &start.history, &start.directives].iter()
        .try_fold(0usize, |total, text| total.checked_add(text.len())).ok_or_else(mismatch)?;
    if total > 24 * 1024 * 1024 { return Err(mismatch()); }
    let expected = history_projection::project(material.messages(), prepared, ids)?;
    let history = json_text::parse(&start.history, json_text::MAX_BYTES)?;
    let directives = json_text::parse(&start.directives, 2 * 1024 * 1024)?;
    if history != expected.history || directives != expected.directives { return Err(mismatch()); }
    super::history::check(&history, prepared, ids)?;
    verify_parameters(material.parameters(), &prepared.constraints)?;
    let turn = json_text::parse(&start.turn, json_text::MAX_BYTES)?;
    if prepared.continuation {
        if !start.round.is_empty() { return Err(mismatch()); }
    } else {
        let round = json_text::parse(&start.round, json_text::MAX_BYTES)?;
        super::check_round(&round, prepared, ids, &history, &directives)?;
    }
    super::check_turn(&turn, prepared, ids, &history, &directives)?;
    // model_type来源、目录默认context及能力支持必须另由真实宿主目录见证核对。
    Ok(())
}

fn verify_parameters(original: &Map<String, Value>, constraints: &Value) -> Result<(), proto::WireError> {
    for (name, value) in original {
        match name.as_str() {
            "tools" | "tool_choice" | "parallel_tool_calls" | "reasoning_effort" | "reasoningEffort" | "effort"
                | "context_window" | "contextWindow" | "context_length" | "contextLength" => {}
            // body.stream不是原client_stream权限；父PreparedInput已对原SourceRef bool核验。
            "stream" => { if !value.is_null() && !value.is_boolean() { return Err(mismatch()); } }
            "stream_options" => {
                if !value.is_null() {
                    let options = json_text::fields(value, &["include_usage"], &[])?;
                    if options.get("include_usage").is_some_and(|flag| !flag.is_boolean()) { return Err(mismatch()); }
                }
            }
            "n" => { if !value.is_null() && value.as_u64() != Some(1) { return Err(unexpressed()); } }
            "response_format" => {
                if !value.is_null() {
                    json_text::fields(value, &["type"], &["type"])?;
                    if value["type"] != "text" { return Err(unexpressed()); }
                }
            }
            "model_params" | "modelParams" => {
                if !value.is_null() { json_text::fields(value, &["context"], &[])?; }
            }
            "request_context" | "requestContext" => {
                if !value.is_null() {
                    json_text::fields(value, &["modelParams"], &["modelParams"])?;
                    json_text::fields(&value["modelParams"], &["declarativeParams"], &["declarativeParams"])?;
                    json_text::fields(&value["modelParams"]["declarativeParams"], &["context"], &[])?;
                }
            }
            // temperature/top_p/max_tokens/泛化reasoning/metadata等未在当前原协议转译表达。
            // 包括未知null键都不静默忽略；材料完整保留，拒绝不是声称上游没有能力。
            _ => return Err(unexpressed()),
        }
    }
    let (choice, configs) = tools(original)?;
    if constraints["tool_choice"] != choice || constraints["tool_configs"] != Value::Array(configs) { return Err(mismatch()); }
    let parallel = original.get("parallel_tool_calls").unwrap_or(&Value::Null);
    if !matches!(parallel, Value::Null | Value::Bool(_)) || &constraints["parallel_requested"] != parallel { return Err(mismatch()); }
    let mut effort: Option<String> = None;
    for name in ["reasoning_effort", "reasoningEffort", "effort"] {
        if let Some(value) = original.get(name).filter(|value| !value.is_null()) {
            let text = value.as_str().filter(|text| text.len() <= 64).ok_or_else(mismatch)?.trim().to_ascii_lowercase();
            if text.is_empty() || effort.as_ref().is_some_and(|old| old != &text) { return Err(mismatch()); }
            effort = Some(text);
        }
    }
    match effort {
        None if !constraints["effort"].is_null() => return Err(mismatch()),
        Some(expected) if constraints["effort"].as_str().is_none_or(|actual| !actual.eq_ignore_ascii_case(&expected)) => return Err(mismatch()),
        _ => {}
    }
    let mut context: Option<String> = None;
    for name in ["context_window", "contextWindow", "context_length", "contextLength"] {
        observe_context(original.get(name), &mut context)?;
    }
    for name in ["model_params", "modelParams"] { observe_context(original.get(name).and_then(|value| value.get("context")), &mut context)?; }
    for name in ["request_context", "requestContext"] {
        observe_context(original.get(name).and_then(|value| value.pointer("/modelParams/declarativeParams/context")), &mut context)?;
    }
    if let Some(expected) = context {
        if constraints["context"].as_str() != Some(expected.as_str()) { return Err(mismatch()); }
    } else if !constraints["context"].is_null() {
        // 原请求未指定时插件可用了目录默认值；只核格式，来源必须由最终目录硬门挡住。
        if constraints["context"].as_str().is_none_or(|text| text.parse::<i64>().ok().is_none_or(|value| value <= 0)) { return Err(mismatch()); }
    }
    Ok(())
}

fn tools(original: &Map<String, Value>) -> Result<(Value, Vec<Value>), proto::WireError> {
    let choice = match original.get("tool_choice") {
        None | Some(Value::Null) => json!("auto"),
        Some(Value::String(choice)) if matches!(choice.as_str(), "auto" | "none" | "required") => json!(choice),
        Some(choice @ Value::Object(_)) => {
            json_text::fields(choice, &["type", "function"], &["type", "function"])?;
            json_text::fields(&choice["function"], &["name"], &["name"])?;
            if choice["type"] != "function" || choice["function"]["name"].as_str().is_none_or(|name| !tool_name(name)) { return Err(mismatch()); }
            choice.clone()
        }
        _ => return Err(mismatch()),
    };
    let items = match original.get("tools") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(items)) if items.len() <= 256 => items.as_slice(),
        _ => return Err(mismatch()),
    };
    let mut names = std::collections::BTreeSet::new();
    let mut configs = Vec::new();
    let mut all_bytes = 0usize;
    for tool in items {
        json_text::fields(tool, &["type", "function"], &["type", "function"])?;
        if tool["type"] != "function" { return Err(mismatch()); }
        let function = json_text::fields(&tool["function"], &["name", "description", "parameters", "strict"], &["name"])?;
        let name = function.get("name").and_then(Value::as_str).filter(|name| tool_name(name)).ok_or_else(mismatch)?;
        if !names.insert(name.to_string()) { return Err(mismatch()); }
        if function.get("strict").is_some_and(|value| !value.is_null() && value.as_bool() != Some(false)) { return Err(unexpressed()); }
        // 有据的既有默认schema：缺失/null参数；不改变原材料，也不给出结构强制保证。
        let default = json!({"type": "object", "properties": {}});
        let schema = function.get("parameters").filter(|value| !value.is_null()).unwrap_or(&default);
        if !schema.is_object() { return Err(mismatch()); }
        validate_tool_json(schema, true)?;
        let mut config = json!({"name": name, "enable": true, "fromClient": true, "inputSchema": schema});
        if let Some(description) = function.get("description").filter(|value| !value.is_null()) {
            let text = description.as_str().filter(|text| text.chars().count() <= 8000).ok_or_else(mismatch)?;
            config["description"] = json!(text);
        }
        let bytes = json_text::encode(&config, 256 * 1024)?.len();
        all_bytes = all_bytes.checked_add(bytes).filter(|bytes| *bytes <= 2 * 1024 * 1024).ok_or_else(mismatch)?;
        // 全批原定义先核，即使choice=none也不能用未选中藏未知/非法工具。
        let selected = match choice.as_str() {
            Some("none") => false,
            Some("auto" | "required") => true,
            _ => choice["function"]["name"].as_str() == Some(name),
        };
        if selected { configs.push(config); }
    }
    if choice == "required" && configs.is_empty() || choice.is_object() && configs.is_empty() { return Err(mismatch()); }
    Ok((choice, configs))
}

fn observe_context(value: Option<&Value>, expected: &mut Option<String>) -> Result<(), proto::WireError> {
    let Some(value) = value.filter(|value| !value.is_null()) else { return Ok(()); };
    let text = match value {
        Value::String(text) if text.len() <= 64 => text.trim().to_ascii_lowercase(),
        Value::Number(number) => number.as_i64().filter(|number| *number > 0).ok_or_else(mismatch)?.to_string(),
        _ => return Err(mismatch()),
    };
    let text = match text.as_str() { "200k" => "204800", "500k" => "512000", "1m" | "1024k" => "1024000", text => text };
    let context = text.parse::<i64>().ok().filter(|context| *context > 0).ok_or_else(mismatch)?.to_string();
    if expected.as_ref().is_some_and(|old| old != &context) { return Err(mismatch()); }
    *expected = Some(context);
    Ok(())
}

/// 与既有插件tools::validate_json相同的物理/语义双预算，参数不豁免schema组合键。
fn validate_tool_json(value: &Value, schema: bool) -> Result<(), proto::WireError> {
    fn walk(value: &Value, schema: bool, depth: usize, physical: usize, nodes: &mut usize) -> Result<(), proto::WireError> {
        *nodes = (*nodes).checked_add(1).filter(|nodes| *nodes <= 32_768).ok_or_else(mismatch)?;
        if depth > 32 || physical > 64 { return Err(mismatch()); }
        match value {
            Value::Array(items) => { for item in items { walk(item, schema, depth + 1, physical + 1, nodes)?; } }
            Value::Object(object) => {
                for (key, item) in object {
                    if matches!(key.as_str(), "__proto__" | "constructor" | "prototype") { return Err(mismatch()); }
                    if schema && matches!(key.as_str(), "anyOf" | "oneOf" | "allOf" | "not") {
                        if let Value::Array(branches) = item {
                            *nodes = (*nodes).checked_add(1).filter(|nodes| *nodes <= 32_768).ok_or_else(mismatch)?;
                            if physical + 1 > 64 { return Err(mismatch()); }
                            for branch in branches { walk(branch, schema, depth, physical + 2, nodes)?; }
                        } else { walk(item, schema, depth, physical + 1, nodes)?; }
                    } else { walk(item, schema, depth + 1, physical + 1, nodes)?; }
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(value, schema, 0, 0, &mut 0)?;
    json_text::shape(value, 256 * 1024)
}
fn tool_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}
fn mismatch() -> proto::WireError {
    proto::WireError::with_code("CatPaw 插件转译的历史、指令或约束与原封存真实请求不一致，未签发执行许可", "catpaw_call_projection_mismatch")
}
fn unexpressed() -> proto::WireError {
    proto::WireError::with_code("CatPaw 原请求含当前宿主无法证明完整转译的内容或上下文约束，未忽略或替换原材料", "catpaw_call_projection_unexpressed")
}
fn model_unconfirmed() -> proto::WireError {
    proto::WireError::with_code("CatPaw 原宿主模型目录选择来源尚未接入，不能按插件名称或数字猜测model_type和默认上下文，未签发执行许可", "catpaw_call_model_selection_unconfirmed")
}
