use axum::http::HeaderMap;
use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::api_keys;
use crate::server::core::key_scope::KeyScope;
use crate::server::errors::GatewayError;
use crate::server::request_stats::{ApiKeyIdentity, RequestSource};

pub const MODEL_TEST: &str = "model_test";
pub const ASSISTANT: &str = "assistant";
pub const ASSISTANT_IMAGES: &str = "assistant_images";
pub const ASSISTANT_TITLE: &str = "assistant_title";
pub const ENV_KEY_ID: &str = "env";

pub fn is_model_endpoint(path: &str) -> bool {
    matches!(path, "/api/models/test" | "/api/model-access/models" | "/api/assistant/chat" | "/api/assistant/images/generations" | "/api/assistant/images/edits")
        || path.strip_prefix("/api/assistant/sessions/").is_some_and(|tail| tail.ends_with("/title"))
        // 工具端点自行校验网关 Key；账号型搜索在运行时校验提供商权限，
        // 独立搜索 API 和网页抓取不使用模型白名单。
        || path.starts_with("/api/web-search")
        || path.starts_with("/skill/")
        || path.starts_with("/mcp/")
}

pub struct ModelAccess {
    pub scope: Option<KeyScope>,
    pub source: RequestSource,
}

pub fn authenticate(headers: &HeaderMap, client: &str) -> Result<ModelAccess, GatewayError> {
    // 显式选择的 x-api-key 不得被 Authorization 中的管理凭证兜底替换。
    let presented = if let Some(value) = headers.get("x-api-key") {
        value.to_str().ok()
    } else {
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(crate::server::http::strip_bearer_prefix)
    };
    let key = presented.filter(|key| !key.is_empty()).ok_or_else(|| {
        GatewayError::with_status(401, "模型调用必须选择并携带 API Key，请先在「网关 Key」页创建并启用一把 Key")
            .with_code("model_api_key_required")
    })?;
    let snapshot = config::current();
    let (scope, identity) = if config::parse::env_api_key_value().as_deref() == Some(key) {
        (None, ApiKeyIdentity { source: "env".to_string(), ..Default::default() })
    } else {
        let entry = api_keys::entries_from(snapshot.raw())
            .into_iter()
            .find(|entry| entry.is_usable() && entry.key == key)
            .ok_or_else(|| {
                GatewayError::with_status(401, "所选 API Key 已失效、停用或不可用，请重新选择一把启用的 Key")
                    .with_code("invalid_api_key")
            })?;
        (Some(KeyScope::from_entry(&entry)), ApiKeyIdentity {
            id: entry.id,
            name: entry.name,
            source: "key".to_string(),
        })
    };
    Ok(ModelAccess {
        scope,
        source: RequestSource { client: client.to_string(), api_key: identity, ..Default::default() },
    })
}

pub async fn models(
    axum::extract::State(state): axum::extract::State<crate::server::ServerState>,
    headers: HeaderMap,
) -> axum::response::Response {
    match authenticate(&headers, "") {
        Ok(access) => crate::server::http::ok_json(
            crate::server::core::providers::catalog::models_response(state.store(), access.scope.as_ref()),
        ),
        Err(error) => error.payload_response(),
    }
}

pub async fn list_keys() -> axum::response::Response {
    crate::server::http::ok_json(json!({ "keys": key_choices() }))
}

pub fn key_choices() -> Vec<Value> {
    let mut choices: Vec<Value> = api_keys::list()
        .into_iter()
        .filter(api_keys::ApiKeyEntry::is_usable)
        .map(|entry| {
            let mut value = entry.public_json();
            value["source"] = json!("key");
            value
        })
        .collect();
    if let Some(key) = config::parse::env_api_key_value() {
        choices.push(json!({
            "id": ENV_KEY_ID,
            "name": "环境变量 Key",
            "masked": api_keys::mask(&key),
            "key": key,
            "source": "env",
            "enabled": true,
            "allowedProviders": [],
            "allowedModels": [],
        }));
    }
    choices
}
