//! 工具服务的运行时端点（对外给客户端调用，自己校验网关 Key）。
//!
//! 与 OmniProxy 的端点路径逐字对齐（客户端接入地址写死，改动会影响已接入方）：
//!   · `GET  /skill/web-search`           服务元数据（免鉴权）
//!   · `POST /skill/web-search/search`    批量搜索
//!   · `POST /skill/web-search/fetch`     批量抓取
//!   · `POST /mcp/web-search`             网络搜索 MCP（Streamable HTTP）
//!   · `POST /mcp/vision`                 视觉分析 MCP
//!   · `GET  /api/web-search`             REST 元数据（免鉴权）
//!   · `POST /api/web-search/search|fetch`
//!
//! 鉴权复用网关 Key（`Authorization: Bearer <key>` 或 `x-api-key`），与模型调用
//! 同一把钥匙；账号型搜索遵守 Key 的提供商白名单，独立搜索 API 不按模型过滤。

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::server::api::model_access;
use crate::server::core::key_scope::KeyScope;
use crate::server::core::tool_services::{self, McpRequestContext, McpServerKind, SearchErrorCode};
use crate::server::http::parse_body;
use crate::server::ServerState;

fn base_url(headers: &HeaderMap, state: &ServerState) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("127.0.0.1:{}", state.port));
    let host = if state.panel_port.is_some() { with_port(&host, state.port) } else { host };
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .filter(|value| *value == "https")
        .unwrap_or("http");
    format!("{scheme}://{host}")
}

fn with_port(host: &str, port: u16) -> String {
    if let Some(stripped) = host.strip_prefix('[') {
        if let Some(end) = stripped.find(']') {
            return format!("[{}]:{port}", &stripped[..end]);
        }
    }
    match host.rsplit_once(':') {
        Some((name, tail)) if !name.is_empty() && tail.chars().all(|ch| ch.is_ascii_digit()) => {
            format!("{name}:{port}")
        }
        _ => format!("{host}:{port}"),
    }
}

fn authorized(headers: &HeaderMap) -> bool {
    model_access::authenticate(headers, "tool").is_ok()
}

fn json_response(status: StatusCode, body: Value) -> Response {
    (status, axum::Json(body)).into_response()
}

fn search_error_response(error: &tool_services::SearchError) -> Response {
    json_response(
        StatusCode::from_u16(error.code.http_status()).unwrap_or(StatusCode::BAD_GATEWAY),
        json!({ "error": { "code": error.code.as_str(), "message": error.message } }),
    )
}

fn skill_disabled() -> Response {
    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({ "error": "Skill service is disabled", "code": "skill_service_disabled" }),
    )
}

fn api_disabled() -> Response {
    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({ "error": { "code": "api_service_disabled", "message": "API service is disabled" } }),
    )
}

/* ─── Skill 端点 ───────────────────────────────────────────── */

/// GET /skill/web-search
pub async fn skill_metadata(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    if !tool_services::is_enabled(tool_services::WEB_SEARCH_SERVICE_ID, "skill") {
        return skill_disabled();
    }
    let base = base_url(&headers, &state);
    json_response(
        StatusCode::OK,
        json!({
            "name": tool_services::WEB_SEARCH_SKILL_NAME,
            "version": tool_services::WEB_SEARCH_SKILL_VERSION,
            "recommended_variant": tool_services::recommended_skill_variant(),
            "variants": tool_services::skill_variants(),
            "endpoints": {
                "search": format!("{}/skill/web-search/search", base.trim_end_matches('/')),
                "fetch": format!("{}/skill/web-search/fetch", base.trim_end_matches('/')),
            },
        }),
    )
}

async fn skill_operation(state: &ServerState, scope: Option<&KeyScope>, body: Bytes, operation: &str) -> Response {
    if !tool_services::is_enabled(tool_services::WEB_SEARCH_SERVICE_ID, "skill") {
        return skill_disabled();
    }
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": { "code": "invalid_input", "message": error.message } }),
            )
        }
    };
    let field = if operation == "search" { "queries" } else { "urls" };
    let items = match tool_services::batch_items(payload.get(field), field) {
        Ok(items) => items,
        Err(error) => return search_error_response(&error),
    };
    let outcome = if operation == "search" {
        tool_services::run_search(state.store(), scope, items)
            .await
            .map(|run| (run.payload, run.successful))
    } else {
        tool_services::run_fetch(items)
            .await
            .map(|run| (run.payload, run.successful))
    };
    match outcome {
        Ok((payload, successful)) => {
            let status = if successful == 0 { StatusCode::BAD_GATEWAY } else { StatusCode::OK };
            json_response(status, payload)
        }
        Err(error) => search_error_response(&error),
    }
}

/// POST /skill/web-search/search
pub async fn skill_search(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(access) = model_access::authenticate(&headers, "tool") else {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": { "code": "unauthorized", "message": "Missing or invalid API key. Pass it as \"Authorization: Bearer <Agent2API API Key>\"." } }),
        );
    };
    skill_operation(&state, access.scope.as_ref(), body, "search").await
}

