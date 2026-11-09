use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::{config, now, projects::{self, CodingProject}, string};

#[derive(Clone)]
pub struct Permission {
    pub id: String,
    pub native_id: Value,
    pub thread_id: String,
    pub method: String,
    pub arguments: Value,
    pub created_at: i64,
    pub submitted: bool,
}

#[derive(Default)]
pub struct LiveThread {
    pub thread: Value,
    pub activity_revision: u64,
    pub settings_revision: u64,
    pub attached: bool,
    pub turn: Option<String>,
    pub ending: bool,
    pub model: Option<String>,
    pub permission_mode: String,
    pub sandbox: Value,
    pub settings: Value,
    pub token_usage: Value,
    pub usage_revision: u64,
    pub usage_updated_at: Option<i64>,
    pub usage_source: Option<String>,
    pub metadata_checked_at: i64,
    pub terminals: usize,
    pub terminal_pending: usize,
    pub terminal_error: Option<String>,
    pub completed_turns: std::collections::HashSet<String>,
    pub items: BTreeMap<String, Value>,
    pub item_versions: HashMap<String, u64>,
    pub item_order: Vec<String>,
}

impl LiveThread {
    pub fn busy(&self) -> bool {
        self.turn.is_some() || self.thread.pointer("/status/type").and_then(Value::as_str) == Some("active")
    }
}

pub struct RuntimeState {
    pub threads: HashMap<String, LiveThread>,
    pub permissions: HashMap<String, Permission>,
    pub last_error: Option<String>,
    pub revision: u64,
}

impl Default for RuntimeState {
    fn default() -> Self { Self { threads: HashMap::new(), permissions: HashMap::new(), last_error: None, revision: 0 } }
}

pub struct SessionState {
    pub inner: Mutex<RuntimeState>,
    pub events: broadcast::Sender<Value>,
}

impl SessionState {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        Self { inner: Mutex::new(RuntimeState::default()), events }
    }

    pub fn changed(&self, id: &str) {
        let _ = self.events.send(json!({ "type": "session-updated", "sessionId": id }));
    }

    pub fn changed_all(&self) {
        let ids = self.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.keys().cloned().collect::<Vec<_>>();
        for id in ids { self.changed(&id); }
    }

    pub fn snapshot(&self, thread: &Value, projects: &[CodingProject], running: bool) -> Value {
        let id = string(thread, "id");
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let live = state.threads.get(id);
        let mut permissions: Vec<Value> = state.permissions.values().filter(|p| p.thread_id == id).map(|p| json!({
            "id": p.id, "tool": match p.method.as_str() {
                "item/commandExecution/requestApproval" => "commandExecution",
                "item/fileChange/requestApproval" => "fileChange",
                "item/tool/requestUserInput" => "request_user_input",
                "item/permissions/requestApproval" => "permissions",
                _ => p.method.as_str(),
            }, "arguments": p.arguments, "createdAt": p.created_at, "submitted": p.submitted,
            "canApprove": matches!(p.method.as_str(), "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" | "item/tool/requestUserInput" | "item/permissions/requestApproval"),
        })).collect();
        permissions.sort_by_key(|p| p.get("createdAt").and_then(Value::as_i64).unwrap_or(0));
        let thread = live.filter(|l| !string(&l.thread, "cwd").is_empty()
            && l.thread.get("updatedAt").and_then(Value::as_i64).unwrap_or(0) >= thread.get("updatedAt").and_then(Value::as_i64).unwrap_or(0))
            .map(|l| &l.thread).unwrap_or(thread);
        let title = thread.get("name").and_then(Value::as_str).filter(|v| !v.is_empty())
            .unwrap_or_else(|| string(thread, "preview"));
        let active = running && live.is_some_and(|l| l.attached && !l.ending);
        let directory = config::canonical_directory(string(thread, "cwd")).ok();
        let project = directory.as_deref().and_then(|path| projects::find(projects, path));
        json!({ "id": id, "engine": "codex", "title": title.chars().take(160).collect::<String>(),
            "directory": string(thread, "cwd"), "machineId": "local", "active": active,
            "projectId": project.map(|project| &project.id), "projectName": project.map(|project| projects::name(&project.directory)),
            "thinking": running && live.is_some_and(|l| l.turn.is_some() || l.thread.pointer("/status/type").and_then(Value::as_str) == Some("active")),
            "updatedAt": thread.get("updatedAt").and_then(Value::as_i64).unwrap_or(now() / 1000) * 1000,
            "model": live.and_then(|l| l.model.as_ref()), "permissionMode": live.map(|l| l.permission_mode.as_str()).filter(|m| !m.is_empty()),
            "modelProvider": live.and_then(|l| l.settings.get("modelProvider")).or_else(|| thread.get("modelProvider")),
            "reasoningEffort": live.and_then(|l| l.settings.get("effort")),
            "serviceTier": live.and_then(|l| l.settings.get("serviceTier")),
            "settingsRevision": live.map(|l| l.settings_revision).unwrap_or(0),
            "settingsKnown": live.is_some_and(|l| l.settings.get("model").is_some()),
            "tokenUsage": live.map(|l| &l.token_usage), "usageUpdatedAt": live.and_then(|l| l.usage_updated_at),
            "usageSource": live.and_then(|l| l.usage_source.as_ref()), "forkedFromId": thread.get("forkedFromId"),
            "pendingCount": permissions.len(), "permissions": permissions,
            "terminalCount": live.map(|s| s.terminals).unwrap_or(0),
            "terminalOpening": live.is_some_and(|s| s.terminal_pending > 0),
            "terminalError": live.and_then(|s| s.terminal_error.as_ref()),
            "shared": true, "activeTurnId": live.and_then(|s| s.turn.as_ref()),
            "lifecycle": if active { "running" } else { "inactive" },
            "accessible": project.is_some(),
        })
    }
}
