use axum::body::Bytes;
use axum::response::Response;
use serde_json::Value;

use crate::server::config::runtime_capacity::{self, CapacitySettings};
use crate::server::core::runtime_capacity as capacity;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};

pub async fn get() -> Response { ok_json(capacity::snapshot()) }

pub async fn save(body: Bytes) -> Response {
    let value = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return management_error(400, error.message),
    };
    if value.as_object().is_none_or(|fields| fields.keys().any(|key| !matches!(key.as_str(), "settings" | "revision"))) {
        return management_error(400, "运行容量请求格式无效");
    }
    let Some(revision) = value.get("revision").and_then(Value::as_str).filter(|revision| revision.len() == 64) else {
        return management_error(400, "保存运行容量需要当前修订号");
    };
    let settings: CapacitySettings = match serde_json::from_value(value["settings"].clone()) {
        Ok(settings) => settings,
        Err(_) => return management_error(400, "运行容量配置格式无效"),
    };
    match runtime_capacity::save(settings, revision) {
        Ok(()) => ok_json(capacity::snapshot()),
        Err(error) => management_error(error.status_code, error.message),
    }
}
