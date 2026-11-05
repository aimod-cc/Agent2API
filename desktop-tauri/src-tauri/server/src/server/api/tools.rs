//! 工具服务管理 API（`/api/tools/*`，挂 protected）。
//!
//! - 「服务 × 协议」清单 / 开关 / 执行超时；
//! - 四家搜索提供商的密钥管理（只回 configured / key_hint，不回明文）、
//!   搜索 / 抓取提供商选择、连通性测试；
//! - 视觉分析的提供商 + 模型 + 思考等级 + 内部 Key 配置与测试；
//! - 本机接入：安装 / 卸载 Skill 与 MCP 配置，安装目标探测；
//! - 视觉本地桥接脚本的下载（面板安装时直接取用）。
//!
//! 运行时端点（`/skill/web-search*`、`/mcp/*`、`/api/web-search*`）在
//! `api::tool_runtime`，它们自己校验网关 Key，不走这里的面板鉴权。

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::server::core::api_keys;
use crate::server::core::providers::catalog;
use crate::server::core::tool_services::{self, VisionConfig};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

fn text_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> &'a str {
    object.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("")
}

/// 网关基址：优先取请求 Host（客户端看到的就是它要填的地址），没有则回环。
///
/// 分端口形态（`panel_port` 有值）下面板与网关不在同一端口：管理请求来自面板
/// 端口，但 Skill / MCP / 视觉回环要访问的是网关端口，所以把 Host 里的端口换成
/// `state.port` 再拼。单端口形态原样使用 Host（保留反代映射的域名与端口）。
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

/// 把 Host 里的端口换成指定端口（IPv6 的方括号形态单独处理）
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

/* ─── 服务 × 协议 ─────────────────────────────────────────── */

/// GET /api/tools/servers?protocol=skill|mcp
pub async fn get_servers(Query(query): Query<HashMap<String, String>>) -> Response {
    let protocol = query.get("protocol").map(String::as_str);
    if let Some(protocol) = protocol {
        if !tool_services::is_protocol(protocol) {
            return errors::management_error(400, "未知的工具服务协议");
        }
    }
    ok_json(Value::Array(tool_services::list_services(protocol)))
}

/// PATCH /api/tools/servers/{id} `{protocol, enabled?, timeout_seconds?}`
pub async fn patch_server(Path(id): Path<String>, body: Bytes) -> Response {
    if !tool_services::is_service(&id) {
        return errors::management_error(400, "未知的工具服务");
    }
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let protocol = text_field(object, "protocol");
    if !tool_services::is_protocol(protocol) {
        return errors::management_error(400, "protocol 必须是 skill / mcp 之一");
    }
    if !tool_services::service_protocols(&id).contains(&protocol) {
        return errors::management_error(400, format!("服务 {id} 不支持 {protocol} 协议"));
    }
    let enabled = object.get("enabled").and_then(Value::as_bool);
    let timeout = object.get("timeout_seconds").and_then(Value::as_u64);
    if enabled.is_none() && timeout.is_none() {
        return errors::management_error(400, "需要 enabled 或 timeout_seconds");
    }
    if let Some(seconds) = timeout {
        if !(tool_services::TIMEOUT_MIN_SECONDS..=tool_services::TIMEOUT_MAX_SECONDS).contains(&seconds) {
            return errors::management_error(
                400,
                format!(
                    "timeout_seconds 必须是 {}~{} 之间的整数",
                    tool_services::TIMEOUT_MIN_SECONDS,
                    tool_services::TIMEOUT_MAX_SECONDS
                ),
            );
        }
    }
    if let Some(enabled) = enabled {
        if let Err(error) = tool_services::set_enabled(&id, protocol, enabled) {
            return errors::management_error(400, error);
        }
    }
    if let Some(seconds) = timeout {
        if let Err(error) = tool_services::set_timeout(&id, seconds) {
            return errors::management_error(400, error);
        }
    }
    ok_json(tool_services::service_info(&id, protocol))
}

/* ─── 搜索提供商 ───────────────────────────────────────────── */

pub async fn get_providers(State(state): State<ServerState>) -> Response {
    ok_json(tool_services::list_providers(state.store()).await)
}

pub async fn get_selection() -> Response {
    ok_json(tool_services::provider_selection())
}

/// PUT /api/tools/provider-selection `{search_providers?, fetch_provider?}`
pub async fn put_selection(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let search = object.get("search_providers").map(|value| {
        value
            .as_array()
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    let fetch = object.get("fetch_provider").and_then(Value::as_str).map(str::to_string);
    match tool_services::update_selection(search, fetch) {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(400, error),
    }
}

/// PUT /api/tools/providers/{provider} `{api_key?, account_id?, enabled?}`
pub async fn put_provider(State(state): State<ServerState>, Path(provider): Path<String>, body: Bytes) -> Response {
    if !tool_services::is_search_provider(&provider) {
        return errors::management_error(400, "未知的搜索提供商");
    }
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let api_key = match object.get("api_key") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.as_str()),
        Some(_) => return errors::management_error(400, "api_key 必须是字符串"),
    };
    let enabled = match object.get("enabled") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return errors::management_error(400, "enabled 必须是布尔值"),
    };
    let account_id = match object.get("account_id") {
        None => None,
        Some(Value::Null) => Some(""),
        Some(Value::String(value)) => Some(value.as_str()),
        Some(_) => return errors::management_error(400, "account_id 必须是字符串或 null"),
    };
    match tool_services::update_provider(state.store(), &provider, api_key, enabled, account_id) {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(400, error),
    }
}

