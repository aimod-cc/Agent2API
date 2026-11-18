//! 领取验证的账号管理入口；WASM 仅使用宿主绑定的短通道，旧原生保持受控兼容。
use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::plugin_host::{self, wasm::verification as bridge};
use crate::server::core::providers::{adapter, zcode};
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

fn unsupported() -> Response { ok_json(json!({ "supported": false, "active": false, "task": null })) }

pub async fn verification(State(state): State<ServerState>, body: Bytes) -> Response {
    if body.len() > bridge::MAX_MESSAGE { return management_error(413, "领取验证请求超过大小限制"); }
    let mut payload = match parse_body(&body) {
        Ok(value) if value.is_object() => value,
        Ok(_) => return management_error(400, "请求体必须是 JSON 对象"),
        Err(error) => return management_error(400, error.message),
    };
    let complete = match payload.get("action").and_then(Value::as_str) {
        Some("poll") => false,
        Some("complete") => true,
        _ => return management_error(400, "领取验证操作无效"),
    };
    let account = payload["accountId"].as_str().unwrap_or("").to_string();
    let supplied_provider = payload["provider"].as_str().unwrap_or("").to_string();
    let provider = if account.is_empty() {
        if complete || payload.get("accountId").is_some() || payload.get("provider").is_some() {
            return management_error(400, "领取验证必须明确所属账号与提供商");
        }
        if !bridge::legacy_global_allowed() { return management_error(400, "WASM 领取验证不能使用旧空账号轮询"); }
        let Some(provider) = zcode::region::Region::ALL.into_iter().map(|region| region.provider_id())
            .find(|provider| bridge::legacy_native(provider)) else { return unsupported(); };
        provider.to_string()
    } else if !supplied_provider.is_empty() {
        if zcode::region::Region::from_provider_id(&supplied_provider).is_none() { return management_error(400, "领取验证提供商无效"); }
        supplied_provider
    } else {
        // 只有确实属于旧原生的完整旧回传允许省略 provider；WASM 后续仍强制检查。
        if payload.get("provider").is_some() || payload.get("configId").is_some() {
            return management_error(400, "领取验证包含不完整的账号配置绑定");
        }
        let Some(provider) = state.store().account_provider_of(&account) else { return management_error(404, "领取账号已不存在"); };
        if !bridge::legacy_native(&provider) || zcode::region::Region::from_provider_id(&provider).is_none() {
            return management_error(400, "WASM 领取验证缺少 provider");
        }
        provider
    };
    let wasm = plugin_host::wasm::owns(&provider);
    if !wasm {
        if !bridge::legacy_native(&provider) { return unsupported(); }
        if !account.is_empty() {
            if !complete && payload["provider"].as_str().is_none_or(|provider| provider.is_empty()) {
                return management_error(400, "定向领取轮询必须明确提供商");
            }
            if let Err((status, message)) = super::zcode_captcha::account_provider(&state, &account, Some(&provider)) {
                return management_error(status, message);
            }
        }
        if payload.get("configId").is_some() { return management_error(400, "旧原生领取验证不接受部分新配置绑定"); }
    }
    let Some(adapter) = adapter::adapter_by_id(&provider) else { return unsupported(); };
    if let Some(fields) = payload.as_object_mut() { fields.remove("action"); }
    match adapter.claim_verification_action(state.store(), &account, complete, &payload).await {
        Ok(value) => ok_json(value),
        Err(error) if !complete && error.status_code == 501 => unsupported(),
        Err(error) => management_error(error.status_code, error.message),
    }
}
