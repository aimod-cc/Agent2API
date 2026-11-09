use axum::{body::Bytes, response::Response};
use serde_json::Value;

use crate::server::{core::plugin_host::provider_settings, errors::management_error, http::{ok_json, parse_body}};

pub async fn list() -> Response { ok_json(provider_settings::snapshot().await) }

pub async fn configure(body: Bytes) -> Response {
    if body.len() > 128 * 1024 { return management_error(413, "插件设置输入过大"); }
    let value = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return management_error(400, error.message),
    };
    if value.as_object().is_none_or(|object| object.keys().any(|key| !matches!(key.as_str(), "provider" | "revision" | "values"))) {
        return management_error(400, "插件设置请求格式无效");
    }
    match provider_settings::configure(value["provider"].as_str().unwrap_or(""),
        value["revision"].as_str().unwrap_or(""), value.get("values").unwrap_or(&Value::Null)).await {
        Ok(value) => ok_json(value),
        Err(error) => management_error(error.status_code, error.message),
    }
}
