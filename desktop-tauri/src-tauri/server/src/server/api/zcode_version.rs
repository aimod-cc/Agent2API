//! 旧面板地址只转换请求形状；规则和默认值来自当前插件声明。

use axum::{body::Bytes, response::Response};
use serde_json::{json, Value};

use crate::server::{core::plugin_host::provider_settings, errors::management_error, http::{ok_json, parse_body}};

const PROVIDERS: &[&str] = &["zcode", "zcode-intl"];
const FIELD: &str = "appVersion";

fn response(value: Value) -> Response {
    ok_json(json!({ "version": value["values"][FIELD].as_str().unwrap_or(""),
        "envOverride": value["overrides"][FIELD]["value"], "saved": value["saved"] }))
}

pub async fn get_version() -> Response {
    match provider_settings::legacy_provider(PROVIDERS, FIELD).await {
        Ok(value) => response(value),
        Err(error) => management_error(error.status_code, error.message),
    }
}

pub async fn put_version(body: Bytes) -> Response {
    if body.len() > 16 * 1024 { return management_error(413, "设置输入过大"); }
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return management_error(400, error.message),
    };
    let Some(version) = payload["version"].as_str() else { return management_error(400, "version 必须是字符串"); };
    let current = match provider_settings::legacy_provider(PROVIDERS, FIELD).await {
        Ok(value) => value,
        Err(error) => return management_error(error.status_code, error.message),
    };
    match provider_settings::configure(current["provider"].as_str().unwrap_or(""),
        current["revision"].as_str().unwrap_or(""), &json!({ FIELD: version })).await {
        Ok(value) => response(value),
        Err(error) => management_error(error.status_code, error.message),
    }
}
