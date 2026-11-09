use serde_json::{json, Value};
use super::{client::CodexClient, config, projection, sessions, string, CodingError};

pub struct TerminalSession {
    pub thread_id: String,
    pub initialized: bool,
    pub attached: bool,
}

pub fn serialized(request: &Value) -> bool {
    matches!(string(request, "method"), "thread/start" | "thread/resume" | "thread/fork" | "thread/archive" | "thread/delete"
        | "thread/settings/update" | "turn/start" | "turn/steer" | "turn/interrupt" | "thread/shellCommand")
}

pub async fn before(client: &CodexClient, session: &TerminalSession, request: &mut Value) -> Result<(), CodingError> {
    client.check_current().await?;
    let current_directory = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(&session.thread_id)
        .map(|s| string(&s.thread, "cwd").to_string());
    let method = string(request, "method").to_string();
    if method == "thread/start" && request.pointer("/params/cwd").is_none_or(Value::is_null) {
        if let Some(directory) = current_directory { request["params"]["cwd"] = json!(directory); }
    }
    let method = method.as_str();
    let params = request.get("params").unwrap_or(&Value::Null);
    if method == "initialize" {
        if session.initialized { return Err(CodingError::new(409, "终端连接已初始化")); }
        return Ok(());
    }
    if !session.initialized { return Err(CodingError::new(409, "终端连接尚未初始化")); }
    if method == "initialized" { return Ok(()); }
    if !session.attached && !matches!(method, "thread/resume" | "thread/list" | "thread/read" | "thread/turns/list" | "thread/loaded/list"
        | "model/list" | "modelProvider/capabilities/read" | "config/read" | "configRequirements/read" | "account/read" | "account/rateLimits/read"
        | "skills/list" | "hooks/list" | "experimentalFeature/list" | "collaborationMode/list" | "permissionProfile/list" | "plugin/list"
        | "plugin/installed" | "app/list" | "mcpServerStatus/list" | "windowsSandbox/readiness") {
        return Err(CodingError::new(409, "请先接入指定的共享会话"));
    }
    if !session.attached && method == "thread/resume" && string(params, "threadId") != session.thread_id {
        return Err(CodingError::new(403, "终端只能先连接指定的共享会话"));
    }
    if matches!(method, "thread/rollback" | "thread/revert") {
        return Err(CodingError::new(409, "共享会话不能原地回退，请使用 /fork 创建分支"));
    }
    if matches!(method, "thread/archive" | "thread/delete") {
        let peers = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(&session.thread_id)
            .map(|s| s.terminals + s.terminal_pending).unwrap_or(0);
        if peers > 1 { return Err(CodingError::new(409, "其他官方终端仍在使用这条会话，请先关闭其他终端")); }
    }
    if method == "thread/resume" && (params.get("history").is_some_and(|v| !v.is_null()) || params.get("path").is_some_and(|v| !v.is_null())) {
        return Err(CodingError::new(400, "共享会话只能按会话标识恢复"));
    }
    if matches!(method, "thread/start" | "thread/fork" | "thread/resume") {
        let existing = if method != "thread/start" {
            let id = string(params, "threadId");
            let thread = sessions::read_thread(client, id, false).await?;
            client.directory(string(&thread, "cwd"))?;
            if !thread.get("parentThreadId").is_none_or(Value::is_null) { return Err(CodingError::new(409, "请在原会话中查看子任务，不能单独接管子任务")); }
            Some(thread)
        } else { None };
        let directory = params.get("cwd").and_then(Value::as_str)
            .or_else(|| existing.as_ref().map(|thread| string(thread, "cwd"))).unwrap_or("");
        client.directory(directory)?;
    } else if let Some(id) = params.get("threadId").and_then(Value::as_str) {
        if id != session.thread_id { return Err(CodingError::new(403, "终端请求不属于当前共享会话")); }
    }
    if let Some(cwd) = params.get("cwd").and_then(Value::as_str) { client.directory(cwd)?; }
    if method == "thread/resume" {
        let id = string(params, "threadId").to_string();
        let loaded = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(&id).is_some_and(|s| s.attached);
        if loaded {
            if let Some(params) = request.get_mut("params").and_then(Value::as_object_mut) {
                for key in ["approvalPolicy", "approvalsReviewer", "sandbox", "permissions", "runtimeWorkspaceRoots", "config", "model", "modelProvider", "serviceTier", "cwd"] { params.remove(key); }
            }
        }
    }
    Ok(())
}