pub async fn delete_provider_key(Path(provider): Path<String>) -> Response {
    if !tool_services::is_search_provider(&provider) {
        return errors::management_error(400, "未知的搜索提供商");
    }
    if !tool_services::clear_provider_key(&provider) {
        return errors::management_error(404, "清除密钥失败");
    }
    ok_json(json!({ "success": true }))
}

pub async fn test_provider(State(state): State<ServerState>, Path(provider): Path<String>) -> Response {
    if !tool_services::is_search_provider(&provider) {
        return errors::management_error(400, "未知的搜索提供商");
    }
    ok_json(tool_services::test_provider(state.store(), &provider).await)
}

/* ─── 视觉分析 ─────────────────────────────────────────────── */

pub async fn get_vision_config() -> Response {
    ok_json(json!({
        "config": tool_services::vision_config(),
        "keys": tool_services::vision_key_options(),
    }))
}

/// GET /api/tools/api-keys —— 「本机接入」下拉用的可用网关 Key（只回 id/name/掩码）
pub async fn list_api_keys() -> Response {
    let keys: Vec<Value> = api_keys::list()
        .into_iter()
        .filter(api_keys::ApiKeyEntry::is_usable)
        .map(|entry| {
            json!({
                "id": entry.id,
                "name": entry.name,
                "masked": api_keys::mask(&entry.key),
            })
        })
        .collect();
    ok_json(json!({ "keys": keys }))
}

/// PUT /api/tools/vision/config `{provider, upstream_model, reasoning_effort?, api_key_id}`
pub async fn put_vision_config(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let provider = text_field(object, "provider").to_string();
    let upstream_model = text_field(object, "upstream_model").to_string();
    let reasoning = text_field(object, "reasoning_effort").to_string();
    let api_key_id = text_field(object, "api_key_id").to_string();
    if provider.is_empty() || upstream_model.is_empty() {
        return errors::management_error(400, "请选择提供商与上游模型");
    }
    if !tool_services::is_reasoning_level(&reasoning) {
        return errors::management_error(400, "思考等级无效");
    }
    if api_key_id.is_empty() {
        return errors::management_error(400, "请选择内部调用 API Key");
    }

    // 提供商与模型从模型管理的当前视图里取：只接受「这家、这个已启用的模型」
    let view = catalog::manage_view(&state.store());
    let provider_label = view
        .get("providers")
        .and_then(Value::as_array)
        .and_then(|providers| {
            providers
                .iter()
                .find(|item| item.get("provider").and_then(Value::as_str) == Some(provider.as_str()))
        })
        .and_then(|item| item.get("providerLabel"))
        .and_then(Value::as_str)
        .unwrap_or(&provider)
        .to_string();
    let model_id = view
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models.iter().find(|item| {
                item.get("provider").and_then(Value::as_str) == Some(provider.as_str())
                    && item.get("id").and_then(Value::as_str) == Some(upstream_model.as_str())
                    && item.get("enabled").and_then(Value::as_bool) == Some(true)
                    && item.get("kind").and_then(Value::as_str) != Some("image")
            })
        })
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(model_id) = model_id else {
        return errors::management_error(400, "该提供商下没有这个已启用的模型，请先在模型管理页确认");
    };

    let config = VisionConfig {
        provider,
        provider_label,
        upstream_model,
        downstream_model: model_id,
        reasoning,
        api_key_id,
        api_key_name: String::new(),
    };
    match tool_services::save_vision_config(config) {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(400, error),
    }
}

pub async fn delete_vision_config() -> Response {
    tool_services::clear_vision_config();
    ok_json(json!({ "success": true }))
}

pub async fn test_vision(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    ok_json(tool_services::test_vision(&base_url(&headers, &state)).await)
}

/* ─── 本机接入 ─────────────────────────────────────────────── */

/// GET /api/tools/install-targets?service=web-search&protocol=skill|mcp
pub async fn install_targets(Query(query): Query<HashMap<String, String>>) -> Response {
    let service = query.get("service").map(String::as_str).unwrap_or("");
    let protocol = query.get("protocol").map(String::as_str).unwrap_or("");
    if !tool_services::is_service(service) {
        return errors::management_error(400, "未知的工具服务");
    }
    if !tool_services::service_protocols(service).contains(&protocol) {
        return errors::management_error(400, "该服务不支持这个协议");
    }
    let targets = match protocol {
        "skill" => tool_services::skill_targets(service),
        "mcp" => tool_services::mcp_targets(service),
        _ => Vec::new(),
    };
    ok_json(json!({ "targets": targets }))
}

