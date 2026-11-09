//! 路由页「标题生成」的配置接口。
//!
//! 与压缩路由同一形态：GET 返回当前配置 + 可选模型 + 修订号，PUT 整块保存。
//! 客户端名单由后端给出（`CLIENTS`），因为「能识别哪几家」是判据决定的，
//! 不该让前端各写一份。

use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::config::title_routes::{self, TitleRoutes};
use crate::server::core::providers::catalog;
use crate::server::errors::{self, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

fn available_models(state: &ServerState) -> Vec<Value> {
    catalog::session_models(state.store()).into_iter()
        .filter(|model| model.get("kind").and_then(Value::as_str) != Some("image"))
        .collect()
}

fn view(state: &ServerState) -> Result<Value, GatewayError> {
    let snapshot = title_routes::current();
    let settings = snapshot.settings.as_ref().map_err(|error| GatewayError::new(error.clone()))?;
    Ok(json!({
        "config": settings,
        "revision": snapshot.revision,
        // `supported` 标出「有判据」的客户端。三家都有判据（Codex 靠请求头
        // `x-openai-subagent` / `thread_source`，另两家靠固定提示词），
        // 字段保留是为了以后新增客户端时能如实标出「勾了也不会命中」。
        "clients": title_routes::CLIENTS.iter().map(|(id, label)| json!({
            "id": id, "label": label, "supported": true,
        })).collect::<Vec<_>>(),
        "models": available_models(state),
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
    config: TitleRoutes,
    revision: String,
}

fn validate_model(state: &ServerState, settings: &TitleRoutes) -> Result<(), GatewayError> {
    // 已保存过同一个失效模型时允许原样保存（否则用户连「关掉」都存不下去）。
    let previous = title_routes::current();
    let before: Vec<String> = previous.settings.as_ref().map(|old| old.rules.iter()
        .map(|rule| title_routes::model_key(&rule.model)).collect()).unwrap_or_default();
    for (index, rule) in settings.rules.iter().enumerate() {
        let model = rule.model.trim();
        if model.is_empty() { continue; }
        let available = available_models(state).iter().any(|item|
            item.get("id").and_then(Value::as_str)
                .is_some_and(|id| title_routes::model_key(id) == title_routes::model_key(model)));
        if available || before.contains(&title_routes::model_key(model)) { continue; }
        return Err(GatewayError::bad_request(
            format!("第 {} 条规则的标题模型「{model}」当前不可用，请刷新模型列表后重试", index + 1)))
    }
    Ok(())
}

pub async fn put_routes(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body).and_then(|value| serde_json::from_value::<SaveRoutes>(value)
        .map_err(|error| GatewayError::bad_request(format!("标题生成路由设置不完整：{error}")))) {
        Ok(value) => value,
        Err(error) => return errors::management_error(error.status_code, error.message),
    };
    match tokio::task::spawn_blocking(move || {
        title_routes::validate(&payload.config).map_err(GatewayError::bad_request)?;
        validate_model(&state, &payload.config)?;
        let enabled = payload.config.enabled;
        let count = payload.config.rules.len();
        title_routes::save(payload.config, &payload.revision)?;
        crate::server::logging::log("[Title]", &format!(
            "标题生成路由已保存：开关={}，规则={count} 条", if enabled { "开" } else { "关" }));
        view(&state)
    }).await {
        Ok(Ok(value)) => ok_json(value),
        Ok(Err(error)) => errors::management_error(error.status_code, error.message),
        Err(error) => errors::management_error(500, error.to_string()),
    }
}
