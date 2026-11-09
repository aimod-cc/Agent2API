use std::sync::atomic::Ordering;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{client::CodexClient, config, policy, projection, projects, storage, string, CodingError, Engine};

pub fn validate_id(id: &str) -> Result<(), CodingError> {
    if id.is_empty() || id.len() > 200 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) || id == "." || id == ".." {
        return Err(CodingError::new(400, "会话标识无效"));
    }
    Ok(())
}

pub async fn read_thread(client: &CodexClient, id: &str, history: bool) -> Result<Value, CodingError> {
    validate_id(id)?;
    let data = client.rpc("thread/read", json!({ "threadId": id, "includeTurns": history })).await;
    match data {
        Ok(data) => data.get("thread").cloned().ok_or_else(|| CodingError::new(502, "Codex 会话响应不完整")),
        Err(error) => {
            let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(live) = state.threads.get(id).filter(|s| s.attached && !history) { return Ok(live.thread.clone()); }
            Err(error)
        }
    }
}

pub async fn observe_loaded(client: &CodexClient) -> Result<(), CodingError> {
    let mut loaded = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = client.rpc("thread/loaded/list", json!({ "cursor": cursor, "limit": 100 })).await?;
        loaded.extend(page.get("data").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
        cursor = page.get("nextCursor").cloned().unwrap_or(Value::Null);
        if cursor.is_null() || loaded.len() >= 1000 { break; }
    }
    {
        let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        for (id, thread) in &mut state.threads {
            if !loaded.contains(id) { thread.attached = false; thread.turn = None; }
        }
    }
    for id in loaded {
        if client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(&id).is_some_and(|s| s.ending) { continue; }
        let thread = match read_thread(client, &id, false).await {
            Ok(thread) => thread,
            Err(error) if error.code == "codex_rejected" => continue,
            Err(error) => return Err(error),
        };
        if client.directory(string(&thread, "cwd")).is_err() || thread.get("parentThreadId").is_some_and(|p| !p.is_null()) { continue; }
        let data = match client.rpc("thread/resume", json!({ "threadId": id, "excludeTurns": false })).await {
            Ok(data) => data,
            Err(error) if error.code == "codex_rejected" => continue,
            Err(error) => return Err(error),
        };
        if let Some(thread) = data.get("thread") { projection::remember(client, thread, Some(&data), Some(true)); }
    }
    Ok(())
}

pub async fn detail(client: &CodexClient, id: &str) -> Result<Value, CodingError> {
    let thread = read_thread(client, id, false).await?;
    client.directory(string(&thread, "cwd"))?;
    super::metadata::refresh(client, &thread).await;
    let mut snapshot = client.state.snapshot(&thread, &client.projects()?, client.running.load(Ordering::Acquire));
    snapshot["connected"] = json!(client.connected.load(Ordering::Acquire));
    Ok(snapshot)
}

pub async fn list(client: &CodexClient) -> Result<Value, CodingError> {
    let mut threads = Vec::new();
    let mut cursor = Value::Null;
    let mut limited = false;
    let allowed = client.projects()?;
    for _ in 0..5 {
        let data = client.rpc("thread/list", json!({ "limit": 100, "cursor": cursor, "sortKey": "updated_at",
            "modelProviders": [], "sourceKinds": ["cli", "vscode", "exec", "appServer", "unknown"] })).await?;
        let rows = data.get("data").and_then(Value::as_array).ok_or_else(|| CodingError::new(502, "Codex 会话列表响应不完整"))?;
        threads.extend(rows.iter().filter(|t| t.get("parentThreadId").is_none_or(Value::is_null) && projects::directory(&allowed, string(t, "cwd")).is_ok()).cloned());
        cursor = data.get("nextCursor").cloned().unwrap_or(Value::Null);
        limited = !cursor.is_null();
        if !limited { break; }
    }
    {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        for (id, live) in &state.threads {
            if live.attached && !threads.iter().any(|t| string(t, "id") == id) && projects::directory(&allowed, string(&live.thread, "cwd")).is_ok() { threads.push(live.thread.clone()); }
        }
    }
    threads.sort_by_key(|t| std::cmp::Reverse(t.get("updatedAt").and_then(Value::as_i64).unwrap_or(0)));
    let directories = threads.iter().map(|thread| string(thread, "cwd").to_string()).collect::<Vec<_>>();
    let projects = projects::include_history(&client.db, &client.scope, &directories)?;
    let sessions: Vec<Value> = threads.iter().map(|thread| client.state.snapshot(thread, &projects, client.running.load(Ordering::Acquire))).collect();
    Ok(json!({ "sessions": sessions, "projects": projects::public(&projects), "limited": limited }))
}

