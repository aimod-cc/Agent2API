use std::time::Duration;

use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::providers::adapter::adapter_by_id;
use crate::server::errors::{management_error, GatewayError};
use crate::server::http::ok_json;
use crate::server::ServerState;

async fn run(state: &ServerState, account: &str, activate: bool) -> Result<Value, GatewayError> {
    match state.store().account_provider_of(account).as_deref() {
        Some("officeace") => {}
        Some(_) => return Err(GatewayError::with_status(400, "该账号不是果办账号")),
        None => return Err(GatewayError::with_status(404, "找不到该账号")),
    }
    let adapter = adapter_by_id("officeace")
        .ok_or_else(|| GatewayError::with_status(503, "果办提供商插件未加载"))?;
    let action = if activate { "subscription.ensure" } else { "subscription.preview" };
    let payload = json!({});
    let value = match tokio::time::timeout(
        Duration::from_secs(45),
        adapter.admin_action(state.store(), account, action, &payload),
    ).await {
        Ok(Ok(value)) => value,
        Ok(Err(error)) if error.status_code == 501 => {
            return Err(GatewayError::with_status(501, "果办套餐接口不可用，请更新主程序和提供商插件"));
        }
        Ok(Err(error)) => return Err(error),
        Err(_) if activate => return Ok(json!({
            "status": "pending", "canActivate": false,
            "message": "开通请求仍在处理或结果待确认，请稍后查询，不要重复提交",
        })),
        Err(_) => return Err(GatewayError::with_status(504, "套餐状态查询超时，请稍后重试")),
    };
    let status = value.get("status").and_then(Value::as_str).unwrap_or("");
    let accepted = if activate {
        matches!(status, "activated" | "already_active" | "skipped" | "pending")
    } else {
        matches!(status, "active" | "inactive" | "expired" | "frozen" | "unknown" | "pending"
            | "credentials_missing" | "credentials_expired" | "permission_denied" | "unsupported")
    };
    if !accepted {
        return Err(GatewayError::with_status(502, "插件未返回有效的套餐状态"));
    }
    // 公开接口只返回状态和说明，不透出插件的原始凭证或订阅响应。
    Ok(json!({
        "status": status,
        "canActivate": !activate && status == "inactive" && value.get("canActivate").and_then(Value::as_bool) == Some(true),
        "message": value.get("message").and_then(Value::as_str).unwrap_or(""),
        "checkedAt": crate::server::logging::now_ms(),
    }))
}

pub async fn preview(state: &ServerState, account: &str) -> Response {
    response(run(state, account, false).await)
}

pub async fn activate(state: &ServerState, account: &str) -> Response {
    response(run(state, account, true).await)
}

fn response(result: Result<Value, GatewayError>) -> Response {
    match result {
        Ok(value) => ok_json(value),
        Err(error) => management_error(error.status_code, error.message),
    }
}
