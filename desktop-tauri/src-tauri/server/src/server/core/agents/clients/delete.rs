use serde_json::Value;
use std::{collections::HashSet, path::Path};

use super::{crush, dsh, gemini, remove, string, Plan};
use super::super::{files::{self, Change}, ClientDef, ConfigFormat, Profile};

pub(super) fn plan(def: &ClientDef, path: &Path, profile: &Profile) -> Result<Plan, String> {
    let mut change = Change::load(path.to_path_buf())?;
    let original = files::text(change.before.as_deref())?;
    let mut value = files::parse(def.format, &original)?;
    let id = profile.native.source_id.as_str();
    let mut extra = Vec::new();
    match def.id {
        "claude-code" => {
            for key in ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "ANTHROPIC_MODEL",
                "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                "CLAUDE_CODE_SUBAGENT_MODEL", "CLAUDE_CODE_EFFORT_LEVEL"] {
                remove(&mut value, &["env", key]);
                remove(&mut value, &["env", &format!("{key}_NAME")]);
            }
            remove(&mut value, &["model"]);
            remove(&mut value, &["effortLevel"]);
        }
        "codex" => {
            remove(&mut value, &["model_providers", id]);
            let inherited = string(&value, "model_provider") == id || (id == "openai" && string(&value, "model_provider").is_empty());
            if let Some(profiles) = value.get_mut("profiles").and_then(Value::as_object_mut) {
                for runtime in profiles.values_mut() {
                    let provider = string(runtime, "model_provider");
                    if provider == id || (inherited && provider.is_empty()) { clear_runtime(runtime); }
                }
            }
            if inherited { clear_runtime(&mut value); }
        }
        "opencode" => {
            remove(&mut value, &["provider", id]);
            for key in ["model", "small_model"] {
                if string(&value, key).starts_with(&format!("{id}/")) { remove(&mut value, &[key]); }
            }
            for key in ["enabled_providers", "disabled_providers"] { retain_strings(&mut value, &[key], |value| value != id); }
        }
        "workbuddy" => {
            let ids = profile.models.iter().map(|model| model.id.as_str()).collect::<HashSet<_>>();
            retain_models(&mut value, &ids);
        }
        "zcode" => {
            retain_rules(&mut value, &["config", "providerConfigRules", "providerRules"], id, None);
            retain_strings(&mut value, &["config", "providerOrder"], |value| value != id);
            for key in ["providerModelRules", "manualProviderModelRules"] {
                retain_rules(&mut value, &["config", "modelConfigRules", key], id, None);
            }
        }
        "kimi-code" => {
            remove(&mut value, &["providers", id]);
            let removed = value.get("models").and_then(Value::as_object).map(|models| models.iter()
                .filter(|(_, model)| string(model, "provider") == id).map(|(key, _)| key.clone()).collect::<Vec<_>>()).unwrap_or_default();
            for key in &removed { remove(&mut value, &["models", key]); }
            if removed.contains(&string(&value, "default_model")) { remove(&mut value, &["default_model"]); }
        }
        "zed" => {
            let ids = profile.models.iter().map(|model| model.id.as_str()).collect::<HashSet<_>>();
            let empty = value.pointer_mut("/language_models/openai_compatible").and_then(|providers| providers.get_mut(id))
                .and_then(|provider| provider.get_mut("available_models")).and_then(Value::as_array_mut)
                .is_some_and(|models| { models.retain(|model| !ids.contains(string(model, "name").as_str())
                    || (model.pointer("/capabilities/chat_completions").and_then(Value::as_bool) == Some(false)) != (profile.protocol == "responses")); models.is_empty() });
            if empty { remove(&mut value, &["language_models", "openai_compatible", id]); }
            let selected = value.pointer("/agent/default_model");
            if selected.is_some_and(|model| string(model, "provider") == id && ids.contains(string(model, "model").as_str())
                && !value.pointer("/language_models/openai_compatible").and_then(|providers| providers.get(id))
                    .and_then(|provider| provider.get("available_models")).and_then(Value::as_array)
                    .is_some_and(|models| models.iter().any(|entry| string(entry, "name") == string(model, "model")))) {
                remove(&mut value, &["agent", "default_model"]);
            }
        }
        "crush" => {
            delete_crush(&mut value, id);
            let data = crush::data_path();
            if data != path {
                let mut cached = Change::load(data)?;
                if cached.before.is_some() {
                    let text = files::text(cached.before.as_deref())?;
                    let mut state = files::parse(ConfigFormat::Json, &text)?;
                    delete_crush(&mut state, id);
                    cached.after = Some(files::render(ConfigFormat::Json, &text, &state)?);
                    extra.push(cached);
                }
            }
        }
        "cline" => {
            remove(&mut value, &["providers", id]);
            if string(&value, "lastUsedProvider") == id { remove(&mut value, &["lastUsedProvider"]); }
            let mut models = Change::load(path.with_file_name("models.json"))?;
            if models.before.is_some() {
                let text = files::text(models.before.as_deref())?;
                let mut state = files::parse(ConfigFormat::Json, &text)?;
                remove(&mut state, &["providers", id]);
                models.after = Some(files::render(ConfigFormat::Json, &text, &state)?);
                extra.push(models);
            }
        }
        "dsh" => extra = dsh::delete(path, &mut value, profile)?,
        "gemini-cli" => {
            remove(&mut value, &["model", "name"]);
            if value.pointer("/security/auth/selectedType").and_then(Value::as_str) == Some("gemini-api-key") {
                remove(&mut value, &["security", "auth", "selectedType"]);
            }
            let mut env = Change::load(path.with_file_name(".env"))?;
            if env.before.is_some() {
                let text = files::text(env.before.as_deref())?;
                env.after = Some(gemini::merge_env(&text, &[("GOOGLE_GEMINI_BASE_URL", None), ("GEMINI_API_KEY", None), ("GEMINI_MODEL", None)])?.into_bytes());
                extra.push(env);
            }
        }
        _ => return Err("这个客户端暂不支持删除原生配置".into()),
    }
    change.after = Some(files::render(def.format, &original, &value)?);
    let mut changes = vec![change];
    changes.extend(extra);
    for change in &changes { change.check()?; }
    let mut target = profile.clone();
    target.kind = "native-delete".into();
    Ok(Plan { changes, profile: target, operation: "native-delete".into(), warnings: vec![
        "删除只作用于所选客户端连接及关联模型，保留其它配置、MCP、Skill 和系统登录凭证；写入前自动备份。".into(),
        "删除当前使用的配置后，客户端可能回到内置默认；不会自动选择另一份自定义配置。".into(),
    ] })
}

