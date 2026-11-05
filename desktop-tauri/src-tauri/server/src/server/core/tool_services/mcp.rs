//! MCP 端点的 JSON-RPC 骨架（Streamable HTTP，单端点 POST，dual-era）。
//!
//! 两个服务（网络搜索 / 视觉分析）共用同一套分发：initialize 走 legacy 握手，
//! `server/discover` 与直接调用走 modern（无握手、无会话）。端点本身无状态，
//! 两代语义可以共存而不需要维护会话。
//!
//! 协议细节与 OmniProxy 的 `mcpProtocol` 一致：
//!   · 通知（`notifications/*` 或没有 id）不产生响应；
//!   · 批量数组（2025-03-26）与单条消息都接；
//!   · 参数不合法 → JSON-RPC `INVALID_PARAMS`；业务失败 → `isError` 结果，
//!     让模型有机会自我纠正。

use serde_json::{json, Value};

use super::search::batch_items;
use crate::server::logging;

pub const MCP_MODERN_VERSION: &str = "2026-07-28";
pub const MCP_LEGACY_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
pub const MCP_LIST_TTL_MS: u64 = 3_600_000;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum McpServerKind {
    WebSearch,
    Vision,
}

pub struct McpRequestContext {
    /// 网关基址（视觉分析内部调用用）
    pub base_url: String,
    pub store: crate::server::core::account_store::AccountStore,
    pub scope: Option<crate::server::core::key_scope::KeyScope>,
}

pub struct McpSpec {
    pub server_info: Value,
    pub instructions: &'static str,
    pub tools: Vec<Value>,
}

pub fn web_search_spec() -> McpSpec {
    McpSpec {
        server_info: json!({
            "name": super::WEB_SEARCH_PACKAGE,
            "title": "Web Search",
            "version": super::WEB_SEARCH_VERSION,
        }),
        instructions: "Batched web search and page fetching. Upstream providers are chosen by the Agent2API administrator; callers only pass queries or urls.",
        tools: vec![
            json!({
                "name": "web_search",
                "title": "Batched web search",
                "description": "Search 1-10 queries in parallel using the providers selected by the administrator, then merge duplicate sources across queries.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "queries": {
                            "type": "array",
                            "description": "Queries to search, each executed in parallel (1-10).",
                            "items": { "type": "string", "minLength": 1 },
                            "minItems": 1,
                            "maxItems": 10
                        }
                    },
                    "required": ["queries"],
                    "additionalProperties": false
                }
            }),
            json!({
                "name": "web_fetch",
                "title": "Batched page fetching",
                "description": "Fetch and extract the readable content of 1-10 public http/https URLs using the single provider selected by the administrator.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "urls": {
                            "type": "array",
                            "description": "Page URLs to fetch and extract (1-10).",
                            "items": { "type": "string", "minLength": 1 },
                            "minItems": 1,
                            "maxItems": 10
                        }
                    },
                    "required": ["urls"],
                    "additionalProperties": false
                }
            }),
        ],
    }
}

pub fn vision_spec() -> McpSpec {
    McpSpec {
        server_info: json!({
            "name": super::VISION_PACKAGE,
            "title": "Vision Analysis",
            "version": super::VISION_VERSION,
        }),
        instructions: "Analyze images with a vision model selected by the Agent2API administrator. Pass image URLs or data URIs together with a prompt; the model reply is returned as text.",
        tools: vec![json!({
            "name": "image_analyze",
            "title": "Image analysis",
            "description": "Analyze 1-10 images with the vision model selected by the administrator. Each image is an http(s) URL or an image data URI; the prompt describes what to look for.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "images": {
                        "type": "array",
                        "description": "Images to analyze (1-10), each an http(s) URL or an image data URI.",
                        "items": { "type": "string", "minLength": 1 },
                        "minItems": 1,
                        "maxItems": 10
                    },
                    "prompt": {
                        "type": "string",
                        "description": "What to look for in the images; the model reply is returned as text.",
                        "minLength": 1
                    }
                },
                "required": ["images", "prompt"],
                "additionalProperties": false
            }
        })],
    }
}

pub fn spec(kind: McpServerKind) -> McpSpec {
    match kind {
        McpServerKind::WebSearch => web_search_spec(),
        McpServerKind::Vision => vision_spec(),
    }
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

fn tool_result(payload: Value) -> Value {
    json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default() }],
        "structuredContent": payload,
    })
}

fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true,
    })
}

fn negotiate_version(requested: Option<&str>) -> String {
    if let Some(value) = requested {
        if value == MCP_MODERN_VERSION || MCP_LEGACY_VERSIONS.contains(&value) {
            return value.to_string();
        }
    }
    MCP_LEGACY_VERSIONS[0].to_string()
}

