//! Magpie bridgeTools / bridgeToolName：稳定映射工具名，工具只由客户端执行。
use super::{errors, process};
use crate::server::errors::GatewayError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use tokio::sync::oneshot;

pub(super) const SERVER: &str = "agent2api";
pub(super) const PREFIX: &str = "mcp__agent2api__";
pub(super) const MAX_TOOLS: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Tool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone)]
pub(super) struct ToolSet {
    pub offered: Vec<Tool>,
    pub names: BTreeMap<String, String>,
    pub allowed: HashSet<String>,
    pub required: bool,
    pub fingerprint: String,
}

impl ToolSet {
    pub fn parse(body: &Value) -> Result<Self, GatewayError> {
        let offered = body.get("tools").and_then(Value::as_array);
        if offered.is_some_and(|tools| tools.len() > MAX_TOOLS) {
            return Err(GatewayError::bad_request("Claude 单次最多声明 256 个客户端工具"));
        }
        let mut offered = Vec::new();
        let mut names = BTreeMap::new();
        let mut originals = HashSet::new();
        let choice = body.get("tool_choice").unwrap_or(&Value::Null);
        let named = choice.pointer("/function/name").and_then(Value::as_str).or_else(|| {
            (choice.get("type").and_then(Value::as_str) == Some("tool"))
                .then(|| choice.get("name").and_then(Value::as_str))
                .flatten()
        });
        if !choice.is_null()
            && !matches!(choice.as_str(), Some("auto" | "none" | "required"))
            && named.is_none()
            && !matches!(choice.get("type").and_then(Value::as_str), Some("auto" | "any" | "none"))
        {
            return Err(GatewayError::bad_request("Claude tool_choice 格式不受支持"));
        }
        let disabled = choice.as_str() == Some("none") || choice.get("type").and_then(Value::as_str) == Some("none");
        let required = choice.as_str() == Some("required")
            || choice.get("type").and_then(Value::as_str) == Some("any")
            || named.is_some();
        if body.get("tools").is_some_and(|tools| !tools.is_array() && !tools.is_null()) {
            return Err(GatewayError::bad_request("Claude tools 必须是数组"));
        }
        for raw in body.get("tools").and_then(Value::as_array).into_iter().flatten() {
            if offered.len() >= MAX_TOOLS {
                return Err(GatewayError::bad_request("Claude 单次最多声明 256 个客户端工具"));
            }
            let definition = if raw.get("type").and_then(Value::as_str) == Some("function") {
                raw.get("function").unwrap_or(raw)
            } else if raw.get("type").is_none() && raw.get("input_schema").is_some() {
                raw
            } else {
                return Err(GatewayError::bad_request("Claude 桥接只接受客户端函数工具，不执行服务器内置工具"));
            };
            let name = definition
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && s.len() <= 512)
                .ok_or_else(|| GatewayError::bad_request("Claude 工具缺少有效名称"))?;
            if !originals.insert(name.to_string()) {
                return Err(GatewayError::bad_request("Claude 工具名称重复"));
            }
            if named.is_some_and(|only| only != name) {
                continue;
            }
            let mapped = bridge_name(name);
            if names.insert(mapped.clone(), name.into()).is_some() {
                return Err(GatewayError::bad_request("Claude 工具映射发生冲突"));
            }
            let schema = definition
                .get("parameters")
                .or_else(|| definition.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            if !schema.is_object() || schema.get("type").is_some_and(|v| v.as_str() != Some("object")) {
                return Err(GatewayError::bad_request("Claude MCP 工具参数必须是对象 schema"));
            }
            offered.push(Tool {
                name: mapped,
                description: definition.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
                input_schema: schema,
            });
        }
        if required && offered.is_empty() {
            return Err(GatewayError::bad_request("要求调用工具，但没有相应的客户端工具声明"));
        }
        let allowed = if disabled { HashSet::new() } else { names.values().cloned().collect() };
        let fingerprint = process::digest(
            serde_json::to_string(&offered).map_err(|_| errors::protocol("Claude 工具定义无法序列化"))?.as_bytes(),
        );
        Ok(Self { offered, names, allowed, required, fingerprint })
    }

    pub fn original(&self, name: &str) -> Option<&str> {
        self.names.get(name.strip_prefix(PREFIX).unwrap_or(name)).map(String::as_str)
    }
    pub fn cli_name(&self, original: &str) -> String {
        format!("{PREFIX}{}", bridge_name(original))
    }
    pub fn can_call(&self, name: &str) -> bool {
        self.original(name).is_some_and(|name| self.allowed.contains(name))
    }
}

fn bridge_name(name: &str) -> String {
    const MOST: usize = 64 - PREFIX.len();
    let suffix_hashed =
        name.rsplit_once('_').is_some_and(|(_, tail)| tail.len() == 12 && tail.bytes().all(|b| b.is_ascii_hexdigit()));
    if !name.is_empty()
        && name.len() <= MOST
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        && !suffix_hashed
    {
        return name.into();
    }
    let short: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') { ch } else { '_' })
        .take(MOST - 13)
        .collect();
    format!("{short}_{}", &process::digest(name.as_bytes())[..12])
}

#[derive(Clone, Debug)]
pub(super) struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ToolResult {
    pub content: Vec<Value>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
}

impl ToolResult {
    pub fn denied() -> Self {
        Self {
            content: vec![json!({"type":"text","text":"No tool can be used for this reply. Answer without tools."})],
            is_error: true,
            tools: None,
        }
    }
}

pub(super) struct IncomingCall {
    pub id: String,
    pub name: String,
    pub args: Value,
    pub response: oneshot::Sender<Result<ToolResult, GatewayError>>,
}

pub(super) fn validate_call(name: &str, args: &Value, tools: &ToolSet) -> Result<(), GatewayError> {
    if tools.original(name).is_none() {
        return Err(errors::protocol("Claude 请求了本次未声明的工具；未在网关执行"));
    }
    if !args.is_object() {
        return Err(errors::protocol("Claude 工具参数不是完整 JSON 对象"));
    }
    Ok(())
}
