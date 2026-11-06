use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub mod client;
pub mod config;
pub mod sessions;

#[derive(Debug)]
pub struct CodingError {
    pub status: u16,
    pub message: String,
    pub code: &'static str,
}

impl CodingError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), code: "coding_error" }
    }

    pub fn uncertain(message: impl Into<String>) -> Self {
        Self { status: 502, message: message.into(), code: "coding_delivery_uncertain" }
    }
}

impl From<String> for CodingError {
    fn from(message: String) -> Self { Self::new(500, message) }
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine { Codex, Claude }

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self { Self::Codex => "codex", Self::Claude => "claude" }
    }
}

pub fn session_view(session: &Value) -> Option<Value> {
    let metadata = session.get("metadata")?;
    let engine = metadata.get("flavor").and_then(Value::as_str).unwrap_or("claude");
    if !["codex", "claude"].contains(&engine) { return None; }
    let title = metadata.get("name").and_then(Value::as_str).filter(|v| !v.is_empty())
        .or_else(|| metadata.pointer("/summary/text").and_then(Value::as_str))
        .or_else(|| metadata.get("promptTitle").and_then(Value::as_str)).unwrap_or("");
    let requests = session.pointer("/agentState/requests").and_then(Value::as_object);
    let permissions: Vec<Value> = requests.into_iter().flat_map(|items| items.iter()).map(|(id, item)| {
        json!({ "id": id, "tool": item.get("tool"), "arguments": item.get("arguments"),
            "createdAt": item.get("createdAt"), "toolCallId": item.get("toolCallId") })
    }).collect();
    Some(json!({
        "id": session.get("id")?, "engine": engine, "title": title,
        "directory": metadata.get("path"), "machineId": metadata.get("machineId"),
        "active": session.get("active").and_then(Value::as_bool).unwrap_or(false),
        "thinking": session.get("thinking").and_then(Value::as_bool).unwrap_or(false),
        "updatedAt": session.get("updatedAt"), "model": session.get("model"),
        "permissionMode": session.get("permissionMode"),
        "controlledByTerminal": session.pointer("/agentState/controlledByUser").and_then(Value::as_bool).unwrap_or(false),
        "concurrentClients": metadata.pointer("/capabilities/concurrentClients").and_then(Value::as_bool).unwrap_or(false),
        "pendingCount": session.get("pendingRequestsCount").and_then(Value::as_u64).unwrap_or(permissions.len() as u64),
        "permissions": permissions,
        "lifecycle": metadata.get("lifecycleState"),
    }))
}
