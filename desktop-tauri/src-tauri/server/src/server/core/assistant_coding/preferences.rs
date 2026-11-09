use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{client::CodexClient, now, sessions, string, CodingError};

pub async fn models(client: &CodexClient) -> Result<Value, CodingError> {
    client.check_current().await?;
    let mut cache = client.catalog_cache.lock().await;
    if let Some((_, data)) = cache.as_ref().filter(|(at, _)| now().saturating_sub(*at) < 300_000) { return Ok(data.clone()); }
    let mut models = Vec::new();
    let mut cursor = Value::Null;
    let mut seen = HashSet::new();
    for _ in 0..20 {
        let data = client.rpc("model/list", json!({ "cursor": cursor, "limit": 100, "includeHidden": true })).await?;
        let rows = data.get("data").and_then(Value::as_array).ok_or_else(|| CodingError::new(502, "Codex 模型目录响应不完整"))?;
        for row in rows {
            let id = string(row, "model");
            if id.is_empty() || !seen.insert(id.to_string()) { continue; }
            models.push(json!({ "id": id, "displayName": row.get("displayName"), "description": row.get("description"),
                "hidden": row.get("hidden"), "isDefault": row.get("isDefault"),
                "reasoningEfforts": row.get("supportedReasoningEfforts"), "defaultReasoningEffort": row.get("defaultReasoningEffort"),
                "serviceTiers": row.get("serviceTiers"), "defaultServiceTier": row.get("defaultServiceTier"),
                "supportsFast": row.get("serviceTiers").and_then(Value::as_array).is_some_and(|values| values.iter().any(|v| string(v, "id") == "priority"))
                    || row.get("additionalSpeedTiers").and_then(Value::as_array).is_some_and(|values| values.iter().any(|v| v.as_str() == Some("fast"))) }));
        }
        let next = data.get("nextCursor").cloned().unwrap_or(Value::Null);
        if next.is_null() { break; }
        if next == cursor { return Err(CodingError::new(502, "Codex 模型目录分页无效")); }
        cursor = next;
    }
    let data = json!({ "models": models });
    *cache = Some((now(), data.clone()));
    Ok(data)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsInput {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub fast_mode: Option<bool>,
    pub expected_revision: u64,
}

pub async fn update(client: &CodexClient, id: &str, input: SettingsInput) -> Result<Value, CodingError> {
    let _operation = client.operations.lock().await;
    client.check_current().await?;
    let thread = sessions::read_thread(client, id, false).await?;
    client.directory(string(&thread, "cwd"))?;
    let (model, revision) = {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        let live = state.threads.get(id).filter(|s| s.attached && !s.ending)
            .ok_or_else(|| CodingError::new(409, "请先恢复会话，再调整运行设置"))?;
        if live.settings_revision != input.expected_revision { return Err(CodingError::new(409, "另一端已修改会话设置，请刷新后重新选择")); }
        (live.model.clone().unwrap_or_default(), live.settings_revision)
    };
    let target = input.model.as_deref().unwrap_or(&model).trim();
    if target.is_empty() || target.len() > 200 || target.chars().any(char::is_control) { return Err(CodingError::new(400, "模型名称无效")); }
    let catalog = if input.reasoning_effort.is_some() || input.fast_mode == Some(true) { models(client).await? } else { json!({ "models": [] }) };
    let selected = catalog.get("models").and_then(Value::as_array).and_then(|rows| rows.iter().find(|row| string(row, "id") == target));
    let mut params = json!({ "threadId": id });
    if input.model.is_some() { params["model"] = json!(target); }
    if let Some(effort) = input.reasoning_effort.as_deref() {
        let selected = selected.ok_or_else(|| CodingError::new(422, "Codex 未提供当前模型的思考等级，请在官方终端中设置"))?;
        let effort = if effort == "default" { string(selected, "defaultReasoningEffort") } else { effort };
        if effort.is_empty() || !selected.get("reasoningEfforts").and_then(Value::as_array)
            .is_some_and(|rows| rows.iter().any(|row| string(row, "reasoningEffort") == effort)) {
            return Err(CodingError::new(422, "当前模型不支持所选思考等级"));
        }
        params["effort"] = json!(effort);
    }
    if let Some(enabled) = input.fast_mode {
        if enabled && selected.is_none_or(|row| row.get("supportsFast").and_then(Value::as_bool) != Some(true)) {
            return Err(CodingError::new(422, "当前模型未声明支持快速模式，未开启"));
        }
        params["serviceTier"] = if enabled { json!("priority") } else { Value::Null };
    }
    if params.as_object().is_some_and(|v| v.len() == 1) { return sessions::detail(client, id).await.map(|session| json!({ "session": session })); }
    {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.threads.get(id).is_none_or(|s| s.settings_revision != revision || !s.attached || s.ending) {
            return Err(CodingError::new(409, "会话设置已变化，请刷新后重新选择"));
        }
    }
    client.rpc("thread/settings/update", params.clone()).await?;
    let acknowledged = async {
        loop {
            client.check_current().await?;
            let confirmed = {
                let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                state.threads.get(id).is_some_and(|s| s.settings_revision > revision && matches_settings(&s.settings, &params))
            };
            if confirmed { return Ok::<(), CodingError>(()); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(12), acknowledged).await
        .map_err(|_| CodingError::uncertain("设置已提交，但 Codex 尚未确认所选值；请刷新核对，未显示为已生效"))??;
    let session = client.state.snapshot(&thread, &client.projects()?, client.running.load(Ordering::Acquire));
    Ok(json!({ "session": session }))
}

fn matches_settings(actual: &Value, requested: &Value) -> bool {
    requested.as_object().is_some_and(|fields| fields.iter().all(|(key, value)| {
        if key == "threadId" { return true; }
        if key == "serviceTier" && value.is_null() {
            return actual.get(key).is_none_or(Value::is_null) || actual.get(key).and_then(Value::as_str) == Some("default");
        }
        actual.get(key) == Some(value)
    }))
}
