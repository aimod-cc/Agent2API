//! 代理管理：手动地址 CRUD、Clash Verge 端点同步、启停与连通性测试。
//! 写操作返回最新代理列表，测试失败以 `success:false` 返回 200。

use axum::body::Bytes;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::egress;
use crate::server::core::proxies::normalize_account_proxy;
use crate::server::core::{proxy_pool, proxy_settings};
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

use super::accounts::proxy_error;

pub async fn list_proxies(state: &ServerState) -> Response {
    let snapshot = state.store().list_accounts();
    let accounts: Vec<Value> = snapshot.get("accounts").and_then(Value::as_array)
        .map(|items| items.iter().map(|account| json!({
            "id": account.get("id"),
            "name": account.get("displayName").filter(|value| value.as_str().is_some_and(|text| !text.is_empty()))
                .or_else(|| account.get("name")),
            "priority": account.get("priority"),
            "enabled": account.get("enabled"),
            "proxy": account.get("proxy"),
        })).collect()).unwrap_or_default();
    ok_json(json!({
        "accounts": accounts,
        "clash": crate::server::core::proxies::clash_proxy_options(),
        "note": "支持手动 HTTP / SOCKS5 地址与 Clash Verge 端点；未配置代理时直连",
    }))
}

pub async fn system_proxy() -> Response {
    let mut response = match tokio::task::spawn_blocking(crate::server::core::proxies::system::snapshot).await {
        Ok(Ok(value)) => ok_json(value),
        Ok(Err(message)) => management_error(400, message),
        Err(_) => management_error(500, "读取系统代理失败"),
    };
    response.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    response
}

pub async fn test_proxy(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let mut proxy = None;
    if let Some(id) = payload.get("id").and_then(Value::as_str).filter(|value| !value.is_empty()) {
        let Some(creds) = state.store().get_credentials_by_id(id) else {
            return management_error(404, "账号不存在");
        };
        if let Some(error) = creds.proxy_error {
            return management_error(400, error);
        }
        match crate::server::core::proxies::ResolvedProxy::from_json(&creds.proxy) {
            Ok(resolved) => proxy = resolved,
            Err(reason) => return management_error(400, reason),
        }
    } else if let Some(config_value) = payload.get("proxy") {
        let config = match normalize_account_proxy(config_value) {
            Ok(value) => value,
            Err(error) => return proxy_error(error),
        };
        if let Some(config) = config {
            if let Some(resolution) = crate::server::core::proxies::resolve_account_proxy(Some(&config)) {
                if let Some(error) = resolution.error() {
                    return management_error(400, error);
                }
                proxy = resolution.resolved().cloned();
            }
        }
    }
    let result = egress::test_proxy(proxy.as_ref()).await;
    logging::log("[Accounts]", &format!("代理测试{}: {}",
        proxy.as_ref().map(|proxy| format!("（{}）", proxy.label)).unwrap_or_else(|| "（直连）".to_string()),
        if result.success { format!("✅ 出口 IP {}", result.ip) } else { format!("❌ {}", result.error.clone().unwrap_or_default()) },
    ));
    let mut data = result.to_json();
    data["proxy"] = egress::describe_public(proxy.as_ref());
    ok_json(data)
}

fn pool_references(state: &ServerState) -> std::collections::HashMap<String, Vec<Value>> {
    let mut references: std::collections::HashMap<String, Vec<Value>> = std::collections::HashMap::new();
    let snapshot = state.store().list_accounts();
    let Some(accounts) = snapshot.get("accounts").and_then(Value::as_array) else {
        return references;
    };
    for account in accounts {
        let Some(config) = account.get("proxy").and_then(|proxy| proxy.get("config")) else { continue };
        let ids: Vec<String> = if crate::server::core::proxies::is_rotate_config(config) {
            crate::server::core::proxy_rotate::candidate_ids(config)
        } else {
            proxy_pool::usage::pool_reference_id(config).map(|id| vec![id.to_string()]).unwrap_or_default()
        };
        let entry = json!({
            "id": account.get("id"),
            "name": account.get("displayName").filter(|value| value.as_str().is_some_and(|text| !text.is_empty()))
                .or_else(|| account.get("name")),
            "enabled": account.get("enabled").cloned().unwrap_or(Value::Bool(true)),
        });
        for id in ids {
            references.entry(id).or_default().push(entry.clone());
        }
    }
    references
}

