use serde::Deserialize;
use serde_json::{json, Value};

use super::{client::CodexClient, policy, projection, sessions, string, CodingError};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForkInput {
    pub last_turn_id: String,
}

pub async fn create(client: &CodexClient, id: &str, input: ForkInput) -> Result<Value, CodingError> {
    let _operation = client.operations.lock().await;
    client.check_current().await?;
    sessions::validate_id(&input.last_turn_id)?;
    let thread = sessions::read_thread(client, id, true).await?;
    let directory = policy::directory(client, string(&thread, "cwd"))?;
    let turns = thread.get("turns").and_then(Value::as_array).ok_or_else(|| CodingError::new(409, "会话历史尚未就绪，未创建分支"))?;
    let index = turns.iter().position(|turn| string(turn, "id") == input.last_turn_id)
        .ok_or_else(|| CodingError::new(409, "分支对应的对话已变化，请刷新后重新选择"))?;
    if turns[..=index].iter().any(|turn| !matches!(string(turn, "status"), "completed" | "interrupted" | "failed")) {
        return Err(CodingError::new(409, "只能在已经结束的轮次后创建分支，当前任务不会被打断"));
    }
    super::metadata::refresh(client, &thread).await;
    let (settings, live_sandbox) = {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.threads.get(id).map(|s| (s.settings.clone(), s.attached.then(|| s.sandbox.clone())))
            .unwrap_or_else(|| (json!({}), None))
    };
    let sandbox = match live_sandbox { Some(sandbox) => sandbox, None => policy::saved_sandbox(client, id)? };
    match string(&sandbox, "type") {
        "readOnly" => {}
        "workspaceWrite" => {
            let roots = sandbox.get("writableRoots").and_then(Value::as_array)
                .ok_or_else(|| CodingError::new(409, "会话权限不完整，未创建分支"))?;
            for root in roots { client.directory(root.as_str().unwrap_or(""))?; }
        }
        _ => return Err(CodingError::new(409, "当前会话的权限模式无法在这里安全继承，请在官方终端创建分支")),
    }
    let mut params = json!({ "threadId": id, "lastTurnId": input.last_turn_id, "cwd": directory,
        "approvalPolicy": "on-request", "approvalsReviewer": "user", "ephemeral": false,
        "sandbox": if string(&sandbox, "type") == "readOnly" { "read-only" } else { "workspace-write" },
        "config": { "sandbox_workspace_write.network_access": false, "sandbox_workspace_write.exclude_tmpdir_env_var": true,
            "sandbox_workspace_write.exclude_slash_tmp": true } });
    if string(&sandbox, "type") == "workspaceWrite" {
        params["config"]["sandbox_workspace_write.writable_roots"] = sandbox.get("writableRoots").cloned().unwrap_or(json!([directory]));
    }
    if let Some(profile) = settings.get("activePermissionProfile").filter(|v| !v.is_null()) {
        let profile_id = string(profile, "id");
        if !matches!(profile_id, ":workspace" | ":read-only") {
            return Err(CodingError::new(409, "当前会话使用特殊权限配置，请在官方终端创建分支，避免改变权限边界"));
        }
    }
    for key in ["model", "modelProvider", "serviceTier"] {
        if let Some(value) = settings.get(key) { params[key] = value.clone(); }
    }
    if let Some(effort) = settings.get("effort").filter(|v| v.is_string()) { params["config"]["model_reasoning_effort"] = effort.clone(); }
    let data = client.rpc("thread/fork", params).await?;
    let fork = data.get("thread").filter(|v| !string(v, "id").is_empty())
        .ok_or_else(|| CodingError::uncertain("Codex 未返回分支标识，请刷新会话列表确认，不要重复创建"))?;
    let fork_id = string(fork, "id");
    client.directory(string(fork, "cwd"))?;
    projection::remember(client, fork, Some(&data), Some(true));
    if policy::store_sandbox(client, fork_id, &sandbox).is_err() {
        return Ok(json!({ "sessionId": fork_id, "warning": "分支已创建，但权限记录保存失败；下次恢复将保持只读，请先核对设置" }));
    }
    Ok(json!({ "sessionId": fork_id }))
}
