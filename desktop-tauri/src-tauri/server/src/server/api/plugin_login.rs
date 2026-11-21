use axum::{body::Bytes, extract::{OriginalUri, Query, State}, http::HeaderMap, response::Response};
use serde_json::{json, Value};
use crate::server::{errors::management_error, http::{ok_json, parse_body}, ServerState};

pub async fn callback(State(state): State<ServerState>, OriginalUri(uri): OriginalUri,
    headers: HeaderMap, Query(query): Query<std::collections::HashMap<String, String>>) -> Response {
    use crate::server::core::login::callback_page;

    let flow = query.get("state").map(String::as_str).unwrap_or("");
    let Some(task) = state.login().tasks().get(flow) else {
        return callback_page::expired_response(None);
    };
    let provider = task.snapshot().provider;
    if !crate::server::core::plugin_host::wasm::has_capability(&provider, "login.callback.v1") {
        return callback_page::error_response(403, Some(&provider), "此流程未声明本机回调能力");
    }
    let callback = match super::session::original_callback_url(&uri, &headers) {
        Ok(url) => url,
        Err(error) => return callback_page::error_response(error.status_code, Some(&provider), &error.message),
    };
    let result = state.login().submit_login_callback(flow, &callback).await;
    callback_page::submission_response(result, Some(&provider))
}

pub async fn get(State(state): State<ServerState>, Query(query): Query<std::collections::HashMap<String, String>>) -> Response {
    let flow = query.get("state").map(String::as_str).unwrap_or("");
    let Some(task) = state.login().tasks().get(flow) else { return management_error(404, "登录任务不存在或已过期") };
    let snapshot = task.snapshot();
    if snapshot.done || snapshot.canceled { return ok_json(json!({ "interaction": null })); }
    if snapshot.managed {
        // 公开快照不等操作锁；换授权地址期间仍能展示 processing 与剩余时间。
        return match crate::server::core::login::managed::snapshot(&task) {
            Ok(value) => ok_json(json!({ "interaction": value })),
            Err(error) => management_error(error.status_code, error.message),
        };
    }
    if snapshot.sms {
        let _operation = task.operation.lock().await;
        if task.snapshot().done || task.snapshot().canceled { return ok_json(json!({ "interaction": null })); }
        return match crate::server::core::login::sms::snapshot(&task) {
            Ok(value) => ok_json(json!({ "interaction": value })),
            Err(error) => management_error(error.status_code, error.message),
        };
    }
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

pub async fn submit(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    if body.len() > 256 * 1024 { return management_error(413, "登录交互输入过大") }
    let value = parse_body(&body).unwrap_or(Value::Null);
    let flow = value["state"].as_str().unwrap_or("");
    let Some(task) = state.login().tasks().get(flow) else { return management_error(404, "登录任务不存在或已过期") };
    if task.snapshot().managed {
        if let Err(error) = crate::server::core::login::managed::check_source(&task, &headers) {
            return management_error(error.status_code, error.message);
        }
        let result = crate::spawn_task(async move {
            let _operation = task.operation.try_lock().map_err(|_|
                crate::server::errors::GatewayError::with_status(409, "当前登录步骤正在处理，请等待完成，不要重复提交"))?;
            crate::server::core::login::managed::submit(&task, value).await
        }).await;
        return match result {
            Ok(Ok(value)) => ok_json(value),
            Ok(Err(error)) => management_error(error.status_code, error.message),
            Err(_) => management_error(503, "托管登录处理已中断，请重新发起；不会自动重放验证码或回调"),
        };
    }
    if task.snapshot().sms {
        let mut value = value.clone();
        if let Some(object) = value.as_object_mut() { object.remove("state"); }
        let result = crate::spawn_task(async move {
            let _operation = task.operation.lock().await;
            let snapshot = task.snapshot();
            if snapshot.done || snapshot.canceled {
                return Err(crate::server::errors::GatewayError::with_status(409, "登录流程已结束"));
            }
            crate::server::core::login::sms::submit(&state, &task, value).await
        }).await;
        return match result {
            Ok(Ok(value)) => ok_json(value),
            Ok(Err(error)) => management_error(error.status_code, error.message),
            Err(_) => management_error(503, "短信登录处理已中断，请重新发起"),
        };
    }
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