pub async fn after(client: &CodexClient, session: &mut TerminalSession, request: &Value, response: &Value) -> Result<Vec<Value>, CodingError> {
    if response.get("error").is_some() { return Ok(Vec::new()); }
    let method = string(request, "method");
    if method == "initialize" { session.initialized = true; return Ok(Vec::new()); }
    if method == "turn/start" || method == "turn/steer" {
        let turn_id = response.pointer("/result/turn/id").or_else(|| response.pointer("/result/turnId")).and_then(Value::as_str);
        if let Some(turn_id) = turn_id {
            let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(live) = state.threads.get_mut(&session.thread_id) {
                if live.turn.is_none() && !live.completed_turns.contains(turn_id) {
                    live.turn = Some(turn_id.into());
                    live.thread["status"] = json!({ "type": "active", "activeFlags": [] });
                }
            }
            drop(state);
            client.state.changed(&session.thread_id);
        }
    }
    if matches!(method, "thread/start" | "thread/resume" | "thread/fork") {
        let result = response.get("result").unwrap_or(&Value::Null);
        let thread = result.get("thread").ok_or_else(|| CodingError::uncertain("Codex 未返回终端会话标识，请在列表中核对"))?;
        let id = string(thread, "id");
        sessions::validate_id(id)?;
        client.directory(string(thread, "cwd"))?;
        let known = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(id).is_some_and(|s| s.attached);
        if !known { projection::remember(client, thread, Some(result), Some(true)); }
        let observed = client.rpc("thread/resume", json!({ "threadId": id })).await?;
        if let Some(thread) = observed.get("thread") { projection::remember(client, thread, Some(&observed), Some(true)); }
        move_session(client, session, id);
        let settings = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(id).map(|s| s.settings.clone()).unwrap_or(Value::Null);
        if settings.get("collaborationMode").is_some() {
            return Ok(vec![json!({ "method": "thread/settings/updated", "params": { "threadId": id, "threadSettings": settings } })]);
        }
    }
    Ok(Vec::new())
}

fn move_session(client: &CodexClient, session: &mut TerminalSession, id: &str) {
    if session.attached && session.thread_id == id { return; }
    let old = session.thread_id.clone();
    {
        let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        if session.attached {
            if let Some(thread) = state.threads.get_mut(&old) { thread.terminals = thread.terminals.saturating_sub(1); }
        }
        let thread = state.threads.entry(id.to_string()).or_default();
        thread.terminals += 1; thread.terminal_error = None;
    }
    session.thread_id = id.to_string(); session.attached = true;
    client.state.changed(&old); client.state.changed(id);
}

pub fn detached(client: &CodexClient, session: &TerminalSession) {
    if session.attached {
        if let Some(thread) = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get_mut(&session.thread_id) {
            thread.terminals = thread.terminals.saturating_sub(1);
        }
        client.state.changed(&session.thread_id);
    }
}

pub fn filter(client: &CodexClient, mut message: Value) -> Value {
    if let Some(rows) = message.pointer_mut("/result/data").and_then(Value::as_array_mut) {
        if rows.first().is_some_and(|row| row.get("cwd").is_some() && row.get("id").is_some()) {
            rows.retain(|thread| client.directory(string(thread, "cwd")).is_ok());
        }
    }
    message
}

pub fn display_directory(client: &CodexClient, id: &str) -> Result<String, CodingError> {
    let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
    let thread = state.threads.get(id).ok_or_else(|| CodingError::new(404, "共享会话不存在"))?;
    client.directory(string(&thread.thread, "cwd")).map(|p| config::display_path(&p))
}
