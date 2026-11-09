use serde_json::{json, Value};
use super::{client::CodexClient, config, storage, string, CodingError};

pub fn initial_sandbox(directory: &str, mode: &str) -> Value {
    if mode == "read-only" { json!({ "type": "readOnly", "networkAccess": false }) }
    else { json!({ "type": "workspaceWrite", "writableRoots": [directory], "networkAccess": false,
        "excludeTmpdirEnvVar": true, "excludeSlashTmp": true }) }
}

pub fn saved_sandbox(client: &CodexClient, id: &str) -> Result<Value, CodingError> {
    let sandbox = storage::sandbox(&client.db, &client.scope, id)?
        .unwrap_or_else(|| initial_sandbox("", "read-only"));
    match string(&sandbox, "type") {
        "readOnly" => {}
        "workspaceWrite" => {
            let roots = sandbox.get("writableRoots").and_then(Value::as_array)
                .ok_or_else(|| CodingError::new(409, "会话权限记录不完整，未恢复任务"))?;
            for root in roots {
                client.directory(root.as_str().unwrap_or(""))?;
            }
        }
        _ => return Err(CodingError::new(409, "此会话的权限模式暂不支持，未恢复任务")),
    }
    Ok(sandbox)
}

pub fn store_sandbox(client: &CodexClient, id: &str, sandbox: &Value) -> Result<(), CodingError> {
    storage::save_sandbox(&client.db, &client.scope, id, sandbox)?;
    let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
    let live = state.threads.entry(id.to_string()).or_default();
    live.sandbox = sandbox.clone();
    live.permission_mode = if string(sandbox, "type") == "readOnly" { "read-only" } else { "default" }.into();
    Ok(())
}

pub fn directory(client: &CodexClient, raw: &str) -> Result<String, CodingError> {
    client.directory(raw).map(|path| config::display_path(&path))
}