fn pool_payload(state: &ServerState) -> Value {
    let references = pool_references(state);
    let service_references = proxy_pool::usage::service_references();
    let items: Vec<Value> = proxy_pool::list().into_iter().map(|mut item| {
        let id = item.get("id").and_then(Value::as_str).unwrap_or("");
        let used = references.get(id).cloned().unwrap_or_default();
        let services: Vec<Value> = service_references.iter().filter(|(_, proxy_id)| id == proxy_id)
            .map(|(service, _)| Value::String((*service).to_string())).collect();
        item["usedBy"] = json!(used);
        item["usedByServices"] = json!(services);
        item
    }).collect();
    json!({ "items": items, "clash": crate::server::core::proxies::clash_proxy_options() })
}

pub async fn pool_list(state: &ServerState) -> Response {
    let _ = proxy_pool::sync_clash(false);
    ok_json(pool_payload(state))
}

pub async fn pool_sync_clash(state: &ServerState) -> Response {
    let report = proxy_pool::sync_clash(true);
    let mut data = pool_payload(state);
    data["changes"] = json!(report.changes);
    data["syncError"] = json!(report.error);
    ok_json(data)
}

pub async fn pool_create(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    if payload.get("items").is_some() {
        return match proxy_pool::create_batch(&payload) {
            Ok(report) => {
                let mut data = pool_payload(state);
                data["imported"] = report;
                ok_json(data)
            }
            Err(message) => management_error(400, message),
        };
    }
    match proxy_pool::create(&payload) {
        Ok(_) => ok_json(pool_payload(state)),
        Err(message) => management_error(400, message),
    }
}

pub async fn pool_update(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    match proxy_pool::update(&payload) {
        Ok(_) => ok_json(pool_payload(state)),
        Err(message) => management_error(400, message),
    }
}

pub async fn pool_remove(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    if id.trim().is_empty() {
        return management_error(400, "缺少代理 id");
    }
    let exists = proxy_pool::list().iter().any(|item| item.get("id").and_then(Value::as_str) == Some(id));
    match proxy_pool::remove(id) {
        Ok((_, name)) => {
            let mut data = pool_payload(state);
            data["removed"] = json!(name);
            ok_json(data)
        }
        Err(message) => management_error(if exists { 400 } else { 404 }, message),
    }
}

pub async fn pool_enable(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    if id.trim().is_empty() {
        return management_error(400, "缺少代理 id");
    }
    let Some(enabled) = payload.get("enabled").and_then(Value::as_bool) else {
        return management_error(400, "缺少 enabled（true / false）");
    };
    match proxy_pool::set_enabled(id, enabled) {
        Ok((_, change)) => {
            let mut data = pool_payload(state);
            data["name"] = json!(change.name);
            data["enabled"] = json!(change.enabled);
            ok_json(data)
        }
        Err(message) => management_error(400, message),
    }
}

pub async fn pool_test(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    let proxy = match proxy_pool::resolve_for_test(id) {
        Ok(proxy) => proxy,
        Err(message) => return management_error(400, message),
    };
    let result = egress::test_proxy(Some(&proxy)).await;
    let write_error = proxy_pool::record_test(id, &proxy_pool::TestOutcome {
        success: result.success,
        ip: result.ip.clone(),
        duration_ms: result.duration_ms,
        error: result.error.clone(),
    }).err();
    logging::log("[Proxies]", &format!("代理测试（{}）: {}", proxy.label,
        if result.success { format!("✅ 出口 IP {}", result.ip) } else { format!("❌ {}", result.error.clone().unwrap_or_default()) },
    ));
    let mut data = result.to_json();
    data["proxy"] = egress::describe_public(Some(&proxy));
    if let Some(error) = write_error { data["saveError"] = json!(error); }
    data["items"] = pool_payload(state)["items"].clone();
    ok_json(data)
}

pub async fn settings_get() -> Response {
    ok_json(proxy_settings::get())
}

pub async fn settings_save(body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    match proxy_settings::save(&payload) {
        Ok(value) => ok_json(value),
        Err(message) => management_error(400, message),
    }
}
