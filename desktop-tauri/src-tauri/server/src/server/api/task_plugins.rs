//! 定时任务类插件管理接口：`/api/task-plugins*`。

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::Path;
use axum::response::Response;
use base64::Engine;
use serde_json::{json, Value};

use crate::server::core::task_plugins::{scheduler, store, store_remote, types};
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};

pub async fn list() -> Response {
    match store::list_plugins() {
        Ok(plugins) => {
            let (store_items, updates) = store_remote::store_snapshot(&plugins);
            ok_json(json!({
                "plugins": plugins,
                "store": store_items,
                "updates": updates,
            }))
        }
        Err(error) => management_error(500, error),
    }
}

pub async fn create(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let plugin = match decode_plugin_payload(&payload, "local") {
        Ok(p) => p,
        Err(error) => return management_error(400, error),
    };
    match store::upsert_plugin(plugin, false) {
        Ok(saved) => ok_json(json!({ "plugin": saved })),
        Err(error) => management_error(400, error),
    }
}

pub async fn update(Path(id): Path<String>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let existing = match store::get_plugin(&id) {
        Ok(Some(p)) => p,
        Ok(None) => return management_error(404, "定时任务插件不存在"),
        Err(error) => return management_error(500, error),
    };
    let mut plugin = match decode_plugin_payload(&payload, &existing.source) {
        Ok(p) => p,
        Err(error) => return management_error(400, error),
    };
    plugin.id = existing.id;
    plugin.signed = existing.signed && existing.script_body == plugin.script_body;
    match store::upsert_plugin(plugin, false) {
        Ok(saved) => ok_json(json!({ "plugin": saved })),
        Err(error) => management_error(400, error),
    }
}

pub async fn patch(Path(id): Path<String>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let enabled = payload.get("enabled").and_then(Value::as_bool);
    let notify_allowed = payload.get("notifyAllowed").and_then(Value::as_bool);
    if enabled.is_none() && notify_allowed.is_none() {
        return management_error(400, "没有需要更新的字段");
    }
    match store::patch_plugin(&id, enabled, notify_allowed) {
        Ok(saved) => ok_json(json!({ "plugin": saved })),
        Err(error) => management_error(400, error),
    }
}

pub async fn remove(Path(id): Path<String>) -> Response {
    match store::delete_plugin(&id) {
        Ok(()) => ok_json(json!({ "deleted": id })),
        Err(error) => management_error(400, error),
    }
}

pub async fn export_single(Path(id): Path<String>) -> Response {
    match store::get_plugin(&id) {
        Ok(Some(plugin)) => ok_json(json!({
            "filename": format!("{}-{}.task-plugin.js", plugin.id, plugin.version),
            "content": types::serialize_single_file(&plugin),
        })),
        Ok(None) => management_error(404, "定时任务插件不存在"),
        Err(error) => management_error(500, error),
    }
}

pub async fn import_plugin(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let parsed = if let Some(b64) = payload.get("zipBase64").and_then(Value::as_str) {
        let bytes = match base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
            Ok(b) => b,
            Err(_) => return management_error(400, "ZIP 文件编码无效"),
        };
        store_remote::import_zip_bytes(&bytes, false)
    } else if let Some(raw) = payload.get("rawScript").and_then(Value::as_str) {
        types::parse_single_file(raw, "third-party")
    } else {
        return management_error(400, "请提供单文件脚本内容或 ZIP 插件包");
    };
    let plugin = match parsed {
        Ok(p) => p,
        Err(error) => return management_error(400, error),
    };
    match store::upsert_plugin(plugin, true) {
        Ok(saved) => ok_json(json!({ "plugin": saved })),
        Err(error) => management_error(400, error),
    }
}

pub async fn install_store(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let id = payload["id"].as_str().unwrap_or("").trim();
    if id.is_empty() {
        return management_error(400, "缺少插件标识");
    }
    match store_remote::install_from_store(id).await {
        Ok(saved) => ok_json(json!({ "plugin": saved })),
        Err(error) => management_error(400, error),
    }
}

pub async fn parse_script(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let raw = payload["rawScript"].as_str().unwrap_or("");
    match types::parse_single_file(raw, "local") {
        Ok(plugin) => ok_json(json!({
            "plugin": plugin,
            "rawScript": types::serialize_single_file(&plugin),
        })),
        Err(error) => management_error(400, error),
    }
}

pub async fn preview(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let plugin_val = payload.get("plugin").cloned().unwrap_or(Value::Null);
    let plugin = match decode_plugin_payload(&plugin_val, "local") {
        Ok(p) => p,
        Err(error) => return management_error(400, error),
    };
    let action_id = payload
        .get("actionId")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    let form_values: BTreeMap<String, Value> = payload
        .get("values")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let proxy_id = payload["proxyId"].as_str().unwrap_or("").to_string();
    let storage: BTreeMap<String, String> = payload
        .get("storage")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    match scheduler::preview_plugin(plugin, action_id, form_values, proxy_id, storage).await {
        Ok(outcome) => ok_json(json!({
            "outcome": outcome,
            "storage": outcome.new_storage,
        })),
        Err(error) => management_error(400, error),
    }
}

fn decode_plugin_payload(payload: &Value, fallback_source: &str) -> Result<types::TaskPlugin, String> {
    if let Some(raw) = payload.get("rawScript").and_then(Value::as_str) {
        return types::parse_single_file(raw, fallback_source);
    }
    let mut plugin: types::TaskPlugin = serde_json::from_value(payload.clone())
        .map_err(|error| format!("定时任务插件数据格式无效：{error}"))?;
    if plugin.source.is_empty() {
        plugin.source = fallback_source.to_string();
    }
    types::validate_and_normalize_plugin(plugin)
}
