//! 旧面板地址兼容：只返回当前已同步或兜底的版本号，不再允许手动覆写版本。

use axum::{body::Bytes, response::Response};
use serde_json::json;

use crate::server::{
    core::{plugin_host::provider_settings, upstream_versions},
    errors::management_error,
    http::ok_json,
};

const PROVIDERS: &[&str] = &["zcode", "zcode-intl"];
const FIELD: &str = "appVersion";

pub async fn get_version() -> Response {
    match provider_settings::legacy_provider(PROVIDERS, FIELD).await {
        Ok(value) => {
            let fallback = value["schema"]["fields"]
                .as_array()
                .and_then(|fields| fields.iter().find(|field| field["key"] == FIELD))
                .and_then(|field| field["default"].as_str())
                .unwrap_or("");
            let version = upstream_versions::cached_version("zcode").unwrap_or_else(|| fallback.to_string());
            ok_json(json!({ "version": version, "envOverride": serde_json::Value::Null, "saved": true }))
        }
        Err(error) => management_error(error.status_code, error.message),
    }
}

pub async fn put_version(_body: Bytes) -> Response {
    management_error(400, "ZCode 客户端版本号已改为自动从官网同步，请在提供商设置中调整拉取间隔")
}
