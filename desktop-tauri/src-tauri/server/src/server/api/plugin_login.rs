use axum::{body::Bytes, extract::{OriginalUri, Query, State}, response::{IntoResponse, Response}};
use serde_json::{json, Value};
use crate::server::{errors::management_error, http::{ok_json, parse_body}, ServerState};

pub async fn callback(State(state): State<ServerState>, OriginalUri(uri): OriginalUri,
    Query(query): Query<std::collections::HashMap<String, String>>) -> Response {
    let flow = query.get("state").map(String::as_str).unwrap_or("");
    let Some(task) = state.login().tasks().get(flow) else { return management_error(404, "登录流程已过期") };
    if !crate::server::core::plugin_host::wasm::has_capability(&task.snapshot().provider, "login.callback.v1") {
        return management_error(403, "此流程未声明本机回调能力");
    }
    let port = crate::server::core::plugin_host::callback_port();
    let callback = format!("http://127.0.0.1:{port}{uri}");
    let result = state.login().submit_login_callback(flow, &callback).await;
    let text = if result.is_ok() { "授权回调已交给插件处理，请返回面板查看登录结果。" }
        else { "授权回调未完成，请返回面板查看原因或重新登录。" };
    let mut response = text.into_response();
    response.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    response.headers_mut().insert("referrer-policy", axum::http::HeaderValue::from_static("no-referrer"));
    response
}

pub async fn get(State(state): State<ServerState>, Query(query): Query<std::collections::HashMap<String, String>>) -> Response {
    let flow = query.get("state").map(String::as_str).unwrap_or("");
    let Some(task) = state.login().tasks().get(flow) else { return management_error(404, "登录任务不存在或已过期") };
    let snapshot = task.snapshot();
    if snapshot.done || snapshot.canceled { return ok_json(json!({ "interaction": null })); }
    if !crate::server::core::plugin_host::wasm::has_capability(&snapshot.provider, "login.interaction.v1") {
        return ok_json(json!({ "interaction": null }));
    }
    let Some(adapter) = task.adapter() else { return management_error(503, "登录执行体已关闭") };
    let _operation = task.operation.lock().await;
    match adapter.admin_action(state.store(), "", "login.interaction", &json!({ "flow": flow })).await {
        Ok(value) => ok_json(json!({ "interaction": value })),
        Err(error) => management_error(error.status_code, error.message),
    }
}

pub async fn submit(State(state): State<ServerState>, body: Bytes) -> Response {
    if body.len() > 256 * 1024 { return management_error(413, "登录交互输入过大") }
    let value = parse_body(&body).unwrap_or(Value::Null);
    let flow = value["state"].as_str().unwrap_or("");
    let Some(task) = state.login().tasks().get(flow) else { return management_error(404, "登录任务不存在或已过期") };
    let _operation = task.operation.lock().await;
    let snapshot = task.snapshot();
    if snapshot.done || snapshot.canceled { return management_error(409, "登录流程已结束") }
    if !crate::server::core::plugin_host::wasm::has_capability(&snapshot.provider, "login.interaction.v1") {
        return management_error(501, "插件未声明交互式登录能力");
    }
    let Some(adapter) = task.adapter() else { return management_error(503, "登录执行体已关闭") };
    let params = json!({ "flow": flow, "interaction": value["interaction"], "inputs": value["inputs"], "captcha": value["captcha"] });
    let result = adapter.admin_action(state.store(), "", "login.submit", &params).await;
    if task.snapshot().canceled || task.snapshot().done { return management_error(409, "登录流程已结束") }
    let value = match result { Ok(value) => value, Err(error) => return management_error(error.status_code, error.message) };
    match serde_json::from_value::<agent2api_provider_protocol::LoginPoll>(value) {
        Ok(agent2api_provider_protocol::LoginPoll::Done { account }) => {
            let saved = crate::server::core::login::persist_plugin_login(&task, adapter.as_ref(), &account);
            task.cancel_provider_flow_locked(flow).await;
            match saved {
                Ok(Some(id)) => ok_json(json!({ "accepted": true, "accountId": id })),
                Ok(None) => management_error(409, "登录流程已结束"),
                Err(error) => management_error(error.status_code, error.message),
            }
        }
        Ok(agent2api_provider_protocol::LoginPoll::Pending { hint }) => {
            crate::server::core::login::set_plugin_login_hint(&task, hint.as_deref());
            ok_json(json!({ "accepted": true }))
        }
        Ok(agent2api_provider_protocol::LoginPoll::Failed { message }) => {
            crate::server::core::login::fail_plugin_login(&task, &message);
            task.cancel_provider_flow_locked(flow).await;
            management_error(400, message)
        }
        Err(_) => management_error(502, "插件返回的登录步骤结果无效"),
    }
}
