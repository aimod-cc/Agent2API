use serde_json::{json, Map, Value};
use super::{client::CodexClient, now, state::Permission, string, CodingError};

pub async fn register(client: &CodexClient, message: Value) {
    let native_id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = string(&message, "method").to_string();
    let arguments = message.get("params").cloned().unwrap_or(Value::Null);
    let thread_id = string(&arguments, "threadId").to_string();
    let supported = matches!(method.as_str(), "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
        | "item/tool/requestUserInput" | "item/permissions/requestApproval" | "mcpServer/elicitation/request");
    let active = {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.threads.get(&thread_id).is_some_and(|s| s.attached && !s.ending
            && client.directory(string(&s.thread, "cwd")).is_ok())
    };
    if !supported || thread_id.is_empty() || !active {
        return;
    }
    {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.permissions.values().any(|p| p.native_id == native_id && p.thread_id == thread_id) { return; }
    }
    let id = format!("{}-{}-{}", client.generation, client.connection_epoch.load(std::sync::atomic::Ordering::Acquire), crate::server::access::random_hex(8));
    let mut arguments = arguments;
    if method == "item/fileChange/requestApproval" {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        let item_id = string(&arguments, "itemId");
        if let Some(item) = state.threads.get(&thread_id).and_then(|s| s.items.get(&super::projection::item_key(string(&arguments, "turnId"), item_id))) {
            if let Some(changes) = item.get("changes") { arguments["changes"] = changes.clone(); }
        }
    }
    let permission = Permission { id: id.clone(), native_id, thread_id: thread_id.clone(), method,
        arguments, created_at: now(), submitted: false };
    client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).permissions.insert(id, permission);
    client.state.changed(&thread_id);
}

pub async fn answer(client: &CodexClient, thread_id: &str, request_id: &str, approve: bool, body: &Value) -> Result<Value, CodingError> {
    let _operation = client.operations.lock().await;
    client.check_current().await?;
    let permission = {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.permissions.get(request_id).filter(|p| p.thread_id == thread_id && !p.submitted).cloned()
            .ok_or_else(|| CodingError::new(409, "这条审批已处理或已失效，请刷新会话"))?
    };
    let result = match permission.method.as_str() {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => json!({ "decision": if approve { "accept" } else { "decline" } }),
        "item/permissions/requestApproval" => {
            json!({ "permissions": if approve { grant_permissions(client, &permission.arguments)? } else { Map::new() }, "scope": "turn" })
        }
        "item/tool/requestUserInput" => {
            if !approve {
                let turn = string(&permission.arguments, "turnId");
                if turn.is_empty() { return Err(CodingError::new(409, "问题没有可停止的任务标识")); }
                mark_submitted(client, request_id)?;
                let result = client.rpc("turn/interrupt", json!({ "threadId": thread_id, "turnId": turn })).await;
                if result.is_err() { unmark(client, request_id); }
                result?;
                return Ok(json!({ "accepted": true }));
            }
            let answers = validate_answers(&permission.arguments, body)?;
            json!({ "answers": answers })
        }
        "mcpServer/elicitation/request" => {
            if approve { return Err(CodingError::new(422, "此 MCP 表单暂不支持在这里授权，可拒绝后调整工具配置")); }
            json!({ "action": "decline", "content": null, "_meta": null })
        }
        _ => return Err(CodingError::new(422, "不支持的审批类型，未授权")),
    };
    mark_submitted(client, request_id)?;
    let response = client.write(json!({ "id": permission.native_id, "result": result })).await;
    if response.is_err() {
        unmark(client, request_id);
        client.state.changed(thread_id);
        return Err(CodingError::uncertain("审批连接中断，未自动重试；请检查任务状态"));
    }
    client.state.changed(thread_id);
    Ok(json!({ "accepted": true }))
}

fn validate_answers(arguments: &Value, body: &Value) -> Result<Map<String, Value>, CodingError> {
    let questions = arguments.get("questions").and_then(Value::as_array).ok_or_else(|| CodingError::new(400, "问题格式不正确"))?;
    let answers = body.get("answers").and_then(Value::as_object).ok_or_else(|| CodingError::new(400, "请回答问题"))?;
    if body.to_string().len() > 64 * 1024 { return Err(CodingError::new(400, "回答内容过长")); }
    let mut result = Map::new();
    for question in questions {
        let id = string(question, "id");
        let values = answers.get(id).and_then(|v| v.get("answers").unwrap_or(v).as_array())
            .filter(|rows| !rows.is_empty() && rows.iter().all(|v| v.as_str().is_some_and(|s| !s.trim().is_empty())))
            .ok_or_else(|| CodingError::new(400, "请回答所有问题"))?;
        result.insert(id.to_string(), json!({ "answers": values }));
    }
    Ok(result)
}

