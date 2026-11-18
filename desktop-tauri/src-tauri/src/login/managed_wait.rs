use std::time::Duration;

use serde_json::{json, Value};
use tauri::AppHandle;
use tokio::time::Instant;

use super::{emit_login_progress, login_is_current, result};
use crate::{gateway, panel_session};

const UNKNOWN: &str = "账号保存未确认，请查询本次状态；不会再次保存。";

pub(super) async fn wait(
    app: &AppHandle,
    provider: &str,
    flow: &str,
    started: &Value,
    instructions: &str,
    generation: u64,
) -> Result<Value, String> {
    let result = wait_inner(app, provider, flow, started, instructions, generation).await;
    if (result.is_err()
        || result
            .as_ref()
            .is_ok_and(|value| value["payload"]["saved"].is_null()))
        && panel_session::revision() == generation
        && login_is_current(app, flow)
    {
        let mut progress = started.clone();
        progress["phase"] = json!("unconfirmed");
        progress["saved"] = Value::Null;
        progress["remainingMs"] = json!(0);
        emit_login_progress(app, flow, &progress, instructions, Some(UNKNOWN));
    }
    result
}

async fn wait_inner(
    app: &AppHandle,
    provider: &str,
    flow: &str,
    started: &Value,
    instructions: &str,
    generation: u64,
) -> Result<Value, String> {
    let method_index = started["methodIndex"].as_u64().ok_or(UNKNOWN)?;
    let timeout = Duration::from_millis(
        started["remainingMs"]
            .as_u64()
            .unwrap_or(0)
            .min(15 * 60 * 1000),
    );
    let deadline = Instant::now() + timeout;
    let interval = Duration::from_millis(
        started["pollIntervalMs"]
            .as_u64()
            .unwrap_or(1500)
            .clamp(1000, 30_000),
    );
    let mut progress = started.clone();
    let mut read_only = false;
    loop {
        if panel_session::revision() != generation {
            return Err("管理会话已变化，本次登录资格已失效；不会继续查询或重发原流程。".into());
        }
        if !login_is_current(app, flow) || Instant::now() >= deadline {
            return Err(UNKNOWN.into());
        }
        let path = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("state", flow);
            format!("/api/session/login/wait?{}", query.finish())
        };
        let reply = gateway::call_without_auth_replay("GET", &path, None).await;
        if panel_session::revision() != generation {
            return Err("管理会话已变化，本次登录资格已失效；不会继续查询或重发原流程。".into());
        }
        if !login_is_current(app, flow) {
            return Err(UNKNOWN.into());
        }
        let value = reply.map_err(|_| UNKNOWN.to_string())?;
        match result::wait(&value, provider, flow, method_index)? {
            result::WaitState::Saved => return Ok(json!({ "ok":true, "payload":value })),
            result::WaitState::Failed => return Ok(json!({ "ok":false, "payload":value })),
            result::WaitState::Canceled => {
                return Ok(json!({ "ok":false, "canceled":true, "payload":value }))
            }
            result::WaitState::Unconfirmed => return Ok(json!({ "ok":false, "payload":value })),
            result::WaitState::Saving => {
                read_only = true;
            }
            result::WaitState::Waiting if read_only => return Err(UNKNOWN.into()),
            result::WaitState::Waiting => {}
        }
        progress["phase"] = value["phase"].clone();
        progress["saved"] = value["saved"].clone();
        progress["remainingMs"] = json!(deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64);
        let hint = if read_only {
            Some("正在保存账号，保存结果尚未确认")
        } else {
            value["hint"].as_str()
        };
        emit_login_progress(app, flow, &progress, instructions, hint);
        if Instant::now() >= deadline {
            return Err(UNKNOWN.into());
        }
        tokio::time::sleep_until((Instant::now() + interval).min(deadline)).await;
    }
}
