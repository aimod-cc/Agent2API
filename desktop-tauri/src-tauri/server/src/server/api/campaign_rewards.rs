//! 活动福利的查询与单项领取；上游资格与协议由提供商插件处理，不记每日签到。

use axum::{body::Bytes, response::Response};
use serde_json::Value;

use crate::server::core::{plugin_host, providers::adapter::adapter_by_id};
use crate::server::errors::{management_error, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

fn provider(state: &ServerState, account: &str) -> Result<String, GatewayError> {
    let provider = state.store().account_provider_of(account)
        .ok_or_else(|| GatewayError::with_status(404, "找不到该账号"))?;
    if !plugin_host::has_campaign_rewards(&provider) {
        return Err(GatewayError::with_status(400, "该账号的提供商插件未提供活动福利，请确认插件已更新并启用"));
    }
    Ok(provider)
}

pub async fn preview(state: &ServerState, account: &str) -> Response {
    let provider = match provider(state, account) {
        Ok(provider) => provider,
        Err(error) => return management_error(error.status_code, error.message),
    };
    let Some(adapter) = adapter_by_id(&provider) else {
        return management_error(503, "提供商插件尚未就绪");
    };
    match adapter.claim_status(state.store(), account, true).await {
        Ok(value) => ok_json(value),
        Err(error) => management_error(error.status_code, error.message),
    }
}

pub async fn redeem(state: &ServerState, account: &str, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => return management_error(error.status_code, error.message),
    };
    let code = payload.get("code").and_then(Value::as_str).unwrap_or("").trim();
    if code.is_empty() || code.len() > 128 || code.chars().any(char::is_control) {
        return management_error(400, "请填写有效的权益兑换码或订单号");
    }
    let provider = match provider(state, account) {
        Ok(provider) => provider,
        Err(error) => return management_error(error.status_code, error.message),
    };
    let Some(adapter) = adapter_by_id(&provider) else {
        return management_error(503, "提供商插件尚未就绪");
    };
    match adapter.admin_action(state.store(), account, "benefits.redeem", &serde_json::json!({ "code": code })).await {
        Ok(value) => ok_json(value),
        Err(error) => management_error(error.status_code, error.message),
    }
}

pub async fn claim(state: &ServerState, account: &str, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => return management_error(error.status_code, error.message),
    };
    let id = payload.get("campaignId").and_then(Value::as_str).unwrap_or("").trim();
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return management_error(400, "请选择要领取的活动");
    }
    let provider = match provider(state, account) {
        Ok(provider) => provider,
        Err(error) => return management_error(error.status_code, error.message),
    };
    let Some(adapter) = adapter_by_id(&provider) else {
        return management_error(503, "提供商插件尚未就绪");
    };
    match adapter.claim(state.store(), account, id).await {
        Ok(value) => ok_json(value),
        Err(error) => management_error(error.status_code, error.message),
    }
}
