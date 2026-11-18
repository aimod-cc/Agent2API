//! 仅核原流工具证据与原封存请求的声明和策略，不消费状态或签发交接/发送许可。
//! 全批原声明先校验；不改JSON、不补名称/schema/参数，不借插件投影或工具结果补声明。
use std::collections::BTreeSet;

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::super::json_text;
use super::{valid_name, ToolEvidence, MAX_CALLS};
use crate::server::core::plugin_host::wasm::conversation::CatPawMaterial;

const MAX_TOOL_BYTES: usize = 256 * 1024;
const MAX_TOOLS_BYTES: usize = 2 * 1024 * 1024;
const MAX_DESCRIPTION_CHARS: usize = 8000;
const MAX_SCHEMA_NODES: usize = 32_768;
const MAX_SCHEMA_DEPTH: usize = 32;
const MAX_PHYSICAL_DEPTH: usize = 64;

enum Choice<'a> {
    Auto,
    None,
    Required,
    Function(&'a str),
}

pub(super) fn verify(
    evidence: &ToolEvidence,
    material: &CatPawMaterial,
) -> Result<(), proto::WireError> {
    // 只借原材料这三个键；n/response_format或响应自报不能替代原工具条件。
    let names = declarations(material.parameter("tools"))?;
    let choice = choice(material.parameter("tool_choice"))?;
    let parallel = match material.parameter("parallel_tool_calls") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        _ => return Err(invalid()),
    };
    let calls = evidence.calls();
    if calls.is_empty() || calls.len() > MAX_CALLS {
        return Err(invalid());
    }
    if parallel == Some(false) && calls.len() > 1 {
        return Err(invalid());
    }
    match choice {
        Choice::None => return Err(invalid()),
        Choice::Required if names.is_empty() => return Err(invalid()),
        Choice::Function(name) => {
            if !names.contains(name) || calls.iter().any(|call| call.name() != name) {
                return Err(invalid());
            }
        }
        Choice::Auto | Choice::Required => {}
    }
    // 名称逐字匹配唯一原声明；同一工具的不同真实调用不按名称合并或重编号。
    for call in calls {
        if !valid_name(call.name()) || !names.contains(call.name()) {
            return Err(invalid());
        }
    }
    // ID/arguments/parameters仍借原ToolEvidence，不解析重写，也不宣称满足参数schema。
    Ok(())
}

fn declarations(value: Option<&Value>) -> Result<BTreeSet<&str>, proto::WireError> {
    let tools = match value {
        None | Some(Value::Null) => &[][..],
        Some(value @ Value::Array(_)) => {
            json_text::shape(value, MAX_TOOLS_BYTES).map_err(|_| invalid())?;
            value
                .as_array()
                .filter(|tools| tools.len() <= MAX_CALLS)
                .ok_or_else(invalid)?
                .as_slice()
        }
        _ => return Err(invalid()),
    };
    let mut names = BTreeSet::new();
    for tool in tools {
        // 未被choice选中的条目也严校，额外字段即使null也不能静默忽略。
        json_text::shape(tool, MAX_TOOL_BYTES).map_err(|_| invalid())?;
        let fields = json_text::fields(tool, &["type", "function"], &["type", "function"])
            .map_err(|_| invalid())?;
        if fields.get("type").and_then(Value::as_str) != Some("function") {
            return Err(invalid());
        }
        let function = json_text::fields(
            fields.get("function").ok_or_else(invalid)?,
            &["name", "description", "parameters", "strict"],
            &["name"],
        )
        .map_err(|_| invalid())?;
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| valid_name(name))
            .ok_or_else(invalid)?;
        if !names.insert(name) {
            return Err(invalid());
        }
        match function.get("strict") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => {}
            _ => return Err(invalid()),
        }
        match function.get("description") {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) if text.chars().count() <= MAX_DESCRIPTION_CHARS => {}
            _ => return Err(invalid()),
        }
        match function.get("parameters") {
            // 沿既有支持形式接受省略/null，但保留其原状态，绝不填默认schema。
            None | Some(Value::Null) => {}
            Some(schema @ Value::Object(_)) => schema_structure(schema)?,
            _ => return Err(invalid()),
        }
    }
    Ok(names)
}

fn choice(value: Option<&Value>) -> Result<Choice<'_>, proto::WireError> {
    match value {
        // 原协议的隐式auto；只解释原策略，不写回原JSON。
        None | Some(Value::Null) => Ok(Choice::Auto),
        Some(Value::String(value)) if value == "auto" => Ok(Choice::Auto),
        Some(Value::String(value)) if value == "none" => Ok(Choice::None),
        Some(Value::String(value)) if value == "required" => Ok(Choice::Required),
        Some(value @ Value::Object(_)) => {
            let fields = json_text::fields(value, &["type", "function"], &["type", "function"])
                .map_err(|_| invalid())?;
            if fields.get("type").and_then(Value::as_str) != Some("function") {
                return Err(invalid());
            }
            let function = json_text::fields(
                fields.get("function").ok_or_else(invalid)?,
                &["name"],
                &["name"],
            )
            .map_err(|_| invalid())?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| valid_name(name))
                .ok_or_else(invalid)?;
            Ok(Choice::Function(name))
        }
        _ => Err(invalid()),
    }
}

/// 沿已有工具投影的结构预算；不删schema键，也不做实际调用参数的schema验证。
fn schema_structure(value: &Value) -> Result<(), proto::WireError> {
    fn walk(
        value: &Value,
        depth: usize,
        physical: usize,
        nodes: &mut usize,
    ) -> Result<(), proto::WireError> {
        *nodes = nodes
            .checked_add(1)
            .filter(|nodes| *nodes <= MAX_SCHEMA_NODES)
            .ok_or_else(invalid)?;
        if depth > MAX_SCHEMA_DEPTH || physical > MAX_PHYSICAL_DEPTH {
            return Err(invalid());
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    walk(item, depth + 1, physical + 1, nodes)?;
                }
            }
            Value::Object(fields) => {
                for (key, item) in fields {
                    if matches!(key.as_str(), "anyOf" | "oneOf" | "allOf" | "not") {
                        if let Value::Array(branches) = item {
                            *nodes = nodes
                                .checked_add(1)
                                .filter(|nodes| *nodes <= MAX_SCHEMA_NODES)
                                .ok_or_else(invalid)?;
                            if physical + 1 > MAX_PHYSICAL_DEPTH {
                                return Err(invalid());
                            }
                            for branch in branches {
                                walk(branch, depth, physical + 2, nodes)?;
                            }
                        } else {
                            walk(item, depth, physical + 1, nodes)?;
                        }
                    } else {
                        walk(item, depth + 1, physical + 1, nodes)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    json_text::shape(value, MAX_TOOL_BYTES).map_err(|_| invalid())?;
    walk(value, 0, 0, &mut 0)
}

fn invalid() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 原返回工具与原请求声明、选择或并行约束不一致，未认可工具材料或签发续接许可",
        "catpaw_turn_tool_declaration_invalid",
    )
}