fn clear_runtime(value: &mut Value) {
    for key in ["model_provider", "model", "model_reasoning_effort", "openai_base_url"] { remove(value, &[key]); }
}

fn delete_crush(value: &mut Value, id: &str) {
    remove(value, &["providers", id]);
    if let Some(models) = value.get_mut("models").and_then(Value::as_object_mut) {
        for model in models.values_mut() {
            if string(model, "provider") == id {
                for key in ["provider", "model", "reasoning_effort"] { remove(model, &[key]); }
            }
        }
    }
}

pub(super) fn retain_models(value: &mut Value, ids: &HashSet<&str>) {
    let models = if value.is_array() { value.as_array_mut() } else { value.get_mut("models").and_then(Value::as_array_mut) };
    if let Some(models) = models { models.retain(|model| !ids.contains(string(model, "id").as_str())); }
    retain_strings(value, &["availableModels"], |value| !ids.contains(value));
}

pub(super) fn retain_strings(value: &mut Value, path: &[&str], keep: impl Fn(&str) -> bool) {
    if let Some(items) = at_mut(value, path).and_then(Value::as_array_mut) {
        items.retain(|value| value.as_str().is_none_or(&keep));
    }
}

pub(super) fn retain_rules(value: &mut Value, path: &[&str], provider: &str, models: Option<&HashSet<&str>>) {
    if let Some(items) = at_mut(value, path).and_then(Value::as_array_mut) {
        items.retain(|rule| string(rule, "providerId") != provider || models.is_some_and(|models| !models.contains(string(rule, "modelId").as_str())));
    }
}

pub(super) fn at_mut<'a>(value: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    let mut target = value;
    for key in path { target = target.get_mut(*key)?; }
    Some(target)
}
