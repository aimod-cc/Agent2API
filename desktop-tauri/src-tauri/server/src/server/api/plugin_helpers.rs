use std::collections::HashMap;

use axum::{body::Bytes, extract::Query, response::Response};
use serde_json::json;
use crate::server::{errors::management_error, http::{ok_json, parse_body}};
use crate::server::core::plugin_host::wasm::native;

pub async fn list(Query(query): Query<HashMap<String, String>>) -> Response {
    ok_json(native::snapshot(query.get("pack").map(String::as_str)).await)
}

pub async fn configure(body: Bytes) -> Response {
    if body.len() > 256 * 1024 { return management_error(413, "原生辅助授权输入过大"); }
    let value = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return management_error(400, error.message),
    };
    if !value.is_object() || value.get("target").is_none()
        || value.get("pack").is_some_and(|pack| pack.as_str().is_none_or(|id| id.is_empty() || id.len() > 128)) {
        return management_error(400, "原生辅助授权请求格式无效");
    }
    if !value["target"].is_null() && value["confirmed"] != true {
        return management_error(400, "原生辅助程序具有本机权限，请明确确认后再授权");
    }
    let pack = value["pack"].as_str();
    match native::grant(value["provider"].as_str().unwrap_or(""), value["helper"].as_str().unwrap_or(""), &value["target"], pack).await {
        Ok(saved) => {
            let mut result = native::snapshot(pack).await;
            result["saved"] = json!(saved);
            ok_json(result)
        }
        Err(message) => management_error(400, message),
    }
}
