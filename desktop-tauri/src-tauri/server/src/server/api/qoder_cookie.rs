//! Qoder 网页 Cookie（积分到期明细）的账号级管理入口。
//!
//! - `GET  /api/qoder-cookie/info?account=<id>` → 插件 `webcookie.info`
//! - `POST /api/qoder-cookie/set` `{accountId, cookie}` → `webcookie.set`
//! - `POST /api/qoder-cookie/clear` `{accountId}` → `webcookie.clear`
//!
//! 为什么是账号级而不是 provider 级：网页 Cookie 属于某个 Qoder 登录账号；
//! 同一家下多个账号各有一枚会话，不能共用。插件负责校验 Cookie 的 `user_id`
//! 与账号一致并加密落库；网关这里只做「账号 → provider → 插件动作」的转发，
//! 不解析 Cookie 内容，也不把 Cookie 写日志。

use axum::extract::{Query, State};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::server::core::providers::adapter;
use crate::server::errors::management_error;
use crate::server::http::ok_json;
use crate::server::ServerState;

#[derive(Debug, Deserialize)]
pub struct AccountQuery {
    pub account: String,
}

#[derive(Debug, Deserialize)]
pub struct SetPayload {
    #[serde(rename = "accountId")]
    pub account_id: String,
    pub cookie: String,
}

#[derive(Debug, Deserialize)]
pub struct ClearPayload {
    #[serde(rename = "accountId")]
    pub account_id: String,
}

async fn call_action(
    state: &ServerState,
    account: &str,
    action: &str,
    payload: &Value,
) -> Result<Value, (i32, String)> {
    let account = account.trim();
    if account.is_empty() {
        return Err((400, "缺少 Qoder 账号 id".to_string()));
    }
    let Some(provider) = state.store().account_provider_of(account) else {
        return Err((404, "账号不存在，请刷新后重试".to_string()));
    };
    if provider != "qoder" && provider != "qoder-intl" {
        return Err((400, "这条账号不是 Qoder，不能设置 Qoder 网页 Cookie".to_string()));
    }
    let Some(adapter) = adapter::adapter_by_id(&provider) else {
        return Err((400, "Qoder 提供商插件没有安装或尚未加载".to_string()));
    };
    adapter
        .admin_action(state.store(), account, action, payload)
        .await
        .map_err(|error| (error.status_code, error.message))
}

/// GET /api/qoder-cookie/info?account=<id>
pub async fn info(State(state): State<ServerState>, Query(query): Query<AccountQuery>) -> Response {
    match call_action(&state, &query.account, "webcookie.info", &Value::Null).await {
        Ok(value) => ok_json(value),
        Err((status_code, message)) => management_error(status_code, &message),
    }
}

/// POST /api/qoder-cookie/set
pub async fn set(State(state): State<ServerState>, Json(payload): Json<SetPayload>) -> Response {
    if payload.cookie.trim().is_empty() {
        return management_error(400, "缺少 Qoder 网页 Cookie");
    }
    match call_action(
        &state,
        &payload.account_id,
        "webcookie.set",
        &serde_json::json!({ "cookie": payload.cookie }),
    )
    .await
    {
        Ok(value) => {
            let packages = value.get("packages").and_then(Value::as_i64).unwrap_or(0);
            crate::server::logging::log(
                "[Qoder]",
                &format!("网页 Cookie 已保存（账号 {}，读到 {} 份积分包）", payload.account_id, packages),
            );
            ok_json(value)
        }
        Err((status_code, message)) => management_error(status_code, &message),
    }
}

/// POST /api/qoder-cookie/clear
pub async fn clear(State(state): State<ServerState>, Json(payload): Json<ClearPayload>) -> Response {
    match call_action(&state, &payload.account_id, "webcookie.clear", &Value::Null).await {
        Ok(value) => ok_json(value),
        Err((status_code, message)) => management_error(status_code, &message),
    }
}
