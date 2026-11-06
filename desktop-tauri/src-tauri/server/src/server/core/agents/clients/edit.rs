use serde_json::Value;
use std::collections::HashSet;

use super::{delete::{at_mut, retain_models, retain_rules, retain_strings}, remove, set, string, Plan};
use super::super::{files::{self, Change}, ClientDef, ConfigFormat, Profile};

pub fn sync_cached_config(def: &ClientDef, path: &std::path::Path, plan: &mut Plan, profile: &Profile) -> Result<(), String> {
    if def.id != "crush" { return Ok(()); }
    let data = super::crush::data_path();
    if data == path { return Ok(()); }
    let mut cached = Change::load(data.clone())?;
    if cached.before.is_none() { return Ok(()); }
    let original = files::text(cached.before.as_deref())?;
    let mut value = files::parse(ConfigFormat::Json, &original)?;
    let id = profile.native.source_id.as_str();
    if value.get("providers").and_then(|providers| providers.get(id)).is_some() {
        super::crush::write(&data, &mut value, profile, "apply")?;
    }
    if profile.enabled {
        let after = plan.changes.iter().find(|change| change.path == data).map(|change| files::text(change.after.as_deref())).transpose()?;
        if let Some(after) = after {
            let state = files::parse(ConfigFormat::Json, &after)?;
            if let Some(model) = state.pointer("/models/large") { set(&mut value, &["models", "large"], model.clone())?; }
        }
    }
    cached.after = Some(files::render(ConfigFormat::Json, &original, &value)?);
    if let Some(change) = plan.changes.iter_mut().find(|change| change.path == data) { *change = cached; }
    else { plan.changes.push(cached); }
    Ok(())
}

pub fn preserve_selection(def: &ClientDef, value: &mut Value, old: &Profile) -> Result<(), String> {
    let id = old.native.source_id.as_str();
    if def.id == "codex" && !old.enabled {
        let selected = string(&old.native.settings, "profile");
        let path = if selected.is_empty() { Vec::new() } else { vec!["profiles", selected.as_str()] };
        let original = old.native.settings.get("runtime").unwrap_or(&Value::Null);
        for key in ["model_provider", "model", "model_reasoning_effort"] {
            let mut field = path.clone(); field.push(key);
            if let Some(item) = original.get(key).filter(|item| !item.is_null()) { set(value, &field, item.clone())?; }
            else { remove(value, &field); }
        }
        if let Some(effort) = old.native.settings.get("rootEffort").filter(|value| !value.is_null()) { set(value, &["model_reasoning_effort"], effort.clone())?; }
        else { remove(value, &["model_reasoning_effort"]); }
    }
    if def.id == "cline" && !old.enabled {
        let selected = old.native.settings.get("lastUsedProvider").unwrap_or(&Value::Null);
        if selected.is_null() { remove(value, &["lastUsedProvider"]); }
        else { set(value, &["lastUsedProvider"], selected.clone())?; }
    }
    if def.id == "cline" {
        let updated = old.native.entry.get("updatedAt");
        if let Some(updated) = updated { set(value, &["providers", id, "updatedAt"], updated.clone())?; }
        else { remove(value, &["providers", id, "updatedAt"]); }
    }
    Ok(())
}

pub fn remove_models(def: &ClientDef, plan: &mut Plan, old: &Profile, wanted: &HashSet<&str>) -> Result<(), String> {
    let removed = old.models.iter().map(|model| model.id.as_str()).filter(|id| !wanted.contains(id)).collect::<HashSet<_>>();
    if removed.is_empty() { return Ok(()); }
    let id = old.native.source_id.as_str();
    for change in &mut plan.changes {
        let original = files::text(change.after.as_deref())?;
        let format = if change.path.extension().is_some_and(|ext| ext == "toml") { super::super::ConfigFormat::Toml } else { def.format };
        let mut value = files::parse(format, &original)?;
        match def.id {
            "opencode" => {
                for model in &removed { remove(&mut value, &["provider", id, "models", model]); }
                if removed.iter().any(|model| string(&value, "small_model") == format!("{id}/{model}")) { remove(&mut value, &["small_model"]); }
            }
            "workbuddy" => retain_models(&mut value, &removed),
            "zcode" => {
                let path = &["config", "providerConfigRules", "providerRules"];
                if let Some(rules) = at_mut(&mut value, path).and_then(Value::as_array_mut) {
                    for rule in rules.iter_mut().filter(|rule| string(rule, "providerId") == id) {
                        for key in ["personalModelIds", "modelOrder"] { retain_strings(rule, &["config", key], |id| !removed.contains(id)); }
                    }
                }
                for key in ["providerModelRules", "manualProviderModelRules"] { retain_rules(&mut value, &["config", "modelConfigRules", key], id, Some(&removed)); }
                if value.pointer("/config/defaultModelSelection/providerId").and_then(Value::as_str) == Some(id)
                    && value.pointer("/config/defaultModelSelection/modelId").and_then(Value::as_str).is_some_and(|model| removed.contains(model))
                {
                    remove(&mut value, &["config", "defaultModelSelection"]);
                    plan.warnings.push("移除的模型是 ZCode 的默认模型，已清除失效选择；请在客户端重新选择默认模型。".into());
                }
            }
            "kimi-code" => {
                let keys = value.get("models").and_then(Value::as_object).map(|models| models.iter()
                    .filter(|(_, model)| string(model, "provider") == id && removed.contains(string(model, "model").as_str()))
                    .map(|(key, _)| key.clone()).collect::<Vec<_>>()).unwrap_or_default();
                for key in keys { remove(&mut value, &["models", &key]); }
            }
            "zed" => {
                if let Some(models) = at_mut(&mut value, &["language_models", "openai_compatible", id, "available_models"]).and_then(Value::as_array_mut) {
                    models.retain(|model| !removed.contains(string(model, "name").as_str())
                        || (model.pointer("/capabilities/chat_completions").and_then(Value::as_bool) == Some(false)) != (old.protocol == "responses"));
                }
            }
            "crush" => {
                if let Some(models) = at_mut(&mut value, &["providers", id, "models"]).and_then(Value::as_array_mut) { models.retain(|model| !removed.contains(string(model, "id").as_str())); }
            }
            "cline" => {
                if change.path.file_name().is_some_and(|file| file == "models.json") {
                    for model in &removed { remove(&mut value, &["providers", id, "models", model]); }
                }
            }
            _ => {}
        }
        change.after = Some(files::render(format, &original, &value)?);
    }
    Ok(())
}
