//! 模型页的压缩路由设置。客户端身份与上游提供商是两个独立维度。

use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::config::compact_routes::{self, CompactRoutes};
use crate::server::core::providers::catalog;
use crate::server::errors::{self, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::{logging, ServerState};

fn available_models(state: &ServerState) -> Vec<Value> {
    catalog::session_models(state.store()).into_iter()
        .filter(|model| model.get("kind").and_then(Value::as_str) != Some("image"))
        .collect()
}

fn view(state: &ServerState) -> Result<Value, GatewayError> {
    let snapshot = compact_routes::current();
    let settings = snapshot.settings.as_ref().map_err(|error| GatewayError::new(error.clone()))?;
    Ok(json!({
        "config": settings,
        "revision": snapshot.revision,
        "clients": [{ "id": compact_routes::CLIENT_ZCODE, "label": "ZCode" }],
        "models": available_models(state),
        "maxRules": compact_routes::MAX_COMPACT_ROUTES,
    }))
}

pub async fn get_routes(State(state): State<ServerState>) -> Response {
    match tokio::task::spawn_blocking(move || view(&state)).await {
        Ok(Ok(value)) => ok_json(value),
        Ok(Err(error)) => errors::management_error(error.status_code, error.message),
        Err(error) => errors::management_error(500, error.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveRoutes {
    config: CompactRoutes,
    revision: String,
}

fn validate_models(state: &ServerState, settings: &CompactRoutes) -> Result<(), GatewayError> {
    let models = available_models(state);
    let available = |name: &str| models.iter().any(|model| model.get("id").and_then(Value::as_str)
        .is_some_and(|id| compact_routes::model_key(id) == compact_routes::model_key(name)));
    let old = compact_routes::current();
    let previous = old.settings.as_ref().ok();
    for rule in &settings.rules {
        let existing = previous.and_then(|settings| settings.rules.iter().find(|old|
            old.client == rule.client && compact_routes::model_key(&old.source) == compact_routes::model_key(&rule.source)));
        if !available(&rule.source) && existing.is_none() {
            return Err(GatewayError::bad_request(format!("来源模型「{}」当前不可用，请刷新模型列表后重试", rule.source)));
        }
        // 已失效的旧规则允许保留、关闭或删除，但不能把失效目标当作新选择或重新启用。
        let preserve_unavailable = existing.is_some_and(|old|
            compact_routes::model_key(&old.target) == compact_routes::model_key(&rule.target)
                && (!settings.enabled || !rule.enabled || (old.enabled && previous.is_some_and(|config| config.enabled))));
        if !available(&rule.target) && !preserve_unavailable {
            return Err(GatewayError::bad_request(format!("压缩模型「{}」当前不可用，请重新选择或关闭该规则", rule.target)));
        }
    }
    Ok(())
}

pub async fn put_routes(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body).and_then(|value| serde_json::from_value::<SaveRoutes>(value)
        .map_err(|error| GatewayError::bad_request(format!("压缩路由设置不完整：{error}")))) {
        Ok(value) => value,
        Err(error) => return errors::management_error(error.status_code, error.message),
    };
    match tokio::task::spawn_blocking(move || {
        compact_routes::validate(&payload.config).map_err(GatewayError::bad_request)?;
        validate_models(&state, &payload.config)?;
        let enabled = payload.config.enabled;
        let count = payload.config.rules.len();
        compact_routes::save(payload.config, &payload.revision)?;
        logging::log("[Models]", &format!("压缩路由已保存：开关={}，规则={count}", if enabled { "开" } else { "关" }));
        view(&state)
    }).await {
        Ok(Ok(value)) => ok_json(value),
        Ok(Err(error)) => errors::management_error(error.status_code, error.message),
        Err(error) => errors::management_error(500, error.to_string()),
    }
}
