//! 托管登录的保存事实独立于任务终态；保存入场后异常不能改成确定未写或重新发送。

use std::panic::{catch_unwind, AssertUnwindSafe};

use agent2api_provider_protocol::NormalizedAccount;
use serde_json::{json, Value};

use super::{managed, mark_adapter_login_done, mark_task_error, LoginTaskHandle, LoginTaskState};
use crate::server::{core::providers::adapter::ProviderAdapter, errors::GatewayError};

const UNKNOWN: &str = "账号保存未确认，请查询本次状态；不会再次保存。";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SaveState {
    #[default]
    NotEntered,
    Unconfirmed,
    Confirmed,
}

pub(super) fn wait_response(task: &LoginTaskState) -> Value {
    let saved = match task.persistence {
        SaveState::NotEntered => json!(false),
        SaveState::Unconfirmed => Value::Null,
        SaveState::Confirmed => json!(true),
    };
    let phase = match task.persistence {
        SaveState::Confirmed => "saved",
        SaveState::Unconfirmed if task.done => "unconfirmed",
        SaveState::Unconfirmed => "saving",
        SaveState::NotEntered if task.canceled => "canceled",
        SaveState::NotEntered if task.done => "failed",
        SaveState::NotEntered => "waiting",
    };
    let mut result = json!({
        "managedLogin": true, "provider": task.provider, "flow": task.state,
        "methodIndex": task.method_index, "phase": phase, "saved": saved,
    });
    if !task.done {
        result["pending"] = json!(true);
        if let Some(text) = &task.instructions { result["instructions"] = json!(text); }
        if let Some(text) = &task.hint { result["hint"] = json!(text); }
    } else {
        result["done"] = json!(true);
        if task.persistence == SaveState::Confirmed {
            result["session"] = task.session.clone().unwrap_or(Value::Null);
        } else {
            result["error"] = json!(if task.persistence == SaveState::Unconfirmed {
                UNKNOWN
            } else {
                task.error.as_deref().unwrap_or("托管登录未确认保存账号")
            });
        }
    }
    result
}

pub(super) fn persist(
    handle: &LoginTaskHandle, adapter: &dyn ProviderAdapter, account: &NormalizedAccount,
) -> Result<Option<String>, GatewayError> {
    let mut task = handle.lock();
    if task.canceled { return Ok(None); }
    if task.done {
        return match &task.error {
            Some(error) => Err(GatewayError::bad_request(error)),
            None => Ok(None),
        };
    }
    if handle.adapter().is_some_and(|owner| !std::ptr::eq(owner.as_ref(), adapter)) {
        return Err(GatewayError::with_status(500, "登录流程执行体不匹配，已拒绝保存账号"));
    }
    let proxy = handle.context.as_ref().and_then(|context| context.proxy.as_ref());
    if task.managed && task.persistence != SaveState::NotEntered {
        return Err(GatewayError::with_status(409, UNKNOWN));
    }
    managed::admit_persist(handle, &mut task)?;
    let attempt = catch_unwind(AssertUnwindSafe(|| {
        adapter.persist_login_account_with_proxy(account, proxy.map(|proxy| &proxy.config))
    }));
    let mut deferred = None;
    let outcome = match attempt {
        Ok(Ok(id)) if valid_id(&id) => {
            if task.managed { task.persistence = SaveState::Confirmed; }
            mark_adapter_login_done(&mut task, &id, proxy.is_some());
            Ok(id)
        }
        Ok(Ok(_)) => {
            mark_task_error(&mut task, UNKNOWN);
            Err(GatewayError::with_status(503, UNKNOWN))
        }
        Ok(Err(error)) => {
            if task.managed {
                mark_task_error(&mut task, UNKNOWN);
                Err(GatewayError::with_status(503, UNKNOWN))
            } else {
                mark_task_error(&mut task, &error.message);
                Err(error)
            }
        }
        Err(payload) => {
            deferred = Some(payload);
            mark_task_error(&mut task, UNKNOWN);
            Err(GatewayError::with_status(503, UNKNOWN))
        }
    };
    drop(task);
    // 自定义异常载荷不能在任务、账号或数据库外层锁内析构。
    drop(deferred);
    let id = outcome?;
    if let Some(store) = crate::server::core::account_store::plugin_accounts::shared() {
        crate::server::core::providers::subscriptions::refresh_after_account_saved(store, adapter.provider_id(), &id);
    }
    Ok(Some(id))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && value.trim() == value && !value.chars().any(char::is_control)
}
