//! 托管登录只认宿主真实保存事实，任务结束或外层ok不能代替本次账号提交。

use serde_json::Value;

pub(super) enum WaitState {
    Waiting,
    Saving,
    Saved,
    Failed,
    Canceled,
    Unconfirmed,
}

pub(super) fn wait(
    value: &Value,
    provider: &str,
    flow: &str,
    method_index: u64,
) -> Result<WaitState, String> {
    let unknown = || "账号保存未确认，请查询本次状态；不会再次保存。".to_string();
    if value["managedLogin"] != true
        || value["provider"] != provider
        || value["flow"] != flow
        || value["methodIndex"].as_u64() != Some(method_index)
        || value
            .get("pending")
            .is_some_and(|value| !value.is_boolean())
        || value.get("done").is_some_and(|value| !value.is_boolean())
    {
        return Err(unknown());
    }
    let pending = value["pending"] == true;
    let done = value["done"] == true;
    let state = match value["phase"].as_str() {
        Some("waiting") if value["saved"] == false && pending && !done => WaitState::Waiting,
        Some("saving") if value.get("saved").is_some_and(Value::is_null) && pending && !done => {
            WaitState::Saving
        }
        Some("saved") if value["saved"] == true && done && !pending => {
            managed(value, provider, flow)?;
            WaitState::Saved
        }
        Some("failed") if value["saved"] == false && done && !pending => WaitState::Failed,
        Some("canceled") if value["saved"] == false && done && !pending => WaitState::Canceled,
        Some("unconfirmed")
            if value.get("saved").is_some_and(Value::is_null) && done && !pending =>
        {
            WaitState::Unconfirmed
        }
        _ => return Err(unknown()),
    };
    if matches!(
        state,
        WaitState::Waiting | WaitState::Saving | WaitState::Saved
    ) && value["error"]
        .as_str()
        .is_some_and(|error| !error.is_empty())
    {
        return Err(unknown());
    }
    Ok(state)
}

pub(super) fn managed(value: &Value, provider: &str, flow: &str) -> Result<(), String> {
    if value["managedLogin"] != true
        || value["provider"] != provider
        || value["flow"] != flow
        || value["done"] != true
        || value["pending"] == true
    {
        return Err("托管登录结果不属于本次流程，未确认账号已添加".into());
    }
    match value["saved"].as_bool() {
        None => Err("账号保存未确认，请查询本次状态；不会再次保存。".into()),
        Some(false) => Err("托管登录未确认保存账号，未宣布登录成功".into()),
        Some(true) => {
            if value["phase"] != "saved"
                || value["session"]["provider"] != provider
                || !["accountUid", "accountId"].iter().any(|key| {
                    value["session"][*key].as_str().is_some_and(|id| {
                        !id.is_empty()
                            && id.len() <= 512
                            && id.trim() == id
                            && !id.chars().any(char::is_control)
                    })
                })
            {
                return Err("登录桥返回了空会话，无法确认账号已添加".into());
            }
            Ok(())
        }
    }
}
