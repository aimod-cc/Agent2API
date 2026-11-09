use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

use super::{client::CodexClient, now, projection, string};

struct Metadata {
    usage: Option<Value>,
    settings: Value,
    updated_at: Option<i64>,
}

fn breakdown(value: &Value) -> Value {
    let mut result = json!({});
    for (source, target) in [("total_tokens", "totalTokens"), ("input_tokens", "inputTokens"),
        ("cached_input_tokens", "cachedInputTokens"), ("output_tokens", "outputTokens"), ("reasoning_output_tokens", "reasoningOutputTokens")] {
        result[target] = value.get(source).filter(|v| v.as_u64().is_some()).cloned().unwrap_or(json!(0));
    }
    result
}

fn read(home: &Path, path: &Path, id: &str) -> Option<Metadata> {
    let path = std::fs::canonicalize(path).ok()?;
    let allowed = ["sessions", "archived_sessions"].iter().any(|name|
        std::fs::canonicalize(home.join(name)).is_ok_and(|root| path.starts_with(root)));
    if !allowed || !path.file_name()?.to_string_lossy().ends_with(&format!("-{id}.jsonl")) { return None; }
    let mut file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() { return None; }
    const MAX: u64 = 8 * 1024 * 1024;
    let start = metadata.len().saturating_sub(MAX);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut result = Metadata { usage: None, settings: json!({}), updated_at: metadata.modified().ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map(|value| value.as_millis().min(i64::MAX as u128) as i64) };
    for (index, line) in text.split('\n').enumerate() {
        if index == 0 && start > 0 { continue; }
        if !line.contains("\"token_count\"") && !line.contains("\"turn_context\"") && !line.contains("\"thread_settings_applied\"") { continue; }
        let Ok(record) = serde_json::from_str::<Value>(line) else { continue; };
        let payload = record.get("payload").unwrap_or(&record);
        if string(payload, "type") == "token_count" {
            let info = payload.get("info").unwrap_or(&Value::Null);
            if info.get("total_token_usage").is_some_and(Value::is_object) && info.get("last_token_usage").is_some_and(Value::is_object) {
                result.updated_at = record.get("timestamp").and_then(Value::as_str)
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok()).map(|time| time.timestamp_millis());
                result.usage = Some(json!({ "total": breakdown(&info["total_token_usage"]), "last": breakdown(&info["last_token_usage"]),
                    "modelContextWindow": info.get("model_context_window").filter(|v| v.as_u64().is_some_and(|n| n > 0)) }));
            }
        }
        if string(&record, "type") == "turn_context" || string(payload, "type") == "thread_settings_applied" {
            if let Some(model) = payload.get("model").filter(|v| v.is_string()) { result.settings["model"] = model.clone(); }
            if let Some(effort) = payload.get("effort").or_else(|| payload.get("reasoning_effort")) { result.settings["effort"] = effort.clone(); }
            if let Some(tier) = payload.get("service_tier") { result.settings["serviceTier"] = tier.clone(); }
        }
    }
    Some(result)
}

pub async fn refresh(client: &CodexClient, thread: &Value) {
    let id = string(thread, "id").to_string();
    let revision = {
        let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        let revision = state.revision;
        let live = state.threads.entry(id.clone()).or_default();
        if now().saturating_sub(live.metadata_checked_at) < 15_000 { return; }
        if live.attached && live.usage_source.as_deref() == Some("live") && live.model.is_some() { return; }
        live.metadata_checked_at = now();
        revision
    };
    let home = client.config.home();
    let path = PathBuf::from(string(thread, "path"));
    let target = id.clone();
    let Ok(Some(metadata)) = tokio::task::spawn_blocking(move || read(&home, &path, &target)).await else { return; };
    let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
    let Some(live) = state.threads.get_mut(&id) else { return; };
    if live.usage_revision <= revision && (!live.attached || live.usage_source.as_deref() != Some("live")) {
        if let Some(usage) = metadata.usage {
            live.token_usage = usage;
            live.usage_updated_at = metadata.updated_at;
            live.usage_source = Some("history".into());
        }
    }
    if !live.attached && live.settings_revision <= revision { projection::apply_settings(live, &metadata.settings); }
}
