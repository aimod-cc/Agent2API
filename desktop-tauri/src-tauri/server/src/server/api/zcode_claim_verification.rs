//! 定时领取的界面验证桥。任务在插件内关联账号和套餐，网关只转发已认证的管理请求。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::providers::{adapter, zcode};
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

pub async fn verification(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) if value.is_object() => value,
        Ok(_) => return management_error(400, "请求体必须是 JSON 对象"),
        Err(error) => return management_error(400, error.message),
    };
    let action = match payload.get("action").and_then(Value::as_str) {
        Some("poll") => "claim.verification.poll",
        Some("complete") => "claim.verification.complete",
        _ => return management_error(400, "领取验证操作无效"),
    };
    let account_id = payload.get("accountId").and_then(Value::as_str).unwrap_or("");
    let selected = if action == "claim.verification.complete" {
        let Some(provider) = state.store().account_provider_of(account_id) else {
            return management_error(404, "领取账号已不存在，验证结果未提交");
        };
        if zcode::region::Region::from_provider_id(&provider).is_none() {
            return management_error(400, "该账号不是 ZCode 账号");
        }
        adapter::adapter_by_id(&provider)
    } else {
        zcode::region::Region::ALL.into_iter()
            .filter(|region| crate::server::core::plugin_host::serves_provider(region.provider_id()))
            .find_map(|region| adapter::adapter_by_id(region.provider_id()))
    };
    let Some(adapter) = selected else {
        return ok_json(json!({ "supported": false, "task": null }));
    };
    match adapter.admin_action(state.store(), account_id, action, &payload).await {
        Ok(value) => ok_json(value),
        // 旧插件不认识新增动作时不反复报错，也不降级成空凭据领取。
        Err(error) if action == "claim.verification.poll"
            && error.message.contains("claim.verification.poll")
            && ["不认识管理动作", "没有实现管理动作", "不支持管理动作", "unknown action", "unsupported action"]
                .iter().any(|marker| error.message.contains(marker)) => {
            ok_json(json!({ "supported": false, "task": null }))
        }
        Err(error) => management_error(error.status_code, error.message),
    }
}
