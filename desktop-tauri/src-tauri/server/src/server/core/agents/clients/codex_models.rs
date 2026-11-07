use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::{model, string};
use super::super::{files::{self, Change}, store, Model, Profile};

pub fn catalog_path(path: &Path, provider: &str) -> PathBuf {
    path.with_file_name(format!("agent2api-codex-{}.json", &store::digest(provider.as_bytes())[..16]))
}

fn load(path: &Path, provider: &str) -> Result<(Change, Value), String> {
    let change = Change::load(catalog_path(path, provider))?;
    let value = match &change.before {
        Some(bytes) => {
            let value: Value = serde_json::from_slice(bytes).map_err(|error| format!("Codex 模型目录损坏：{error}"))?;
            if value.pointer("/_agent2api/provider").and_then(Value::as_str) != Some(provider)
                || value.get("models").and_then(Value::as_array).is_none()
            {
                return Err("Codex 专用模型目录已被其他内容占用，拒绝覆盖".into());
            }
            value
        }
        None => json!({ "_agent2api": { "provider": provider }, "models": [] }),
    };
    Ok((change, value))
}

fn reference_path(path: &Path, name: &str) -> PathBuf {
    if name.starts_with("~/") || name.starts_with("~\\") { return super::super::paths::home().join(&name[2..]); }
    let candidate = PathBuf::from(name);
    if candidate.is_absolute() { candidate } else { path.parent().unwrap_or(Path::new(".")).join(candidate) }
}

pub fn owned_reference(path: &Path, name: &str) -> Result<bool, String> {
    let candidate = reference_path(path, name);
    if !candidate.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("agent2api-codex-") && name.ends_with(".json")) { return Ok(false); }
    let Some(bytes) = files::read(&candidate)? else { return Ok(false); };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { return Ok(false); };
    let Some(provider) = value.pointer("/_agent2api/provider").and_then(Value::as_str) else { return Ok(false); };
    Ok(value.get("models").and_then(Value::as_array).is_some()
        && files::target(&candidate)? == files::target(&catalog_path(path, provider))?)
}

pub fn read(path: &Path, provider: &str, selected_catalog: Option<&str>, active: bool) -> Result<Option<Value>, String> {
    let expected = catalog_path(path, provider);
    if active {
        let Some(name) = selected_catalog else { return Ok(None); };
        let selected = reference_path(path, name);
        if files::target(&selected)? != files::target(&expected)? {
            let Some(bytes) = files::read(&selected)? else { return Err("Codex 当前引用的模型目录不存在，请先在客户端修正路径".into()); };
            let catalog: Value = serde_json::from_slice(&bytes).map_err(|error| format!("Codex 当前模型目录损坏：{error}"))?;
            if catalog.get("models").and_then(Value::as_array).is_none() { return Err("Codex 当前模型目录缺少 models 数组".into()); }
            return Ok(Some(catalog));
        }
    }
    let (change, catalog) = load(path, provider)?;
    Ok(change.before.is_some().then_some(catalog))
}

pub fn models(catalog: &Value) -> Vec<Model> {
    catalog.get("models").and_then(Value::as_array).into_iter().flatten().filter_map(|entry| {
        let id = entry.get("slug").and_then(Value::as_str)?;
        if id.is_empty() { return None; }
        let mut model = model(id, entry);
        model.name = string(entry, "display_name");
        model.context_window = entry.get("context_window").or_else(|| entry.get("max_context_window")).and_then(Value::as_u64);
        model.supports_images = entry.get("input_modalities").and_then(Value::as_array).map(|values| values.iter().any(|value| value == "image"));
        model.supports_tools = entry.get("shell_type").and_then(Value::as_str).map(|value| value != "disabled");
        model.reasoning_levels = entry.get("supported_reasoning_levels").and_then(Value::as_array).into_iter().flatten()
            .filter_map(|level| level.get("effort").and_then(Value::as_str)).map(str::to_string).collect();
        model.supports_reasoning = (!model.reasoning_levels.is_empty()).then(|| model.reasoning_levels.iter().any(|level| level != "none"));
        model.effort = string(entry, "default_reasoning_level");
        Some(model)
    }).collect()
}

pub fn write(path: &Path, provider: &str, profile: &Profile) -> Result<Change, String> {
    let (mut change, mut catalog) = load(path, provider)?;
    let imported = profile.native.model_entries.get("models").and_then(Value::as_array);
    let previous = catalog.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
    let entries = profile.models.iter().enumerate().map(|(index, model)| {
        let old = previous.iter().chain(imported.into_iter().flatten()).find(|entry| entry.get("slug").and_then(Value::as_str) == Some(&model.id));
        entry(model, old, index)
    }).collect::<Vec<_>>();
    catalog["models"] = json!(entries);
    change.after = Some(serde_json::to_vec_pretty(&catalog).map_err(|error| format!("生成 Codex 模型目录失败：{error}"))?);
    Ok(change)
}

fn entry(model: &Model, previous: Option<&Value>, index: usize) -> Value {
    // Codex 目录不是 OpenAI 的 /models 响应；这些字段是客户端解析所需，不代表新增上游能力。
    let mut entry = previous.cloned().unwrap_or_else(|| json!({
        "base_instructions": "You are a coding assistant working with the user in the current workspace.",
        "supports_reasoning_summaries": false, "supports_reasoning_summary_parameter": false,
        "default_reasoning_summary": "none", "support_verbosity": false, "default_verbosity": null,
        "supports_parallel_tool_calls": false, "apply_patch_tool_type": null,
        "truncation_policy": { "mode": "bytes", "limit": 10000 },
        "experimental_supported_tools": [], "supports_search_tool": false,
        "input_modalities": ["text"], "shell_type": "disabled",
        "supported_reasoning_levels": [], "default_reasoning_level": null,
        "effective_context_window_percent": 95
    }));
    entry["slug"] = json!(model.id);
    entry["display_name"] = json!(if model.name.is_empty() { &model.id } else { &model.name });
    entry["description"] = entry["display_name"].clone();
    entry["visibility"] = json!("list");
    entry["supported_in_api"] = json!(true);
    entry["priority"] = json!(index);
    entry["upgrade"] = Value::Null;
    entry["availability_nux"] = Value::Null;
    entry["service_tiers"] = json!([]);
    entry["additional_speed_tiers"] = json!([]);
    entry["default_service_tier"] = Value::Null;
    if let Some(value) = model.context_window {
        entry["context_window"] = json!(value);
        entry["max_context_window"] = json!(value);
    }
    if let Some(value) = model.supports_images { entry["input_modalities"] = if value { json!(["text", "image"]) } else { json!(["text"]) }; }
    if let Some(value) = model.supports_tools { entry["shell_type"] = json!(if value { "shell_command" } else { "disabled" }); }
    if model.supports_reasoning == Some(false) || !model.reasoning_levels.is_empty() {
        let levels = if model.supports_reasoning == Some(false) { Vec::new() } else { model.reasoning_levels.clone() };
        entry["supports_reasoning_summaries"] = json!(levels.iter().any(|level| level != "none"));
        entry["supported_reasoning_levels"] = json!(levels.iter().map(|effort| json!({ "effort": effort, "description": effort })).collect::<Vec<_>>());
        entry["default_reasoning_level"] = if levels.contains(&model.effort) { json!(model.effort) } else { Value::Null };
    }
    entry
}

pub fn remove(path: &Path, provider: &str) -> Result<Option<Change>, String> {
    let (mut change, _) = load(path, provider)?;
    if change.before.is_none() { return Ok(None); }
    change.after = None;
    Ok(Some(change))
}