fn grant_permissions(client: &CodexClient, arguments: &Value) -> Result<Map<String, Value>, CodingError> {
    let invalid = || CodingError::new(422, "此扩展权限无法安全确认，可拒绝后调整任务；未授权");
    let requested = arguments.get("permissions").and_then(Value::as_object).ok_or_else(invalid)?;
    let mut granted = Map::new();
    if let Some(network) = requested.get("network").filter(|v| !v.is_null()) {
        let enabled = network.get("enabled").ok_or_else(invalid)?;
        if !enabled.is_boolean() && !enabled.is_null() { return Err(invalid()); }
        granted.insert("network".into(), json!({ "enabled": enabled }));
    }
    if let Some(files) = requested.get("fileSystem").filter(|v| !v.is_null()) {
        let files = files.as_object().ok_or_else(invalid)?;
        let mut output = Map::new();
        for key in ["read", "write"] {
            let value = files.get(key).unwrap_or(&Value::Null);
            if let Some(paths) = value.as_array() {
                if paths.len() > 128 { return Err(invalid()); }
                for path in paths { validate_permission_path(client, path.as_str().ok_or_else(invalid)?)?; }
            } else if !value.is_null() { return Err(invalid()); }
            output.insert(key.into(), value.clone());
        }
        if let Some(depth) = files.get("globScanMaxDepth") {
            if depth.as_u64().is_none_or(|v| v > 64) { return Err(invalid()); }
            output.insert("globScanMaxDepth".into(), depth.clone());
        }
        if let Some(entries) = files.get("entries") {
            let entries = entries.as_array().filter(|v| v.len() <= 128).ok_or_else(invalid)?;
            let mut approved = Vec::new();
            for entry in entries {
                let access = string(entry, "access");
                let path = entry.get("path").ok_or_else(invalid)?;
                if !["read", "write", "deny"].contains(&access) || string(path, "type") != "path" { return Err(invalid()); }
                validate_permission_path(client, string(path, "path"))?;
                approved.push(json!({ "access": access, "path": { "type": "path", "path": string(path, "path") } }));
            }
            output.insert("entries".into(), json!(approved));
        }
        granted.insert("fileSystem".into(), Value::Object(output));
    }
    if requested.keys().any(|key| !["network", "fileSystem"].contains(&key.as_str())) { return Err(invalid()); }
    Ok(granted)
}

fn validate_permission_path(client: &CodexClient, raw: &str) -> Result<(), CodingError> {
    use std::path::{Component, Path};
    let path = Path::new(raw);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(CodingError::new(422, "扩展权限的路径格式不受支持，未授权"));
    }
    let mut existing = path;
    while !existing.exists() {
        if existing.symlink_metadata().is_ok() { return Err(CodingError::new(422, "扩展权限包含无法解析的链接，未授权")); }
        existing = existing.parent().ok_or_else(|| CodingError::new(422, "扩展权限的路径不存在，未授权"))?;
    }
    let canonical = std::fs::canonicalize(existing).map_err(|_| CodingError::new(403, "无法读取扩展权限的路径"))?;
    let directory = if canonical.is_dir() { canonical.as_path() } else { canonical.parent().ok_or_else(|| CodingError::new(403, "扩展权限的路径无效"))? };
    client.directory(&super::config::display_path(directory))?;
    Ok(())
}

fn mark_submitted(client: &CodexClient, id: &str) -> Result<(), CodingError> {
    let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
    let request = state.permissions.get_mut(id).filter(|p| !p.submitted)
        .ok_or_else(|| CodingError::new(409, "这条审批已处理或已失效"))?;
    request.submitted = true;
    let thread = request.thread_id.clone();
    drop(state);
    client.state.changed(&thread);
    Ok(())
}
fn unmark(client: &CodexClient, id: &str) {
    if let Some(request) = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).permissions.get_mut(id) { request.submitted = false; }
}