/// POST /skill/web-search/fetch
pub async fn skill_fetch(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    if !authorized(&headers) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": { "code": "unauthorized", "message": "Missing or invalid API key. Pass it as \"Authorization: Bearer <Agent2API API Key>\"." } }),
        );
    }
    skill_operation(&state, None, body, "fetch").await
}

/* ─── REST API 端点 ────────────────────────────────────────── */

/// GET /api/web-search
pub async fn api_metadata(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    if !tool_services::is_enabled(tool_services::WEB_SEARCH_SERVICE_ID, "api") {
        return api_disabled();
    }
    let base = base_url(&headers, &state);
    json_response(
        StatusCode::OK,
        json!({
            "name": tool_services::WEB_SEARCH_SKILL_NAME,
            "version": tool_services::WEB_SEARCH_SKILL_VERSION,
            "auth": "Authorization: Bearer <Agent2API API Key>",
            "endpoints": {
                "search": format!("{}/api/web-search/search", base.trim_end_matches('/')),
                "fetch": format!("{}/api/web-search/fetch", base.trim_end_matches('/')),
            },
        }),
    )
}

async fn api_operation(state: &ServerState, scope: Option<&KeyScope>, body: Bytes, operation: &str) -> Response {
    if !tool_services::is_enabled(tool_services::WEB_SEARCH_SERVICE_ID, "api") {
        return api_disabled();
    }
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": { "code": "invalid_input", "message": error.message } }),
            )
        }
    };
    let field = if operation == "search" { "queries" } else { "urls" };
    let items = match tool_services::batch_items(payload.get(field), field) {
        Ok(items) => items,
        Err(error) => return search_error_response(&error),
    };
    let outcome = if operation == "search" {
        tool_services::run_search(state.store(), scope, items)
            .await
            .map(|run| (run.payload, run.successful))
    } else {
        tool_services::run_fetch(items)
            .await
            .map(|run| (run.payload, run.successful))
    };
    match outcome {
        Ok((payload, successful)) => {
            let status = if successful == 0 { StatusCode::BAD_GATEWAY } else { StatusCode::OK };
            json_response(status, payload)
        }
        Err(error) => search_error_response(&error),
    }
}

/// POST /api/web-search/search
pub async fn api_search(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(access) = model_access::authenticate(&headers, "tool") else {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": { "code": "unauthorized", "message": "Missing or invalid API key. Pass it as \"Authorization: Bearer <Agent2API API Key>\"." } }),
        );
    };
    api_operation(&state, access.scope.as_ref(), body, "search").await
}

/// POST /api/web-search/fetch
pub async fn api_fetch(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    if !authorized(&headers) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": { "code": "unauthorized", "message": "Missing or invalid API key. Pass it as \"Authorization: Bearer <Agent2API API Key>\"." } }),
        );
    }
    api_operation(&state, None, body, "fetch").await
}

/* ─── MCP 端点 ─────────────────────────────────────────────── */

async fn mcp_post(state: &ServerState, headers: HeaderMap, body: Bytes, kind: McpServerKind) -> Response {
    let Ok(access) = model_access::authenticate(&headers, "tool") else {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "jsonrpc": "2.0", "id": Value::Null, "error": { "code": -32600, "message": "Unauthorized" } }),
        );
    };
    let protocol = "mcp";
    let service = match kind {
        McpServerKind::WebSearch => tool_services::WEB_SEARCH_SERVICE_ID,
        McpServerKind::Vision => tool_services::VISION_SERVICE_ID,
    };
    if !tool_services::is_enabled(service, protocol) {
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "jsonrpc": "2.0", "id": Value::Null, "error": { "code": -32000, "message": "MCP service is disabled" } }),
        );
    }
    let payload = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "jsonrpc": "2.0", "id": Value::Null, "error": { "code": -32700, "message": "Request body must be a JSON-RPC 2.0 message" } }),
            )
        }
    };
    let context = McpRequestContext { base_url: base_url(&headers, &state), store: state.store().clone(), scope: access.scope };
    match tool_services::mcp_dispatch(&payload, kind, &context).await {
        Some(response) => json_response(StatusCode::OK, response),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// POST /mcp/web-search
pub async fn mcp_web_search(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    mcp_post(&state, headers, body, McpServerKind::WebSearch).await
}

/// POST /mcp/vision
pub async fn mcp_vision(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    mcp_post(&state, headers, body, McpServerKind::Vision).await
}

/// 非 POST 方法：按规范用 405 明确告知（本类服务不提供服务端主动推流）
pub async fn mcp_unsupported() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        axum::Json(json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": { "code": -32600, "message": "Only POST is supported on this MCP endpoint" }
        })),
    )
        .into_response()
}

/// 便于测试与文档引用：错误码 → HTTP 状态
pub fn status_for(code: SearchErrorCode) -> u16 {
    code.http_status()
}