async fn call_tool(
    kind: McpServerKind,
    name: &str,
    args: &Value,
    ctx: &McpRequestContext,
) -> Result<Value, ToolCallError> {
    match kind {
        McpServerKind::WebSearch => {
            let operation = if name == "web_search" {
                "search"
            } else if name == "web_fetch" {
                "fetch"
            } else {
                return Err(ToolCallError::InvalidParams(format!("Unknown tool: {name}")));
            };
            let field = if operation == "search" { "queries" } else { "urls" };
            let items = batch_items(args.get(field), field)
                .map_err(|error| ToolCallError::InvalidParams(error.message))?;
            let started = logging::now_ms();
            let outcome = if operation == "search" {
                super::run_search(&ctx.store, ctx.scope.as_ref(), items.clone())
                    .await
                    .map(|run| (run.payload, run.successful))
            } else {
                super::run_fetch(items.clone())
                    .await
                    .map(|run| (run.payload, run.successful))
            };
            let duration = logging::now_ms() - started;
            match outcome {
                Ok((payload, successful)) => {
                    logging::log(
                        "[Tools]",
                        &format!("MCP web-search {operation}: {} 条输入 / {successful} 条成功 / {duration}ms", items.len()),
                    );
                    if successful == 0 {
                        let mut result = tool_error(format!("All {operation} items failed"));
                        result["structuredContent"] = payload;
                        return Ok(result);
                    }
                    Ok(tool_result(payload))
                }
                Err(error) => {
                    logging::log(
                        "[Tools]",
                        &format!("MCP web-search {operation} 失败：{}", error.message),
                    );
                    Ok(tool_error(error.message))
                }
            }
        }
        McpServerKind::Vision => {
            if name != "image_analyze" {
                return Err(ToolCallError::InvalidParams(format!("Unknown tool: {name}")));
            }
            let (images, prompt) = super::vision::parse_vision_args(args)
                .map_err(ToolCallError::InvalidParams)?;
            match super::run_vision(&ctx.base_url, images, prompt).await {
                Ok(payload) => Ok(tool_result(payload)),
                Err(error) => Ok(tool_error(error)),
            }
        }
    }
}

enum ToolCallError {
    InvalidParams(String),
}

/// 处理单条 JSON-RPC 请求；通知类消息返回 None
async fn dispatch_one(
    message: &Value,
    kind: McpServerKind,
    ctx: &McpRequestContext,
) -> Option<Value> {
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(rpc_error(Value::Null, INVALID_REQUEST, "Invalid JSON-RPC 2.0 request"));
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(rpc_error(Value::Null, INVALID_REQUEST, "Invalid JSON-RPC 2.0 request"));
    };
    if method.starts_with("notifications/") {
        return None;
    }
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    if id.is_null() {
        return None;
    }
    let spec = spec(kind);
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(Value::as_str);
            Some(rpc_result(id, json!({
                "protocolVersion": negotiate_version(requested),
                "capabilities": { "tools": {} },
                "serverInfo": spec.server_info,
                "instructions": spec.instructions,
            })))
        }
        "server/discover" => Some(rpc_result(id, json!({
            "resultType": "complete",
            "ttlMs": MCP_LIST_TTL_MS,
            "cacheScope": "public",
            "supportedVersions": [MCP_MODERN_VERSION, MCP_LEGACY_VERSIONS[0], MCP_LEGACY_VERSIONS[1], MCP_LEGACY_VERSIONS[2], MCP_LEGACY_VERSIONS[3]],
            "capabilities": { "tools": {} },
            "_meta": { "io.modelcontextprotocol/serverInfo": spec.server_info },
            "instructions": spec.instructions,
        }))),
        "ping" => Some(rpc_result(id, json!({}))),
        "tools/list" => Some(rpc_result(id, json!({
            "resultType": "complete",
            "ttlMs": MCP_LIST_TTL_MS,
            "cacheScope": "public",
            "tools": spec.tools,
        }))),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            if !spec.tools.iter().any(|tool| tool.get("name").and_then(Value::as_str) == Some(name)) {
                return Some(rpc_error(id, INVALID_PARAMS, format!("Unknown tool: {name}")));
            }
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match call_tool(kind, name, &args, ctx).await {
                Ok(result) => Some(rpc_result(id, result)),
                Err(ToolCallError::InvalidParams(message)) => Some(rpc_error(id, INVALID_PARAMS, message)),
            }
        }
        _ => Some(rpc_error(id, METHOD_NOT_FOUND, format!("Method not found: {method}"))),
    }
}

/// 处理一个请求体（单条或批量），返回响应体；纯通知返回 None
pub async fn dispatch(
    body: &Value,
    kind: McpServerKind,
    ctx: &McpRequestContext,
) -> Option<Value> {
    if body.is_null() || (!body.is_object() && !body.is_array()) {
        return Some(rpc_error(Value::Null, PARSE_ERROR, "Request body must be a JSON-RPC 2.0 message"));
    }
    let messages: Vec<&Value> = match body {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let mut responses = Vec::new();
    for message in messages {
        if let Some(response) = dispatch_one(message, kind, ctx).await {
            responses.push(response);
        }
    }
    if responses.is_empty() {
        return None;
    }
    if body.is_array() {
        Some(Value::Array(responses))
    } else {
        Some(responses.remove(0))
    }
}