pub fn machine(_client: &CodexClient) -> Value {
    json!({ "id": "local", "name": std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Agent2API".into()),
        "platform": std::env::consts::OS, "local": true })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpawnInput {
    pub machine_id: String,
    pub engine: Engine,
    pub project_id: String,
    #[serde(default)] pub model: String,
    #[serde(default = "default_mode")] pub permission_mode: String,
}
fn default_mode() -> String { "default".into() }

pub async fn spawn(client: &CodexClient, input: SpawnInput) -> Result<Value, CodingError> {
    let _operation = client.operations.lock().await;
    client.check_current().await?;
    if input.machine_id != "local" || input.engine != Engine::Codex { return Err(CodingError::new(400, "这里只能运行本机 Codex")); }
    let projects = client.projects()?;
    let project = projects.iter().find(|project| project.id == input.project_id).ok_or_else(|| CodingError::new(404, "项目不存在，请先新建或选择一个项目"))?;
    let directory = policy::directory(client, &project.directory)?;
    if !["default", "read-only"].contains(&input.permission_mode.as_str()) { return Err(CodingError::new(400, "不支持的权限模式")); }
    if input.model.len() > 200 || input.model.chars().any(char::is_control) { return Err(CodingError::new(400, "模型名称无效")); }
    let sandbox = policy::initial_sandbox(&directory, &input.permission_mode);
    let mut params = json!({ "cwd": directory, "approvalPolicy": "on-request", "approvalsReviewer": "user",
        "sandbox": if input.permission_mode == "read-only" { "read-only" } else { "workspace-write" }, "ephemeral": false,
        "config": { "sandbox_workspace_write.writable_roots": [directory], "sandbox_workspace_write.network_access": false,
            "sandbox_workspace_write.exclude_tmpdir_env_var": true, "sandbox_workspace_write.exclude_slash_tmp": true } });
    if !input.model.trim().is_empty() { params["model"] = json!(input.model.trim()); }
    let data = client.rpc("thread/start", params).await?;
    let thread = data.get("thread").filter(|t| !string(t, "id").is_empty()).ok_or_else(|| CodingError::uncertain("Codex 未返回会话标识，请刷新列表后确认"))?;
    projection::remember(client, thread, Some(&data), Some(true));
    policy::store_sandbox(client, string(thread, "id"), &sandbox)?;
    Ok(json!({ "sessionId": string(thread, "id") }))
}

async fn resume(client: &CodexClient, id: &str, thread: &Value) -> Result<(), CodingError> {
    {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.threads.get(id).is_some_and(|s| s.attached && !s.ending) { return Ok(()); }
    }
    let directory = policy::directory(client, string(thread, "cwd"))?;
    if thread.pointer("/status/type").and_then(Value::as_str).is_some_and(|s| s != "notLoaded") {
        let data = client.rpc("thread/resume", json!({ "threadId": id })).await?;
        let thread = data.get("thread").ok_or_else(|| CodingError::new(502, "Codex 共享会话响应不完整"))?;
        projection::remember(client, thread, Some(&data), Some(true));
        return Ok(());
    }
    let sandbox = policy::saved_sandbox(client, id)?;
    super::metadata::refresh(client, thread).await;
    let settings = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(id)
        .map(|s| s.settings.clone()).unwrap_or_else(|| json!({}));
    let mut params = json!({ "threadId": id, "cwd": directory, "approvalPolicy": "on-request", "approvalsReviewer": "user",
        "sandbox": if string(&sandbox, "type") == "readOnly" { "read-only" } else { "workspace-write" }, "config": {} });
    if string(&sandbox, "type") == "workspaceWrite" {
        params["config"] = json!({ "sandbox_workspace_write.writable_roots": sandbox.get("writableRoots"),
            "sandbox_workspace_write.network_access": false, "sandbox_workspace_write.exclude_tmpdir_env_var": true,
            "sandbox_workspace_write.exclude_slash_tmp": true });
    }
    for key in ["model", "modelProvider", "serviceTier"] {
        if let Some(value) = settings.get(key) { params[key] = value.clone(); }
    }
    if let Some(effort) = settings.get("effort").filter(|v| v.is_string()) { params["config"]["model_reasoning_effort"] = effort.clone(); }
    let data = client.rpc("thread/resume", params).await?;
    let thread = data.get("thread").ok_or_else(|| CodingError::new(502, "Codex 恢复响应不完整"))?;
    projection::remember(client, thread, Some(&data), Some(true));
    policy::store_sandbox(client, id, &sandbox)
}

async fn interrupt(client: &CodexClient, id: &str, turn: &str) -> Result<(), CodingError> {
    client.rpc("turn/interrupt", json!({ "threadId": id, "turnId": turn })).await?;
    let stopped = async {
        loop {
            if !client.running.load(Ordering::Acquire) { return Ok(()); }
            let finished = {
                let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                state.threads.get(id).is_none_or(|s| s.completed_turns.contains(turn)
                    || s.turn.as_deref() != Some(turn) && s.thread.pointer("/status/type").and_then(Value::as_str) != Some("active"))
            };
            if finished { return Ok(()); }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), stopped).await
        .unwrap_or_else(|_| Err(CodingError::uncertain("停止请求已提交，但尚未确认任务结束；请刷新会话查看状态")))
}

pub async fn action(client: &CodexClient, id: &str, action: &str) -> Result<Value, CodingError> {
    let _operation = client.operations.lock().await;
    client.check_current().await?;
    let thread = read_thread(client, id, false).await?;
    client.directory(string(&thread, "cwd"))?;
    match action {
        "resume" => resume(client, id, &thread).await?,
        "abort" | "archive" => {
            let (attached, turn) = {
                let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                if action == "archive" && state.threads.get(id).is_some_and(|s| s.terminals > 0 || s.terminal_pending > 0) {
                    return Err(CodingError::new(409, "官方终端仍在使用这条会话。可先停止任务，关闭终端后再结束会话"));
                }
                state.threads.get(id).map(|s| (s.attached, s.turn.clone())).unwrap_or((false, None))
            };
            if !attached { return Err(CodingError::new(409, "此会话不由当前后台运行，不能停止其他 Codex 窗口")); }
            if let Some(turn) = turn { interrupt(client, id, &turn).await?; }
            else if client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(id)
                .is_some_and(|s| s.thread.pointer("/status/type").and_then(Value::as_str) == Some("active")) {
                return Err(CodingError::new(409, "当前任务标识尚未同步，请刷新后再停止"));
            }
            if action == "archive" {
                client.rpc("thread/backgroundTerminals/clean", json!({ "threadId": id })).await?;
                client.rpc("thread/unsubscribe", json!({ "threadId": id })).await?;
                let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(live) = state.threads.get_mut(id) {
                    live.attached = false; live.ending = true; live.turn = None;
                    live.thread["status"] = json!({ "type": "idle" });
                }
                state.permissions.retain(|_, p| p.thread_id != id);
            }
            client.state.changed(id);
        }
        _ => return Err(CodingError::new(400, "不支持的 Codex 会话操作")),
    }
    Ok(json!({ "sessionId": id }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendInput {
    pub text: String,
    pub local_id: String,
    #[serde(default)] pub expected_turn_id: Option<String>,
}

pub async fn send(client: &CodexClient, id: &str, input: SendInput) -> Result<Value, CodingError> {
    let _operation = client.operations.lock().await;
    client.check_current().await?;
    validate_id(id)?; validate_id(&input.local_id)?;
    if input.text.trim().is_empty() || input.text.len() > 256 * 1024 { return Err(CodingError::new(400, "消息不能为空且不能超过 256 KB")); }
    let thread = read_thread(client, id, false).await?;
    policy::directory(client, string(&thread, "cwd"))?;
    if let Some(result) = storage::delivery(&client.db, &client.scope, id, &input.local_id, &input.text)? { return Ok(result); }
    resume(client, id, &thread).await?;
    {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        let live = state.threads.get(id).filter(|l| l.attached && !l.ending).ok_or_else(|| CodingError::new(409, "Codex 会话未就绪，请刷新后再发送"))?;
        if input.expected_turn_id.is_none() && live.busy() {
            return Err(CodingError::new(409, "Codex 正在执行任务，请刷新后使用追加指令，或等待任务完成"));
        }
        if input.expected_turn_id.as_deref().is_some_and(|id| live.turn.as_deref() != Some(id)) {
            return Err(CodingError::new(409, "当前任务已变化，未把指令追加给其他任务，请刷新会话"));
        }
    }
    if let Some(result) = storage::reserve(&client.db, &client.scope, id, &input.local_id, &input.text)? { return Ok(result); }
    let mut params = json!({ "threadId": id, "clientUserMessageId": input.local_id,
        "input": [{ "type": "text", "text": input.text, "text_elements": [] }] });
    let method = if let Some(turn) = &input.expected_turn_id { params["expectedTurnId"] = json!(turn); "turn/steer" } else { "turn/start" };
    let result = client.rpc(method, params).await;
    match result {
        Ok(data) => {
            let turn = data.pointer("/turn/id").or_else(|| data.get("turnId")).and_then(Value::as_str).ok_or_else(|| CodingError::uncertain("Codex 未返回任务标识，请检查会话"))?;
            {
                let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(live) = state.threads.get_mut(id) {
                    if !live.completed_turns.contains(turn) { live.turn = Some(turn.into()); }
                }
            }
            storage::settle(&client.db, &client.scope, id, &input.local_id, "accepted", Some(turn))?;
            client.state.changed(id);
            Ok(json!({ "accepted": true, "localId": input.local_id }))
        }
        Err(error) => {
            let status = if error.code == "codex_rejected" { "rejected" } else { "uncertain" };
            let _ = storage::settle(&client.db, &client.scope, id, &input.local_id, status, None);
            Err(error)
        }
    }
}

pub async fn messages(client: &CodexClient, id: &str) -> Result<Value, CodingError> {
    detail(client, id).await?;
    let revision = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).revision;
    let thread = match read_thread(client, id, true).await {
        Ok(thread) => thread,
        Err(error) => {
            let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            state.threads.get(id).filter(|s| s.attached).map(|s| s.thread.clone()).ok_or(error)?
        }
    };
    client.directory(string(&thread, "cwd"))?;
    let mut history = projection::history(client, &thread, revision);
    storage::reconcile(&client.db, &client.scope, id, &mut history)?;
    history["submissions"] = json!(storage::submissions(&client.db, &client.scope, id)?);
    Ok(history)
}

pub async fn status(client: &CodexClient) -> Result<Value, CodingError> {
    let account = client.rpc("account/read", json!({ "refreshToken": false })).await?;
    let ready = account.get("requiresOpenaiAuth") == Some(&Value::Bool(false)) || account.get("account").is_some_and(|a| !a.is_null());
    Ok(json!({ "connected": client.connected.load(Ordering::Acquire), "runtime": "local-codex", "version": client.version, "executable": client.executable.display,
        "shared": true, "codexHome": config::display_path(&client.config.home()), "authenticated": ready,
        "accountType": account.pointer("/account/type"), "machines": [machine(client)] }))
}