/// GET /api/tools/install-info?service=web-search
pub async fn install_info(State(state): State<ServerState>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>) -> Response {
    let service = query.get("service").map(String::as_str).unwrap_or("");
    if !tool_services::is_service(service) {
        return errors::management_error(400, "未知的工具服务");
    }
    let base = base_url(&headers, &state);
    let tools: Vec<&str> = if service == tool_services::WEB_SEARCH_SERVICE_ID {
        vec!["web_search", "web_fetch"]
    } else {
        vec!["image_analyze"]
    };
    let mut value = json!({
        "base_url": base,
        "service": service,
        "name": tool_services::package_name(service),
        "version": tool_services::version(service),
        "timeout_seconds": tool_services::timeout_seconds(service),
        "mcp": {
            "server_name": tool_services::package_name(service),
            "url": format!("{}{}", base.trim_end_matches('/'), tool_services::endpoint(service, "mcp")),
            "tools": tools,
            "bridge_url": if service == tool_services::VISION_SERVICE_ID { format!("{}/api/tools/mcp-bridge/vision.js", base.trim_end_matches('/')) } else { String::new() },
        },
    });
    if service == tool_services::WEB_SEARCH_SERVICE_ID {
        value["skill"] = json!({
            "name": tool_services::WEB_SEARCH_SKILL_NAME,
            "version": tool_services::WEB_SEARCH_SKILL_VERSION,
            "recommended_variant": tool_services::recommended_skill_variant(),
            "variants": tool_services::skill_variants(),
        });
    }
    ok_json(value)
}

fn plain_key(key_id: &str) -> Result<String, Response> {
    api_keys::list()
        .into_iter()
        .find(|entry| entry.id == key_id && entry.is_usable())
        .map(|entry| entry.key)
        .ok_or_else(|| errors::management_error(400, "所选 API Key 不存在、已停用或不可用"))
}

fn client_ids(object: &serde_json::Map<String, Value>) -> Vec<String> {
    object
        .get("clients")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn outcomes_json(outcomes: Vec<tool_services::InstallOutcome>) -> Value {
    json!({ "results": outcomes.iter().map(tool_services::InstallOutcome::to_json).collect::<Vec<_>>() })
}

/// POST /api/tools/install-skill `{api_key_id, variant, clients}`
pub async fn install_skill(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let variant = text_field(object, "variant");
    if !tool_services::is_skill_variant(variant) {
        return errors::management_error(400, "Skill 运行时变体无效");
    }
    let key = match plain_key(text_field(object, "api_key_id")) {
        Ok(key) => key,
        Err(response) => return response,
    };
    let clients = client_ids(object);
    if clients.is_empty() {
        return errors::management_error(400, "请至少选择一个客户端");
    }
    let outcomes = tool_services::install_skill(
        tool_services::WEB_SEARCH_SERVICE_ID,
        variant,
        &base_url(&headers, &state),
        &key,
        &clients,
    );
    ok_json(outcomes_json(outcomes))
}

/// POST /api/tools/install-mcp `{service_id, mode?, api_key_id, clients}`
pub async fn install_mcp(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let service = text_field(object, "service_id");
    if !tool_services::is_service(service) {
        return errors::management_error(400, "未知的工具服务");
    }
    let mode = text_field(object, "mode");
    let mode = if mode.is_empty() { "http" } else { mode };
    if mode != "http" && mode != "bridge" {
        return errors::management_error(400, "接入方式必须是 http 或 bridge");
    }
    if mode == "bridge" && service != tool_services::VISION_SERVICE_ID {
        return errors::management_error(400, "只有视觉分析支持本地桥接");
    }
    let key = match plain_key(text_field(object, "api_key_id")) {
        Ok(key) => key,
        Err(response) => return response,
    };
    let clients = client_ids(object);
    if clients.is_empty() {
        return errors::management_error(400, "请至少选择一个客户端");
    }
    let outcomes = tool_services::install_mcp(
        service,
        mode,
        &base_url(&headers, &state),
        &key,
        &clients,
    );
    ok_json(outcomes_json(outcomes))
}

/// POST /api/tools/uninstall-skill `{service_id, clients}`
pub async fn uninstall_skill(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let clients = client_ids(object);
    if clients.is_empty() {
        return errors::management_error(400, "请至少选择一个客户端");
    }
    ok_json(outcomes_json(tool_services::uninstall_skill(tool_services::WEB_SEARCH_SERVICE_ID, &clients)))
}

/// POST /api/tools/uninstall-mcp `{service_id, clients}`
pub async fn uninstall_mcp(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let service = text_field(object, "service_id");
    if !tool_services::is_service(service) {
        return errors::management_error(400, "未知的工具服务");
    }
    let clients = client_ids(object);
    if clients.is_empty() {
        return errors::management_error(400, "请至少选择一个客户端");
    }
    ok_json(outcomes_json(tool_services::uninstall_mcp(service, &clients)))
}

/// GET /api/tools/mcp-bridge/vision.js —— 视觉本地桥接脚本（文本）
pub async fn vision_bridge_script() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        tool_services::VISION_BRIDGE_SCRIPT,
    )
        .into_response()
}
