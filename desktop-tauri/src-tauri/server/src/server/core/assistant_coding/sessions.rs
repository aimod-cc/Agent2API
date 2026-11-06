use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use super::{client::HubClient, session_view, CodingError, Engine};

pub fn validate_id(id: &str) -> Result<(), CodingError> {
    if id.is_empty() || id.len() > 200 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) || id == "." || id == ".." {
        return Err(CodingError::new(400, "会话或电脑标识无效"));
    }
    Ok(())
}

pub async fn detail(client: &HubClient, id: &str) -> Result<Value, CodingError> {
    validate_id(id)?;
    let data = client.request(Method::GET, &["api", "sessions", id], &[], None).await?;
    let session = data.get("session").ok_or_else(|| CodingError::new(502, "HAPI 会话响应不完整"))?;
    session_view(session).ok_or_else(|| CodingError::new(400, "这里只支持 Codex 和 Claude Code 会话"))
}

pub async fn list(client: &HubClient, engine: Engine) -> Result<Value, CodingError> {
    let data = client.request(Method::GET, &["api", "sessions"], &[("limit".into(), "500".into()), ("order".into(), "updatedAt".into())], None).await?;
    let rows = data.get("sessions").and_then(Value::as_array).ok_or_else(|| CodingError::new(502, "HAPI 会话列表响应不完整"))?;
    let sessions: Vec<Value> = rows.iter().filter_map(session_view)
        .filter(|s| s.get("engine").and_then(Value::as_str) == Some(engine.as_str())).collect();
    Ok(json!({ "sessions": sessions, "limited": rows.len() >= 500 }))
}

pub async fn machines(client: &HubClient) -> Result<Value, CodingError> {
    let data = client.request(Method::GET, &["api", "machines"], &[], None).await?;
    let rows = data.get("machines").and_then(Value::as_array).ok_or_else(|| CodingError::new(502, "HAPI 电脑列表响应不完整"))?;
    let machines: Vec<Value> = rows.iter().filter(|m| m.get("active").and_then(Value::as_bool) == Some(true)).map(|m| json!({
        "id": m.get("id"), "name": m.pointer("/metadata/displayName").or_else(|| m.pointer("/metadata/host")),
        "platform": m.pointer("/metadata/platform"), "homeDirectory": m.pointer("/metadata/homeDir"),
        "workspaceRoots": m.pointer("/metadata/workspaceRoots"),
    })).collect();
    Ok(json!({ "machines": machines }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpawnInput {
    pub machine_id: String,
    pub engine: Engine,
    pub directory: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_mode")]
    pub permission_mode: String,
}
fn default_mode() -> String { "default".into() }

pub async fn spawn(client: &HubClient, input: SpawnInput) -> Result<Value, CodingError> {
    validate_id(&input.machine_id)?;
    let directory = input.directory.trim();
    if directory.is_empty() || directory.len() > 4096 || directory.chars().any(char::is_control) {
        return Err(CodingError::new(400, "请选择有效的项目目录"));
    }
    let modes = match input.engine { Engine::Codex => &["default", "read-only"][..], Engine::Claude => &["default", "plan"][..] };
    if !modes.contains(&input.permission_mode.as_str()) { return Err(CodingError::new(400, "不支持该权限模式")); }
    if input.model.len() > 200 || input.model.chars().any(char::is_control) { return Err(CodingError::new(400, "模型名称无效")); }
    let mut body = json!({ "directory": directory, "agent": input.engine.as_str(), "permissionMode": input.permission_mode,
        "startingMode": "remote", "sessionType": "simple" });
    if !input.model.trim().is_empty() { body["model"] = json!(input.model.trim()); }
    let data = client.request(Method::POST, &["api", "machines", &input.machine_id, "spawn"], &[], Some(&body)).await?;
    let id = data.get("sessionId").and_then(Value::as_str).ok_or_else(|| CodingError::uncertain("HAPI 未返回新会话标识，请刷新列表确认，不要重复创建"))?;
    Ok(json!({ "sessionId": id }))
}

pub async fn action(client: &HubClient, id: &str, action: &str) -> Result<Value, CodingError> {
    let session = detail(client, id).await?;
    if !["resume", "abort", "switch", "archive"].contains(&action) { return Err(CodingError::new(400, "不支持的会话操作")); }
    let action = if action == "resume" && session.get("lifecycle").and_then(Value::as_str) == Some("archived") { "reopen" } else { action };
    let data = client.request(Method::POST, &["api", "sessions", id, action], &[], Some(&json!({}))).await?;
    Ok(json!({ "sessionId": data.get("sessionId").and_then(Value::as_str).unwrap_or(id) }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendInput { pub text: String, pub local_id: String }

pub async fn send(client: &HubClient, id: &str, input: SendInput) -> Result<Value, CodingError> {
    validate_id(&input.local_id)?;
    if input.text.trim().is_empty() || input.text.len() > 256 * 1024 {
        return Err(CodingError::new(400, "消息不能为空且不能超过 256 KB"));
    }
    let session = detail(client, id).await?;
    if session.get("active").and_then(Value::as_bool) != Some(true) {
        return Err(CodingError::new(409, "会话未运行，请先恢复会话"));
    }
    client.request(Method::POST, &["api", "sessions", id, "messages"], &[],
        Some(&json!({ "text": input.text, "localId": input.local_id, "deliveryMode": "queue" }))).await?;
    Ok(json!({ "accepted": true, "localId": input.local_id }))
}

pub async fn permission(client: &HubClient, id: &str, request_id: &str, approve: bool, body: Value) -> Result<Value, CodingError> {
    if request_id.is_empty() || request_id.len() > 1024 || request_id.chars().any(char::is_control) {
        return Err(CodingError::new(400, "审批标识无效"));
    }
    let session = detail(client, id).await?;
    let request = session.get("permissions").and_then(Value::as_array)
        .and_then(|rows| rows.iter().find(|v| v.get("id").and_then(Value::as_str) == Some(request_id)))
        .ok_or_else(|| CodingError::new(409, "这条审批已处理或失效，请刷新会话"))?;
    let answers = body.get("answers");
    let question = matches!(request.get("tool").and_then(Value::as_str), Some("AskUserQuestion" | "ask_user_question" | "request_user_input"));
    if approve && question && answers.and_then(Value::as_object).is_none_or(|v| v.is_empty()) {
        return Err(CodingError::new(400, "请先回答问题"));
    }
    let mut payload = json!({ "decision": if approve { "approved" } else { "denied" } });
    if approve {
        if let Some(answers) = answers {
            if !answers.is_object() || answers.to_string().len() > 64 * 1024 { return Err(CodingError::new(400, "回答格式无效或过长")); }
            payload["answers"] = answers.clone();
        }
    }
    client.request(Method::POST, &["api", "sessions", id, "permissions", request_id, if approve { "approve" } else { "deny" }], &[], Some(&payload)).await?;
    Ok(json!({ "accepted": true }))
}
