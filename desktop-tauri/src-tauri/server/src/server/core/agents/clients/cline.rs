use super::super::{
    files::{self, Change},
    ConfigFormat, Model, Profile,
};
use super::{native_entry, remove, set, string};
use serde_json::{json, Value};
use std::path::Path;

pub fn read(path: &Path, value: &Value) -> Result<Vec<Profile>, String> {
    if value
        .get("version")
        .and_then(Value::as_u64)
        .is_some_and(|v| v != 1)
    {
        return Err("Cline providers.json 版本暂不支持".into());
    }
    let Some(providers) = value.get("providers").and_then(Value::as_object) else {
        if value.as_object().is_some_and(|v| v.is_empty()) {
            return Ok(Vec::new());
        }
        return Err("Cline 缺少原生 providers 对象".into());
    };
    let model_file = path.with_file_name("models.json");
    let models = files::read(&model_file)?
        .map(|bytes| files::text(Some(&bytes)))
        .transpose()?
        .map(|text| files::parse(ConfigFormat::Json, &text))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    let active = string(value, "lastUsedProvider");
    let mut out = Vec::new();
    for (id, entry) in providers {
        let Some(settings) = entry.get("settings") else {
            continue;
        };
        let base = string(settings, "baseUrl");
        let protocol = if id == "openai-compatible" {
            match settings.get("protocol").and_then(Value::as_str) {
                Some("openai-responses") => "responses",
                Some("anthropic") => "anthropic",
                _ => "openai_chat",
            }
        } else {
            "native"
        };
        let mut profile = Profile::new(id, id, protocol, &base);
        profile.model = string(settings, "model");
        profile.api_key = settings
            .pointer("/auth/accessToken")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| string(settings, "apiKey"));
        if profile.api_key.is_empty() {
            profile.api_key = settings
                .pointer("/auth/apiKey")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into();
        }
        profile.effort = settings
            .pointer("/reasoning/effort")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into();
        profile.enabled = id == &active;
        profile.default = profile.enabled;
        profile.native.entry = entry.clone();
        profile.native.settings = json!({ "lastUsedProvider": value.get("lastUsedProvider") });
        profile.native.model_entries = models
            .get("providers")
            .and_then(|v| v.get(id))
            .cloned()
            .unwrap_or(Value::Null);
        if let Some(entries) = profile
            .native
            .model_entries
            .get("models")
            .and_then(Value::as_object)
        {
            for (id, entry) in entries {
                let capabilities = entry.get("capabilities").and_then(Value::as_array);
                profile.models.push(Model {
                    id: id.into(),
                    name: string(entry, "name"),
                    context_window: entry.get("contextWindow").and_then(Value::as_u64),
                    max_output_tokens: entry.get("maxTokens").and_then(Value::as_u64),
                    supports_tools: capabilities.map(|v| v.iter().any(|v| v == "tools")),
                    supports_images: capabilities.map(|v| v.iter().any(|v| v == "images")),
                    supports_reasoning: capabilities.map(|v| v.iter().any(|v| v == "reasoning")),
                    reasoning_levels: entry
                        .get("reasoningOptions")
                        .and_then(Value::as_array)
                        .map(|options| {
                            options
                                .iter()
                                .filter(|option| {
                                    option.get("type").and_then(Value::as_str) == Some("effort")
                                })
                                .filter_map(|option| option.get("values").and_then(Value::as_array))
                                .flatten()
                                .filter_map(Value::as_str)
                                .filter(|level| *level != "default")
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                    ..Default::default()
                });
            }
        }
        if !profile.model.is_empty()
            && !profile.models.iter().any(|model| model.id == profile.model)
        {
            profile.models.push(Model {
                id: profile.model.clone(),
                name: profile.model.clone(),
                ..Default::default()
            });
        }
        out.push(profile);
    }
    Ok(out)
}

pub fn write(path: &Path, value: &mut Value, profile: &Profile) -> Result<Vec<Change>, String> {
    if value
        .get("version")
        .and_then(Value::as_u64)
        .is_some_and(|v| v != 1)
    {
        return Err("Cline 配置版本暂不支持".into());
    }
    if value.get("providers").is_none() && !value.as_object().is_some_and(|v| v.is_empty()) {
        return Err("Cline 配置缺少原生 providers".into());
    }
    let id = "openai-compatible";
    if !profile.native.source_id.is_empty() && profile.native.source_id != id {
        return Err(
            "仅适配 Cline CLI 的 OpenAI Compatible 提供商；其他认证请在客户端中管理".into(),
        );
    }
    let imported = profile.native.entry.clone();
    let old_entry = value.get("providers").and_then(|v| v.get(id));
    let changed_model = old_entry.and_then(|entry| entry.pointer("/settings/model")).and_then(Value::as_str) != Some(&profile.model)
        || old_entry.and_then(|entry| entry.pointer("/settings/baseUrl")).and_then(Value::as_str) != Some(&profile.base_url);
    let mut entry = native_entry(old_entry, &imported)?;
    if changed_model {
        for key in ["contextWindow", "maxTokens"] { remove(&mut entry, &["settings", key]); }
    }
    set(&mut entry, &["settings", "provider"], json!(id))?;
    set(&mut entry, &["settings", "protocol"], json!("openai-chat"))?;
    set(
        &mut entry,
        &["settings", "client"],
        json!("openai-compatible"),
    )?;
    remove(&mut entry, &["settings", "routingProviderId"]);
    set(&mut entry, &["settings", "model"], json!(profile.model))?;
    set(
        &mut entry,
        &["settings", "baseUrl"],
        json!(profile.base_url),
    )?;
    super::apply_credential(&mut entry, &["settings", "apiKey"], profile)?;
    if profile.auth_mode != "preserve" {
        remove(&mut entry, &["settings", "auth"]);
    }
    if profile.effort.is_empty() || profile.effort == "default" {
        remove(&mut entry, &["settings", "reasoning", "effort"]);
        remove(&mut entry, &["settings", "reasoning", "enabled"]);
    } else {
        set(
            &mut entry,
            &["settings", "reasoning", "effort"],
            json!(profile.effort),
        )?;
        set(
            &mut entry,
            &["settings", "reasoning", "enabled"],
            json!(profile.effort != "none"),
        )?;
    }
    set(
        &mut entry,
        &["updatedAt"],
        json!(chrono::DateTime::from_timestamp_millis(profile.updated_at)
            .map(|time| time.to_rfc3339())
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())),
    )?;
    if profile.auth_mode != "preserve" {
        set(&mut entry, &["tokenSource"], json!("manual"))?;
    }
    set(value, &["version"], json!(1))?;
    if value.get("modes").is_none() {
        set(value, &["modes"], json!({}))?;
    }
    set(value, &["providers", id], entry)?;
    set(value, &["lastUsedProvider"], json!(id))?;
    let mut change = Change::load(path.with_file_name("models.json"))?;
    let original = files::text(change.before.as_deref())?;
    let mut models = files::parse(ConfigFormat::Json, &original)?;
    if models
        .get("version")
        .and_then(Value::as_u64)
        .is_some_and(|version| version != 1)
    {
        return Err("Cline 模型清单版本暂不支持，已停止写入".into());
    }
    let mut provider = native_entry(
        models.get("providers").and_then(|v| v.get(id)),
        &profile.native.model_entries,
    )?;
    set(&mut provider, &["provider", "id"], json!(id))?;
    set(&mut provider, &["provider", "name"], json!(profile.name))?;
    set(
        &mut provider,
        &["provider", "protocol"],
        json!("openai-chat"),
    )?;
    set(
        &mut provider,
        &["provider", "client"],
        json!("openai-compatible"),
    )?;
    set(&mut provider, &["provider", "source"], json!("file"))?;
    set(
        &mut provider,
        &["provider", "baseUrl"],
        json!(profile.base_url),
    )?;
    set(
        &mut provider,
        &["provider", "defaultModelId"],
        json!(profile.model),
    )?;
    for model in &profile.models {
        let mut item = provider
            .get("models")
            .and_then(|v| v.get(&model.id))
            .cloned()
            .unwrap_or_else(|| json!({}));
        set(&mut item, &["id"], json!(model.id))?;
        set(
            &mut item,
            &["name"],
            json!(if model.name.is_empty() {
                &model.id
            } else {
                &model.name
            }),
        )?;
        if model.supports_tools.is_some()
            || model.supports_images.is_some()
            || model.supports_reasoning.is_some()
        {
            let mut caps = item
                .get("capabilities")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![json!("streaming")]);
            for (name, supported) in [
                ("tools", model.supports_tools),
                ("images", model.supports_images),
                ("reasoning", model.supports_reasoning),
            ] {
                if let Some(supported) = supported {
                    caps.retain(|value| value.as_str() != Some(name));
                    if supported {
                        caps.push(json!(name));
                    }
                }
            }
            set(&mut item, &["capabilities"], json!(caps))?;
        }
        if model.supports_reasoning == Some(false) {
            remove(&mut item, &["reasoningOptions"]);
        } else if !model.reasoning_levels.is_empty() {
            let mut options = item
                .get("reasoningOptions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            options.retain(|option| option.get("type").and_then(Value::as_str) != Some("effort"));
            options.push(json!({ "type": "effort", "values": model.reasoning_levels }));
            set(&mut item, &["reasoningOptions"], json!(options))?;
        }
        if let Some(v) = model.context_window {
            set(&mut item, &["contextWindow"], json!(v))?;
        }
        if let Some(v) = model.max_output_tokens {
            set(&mut item, &["maxTokens"], json!(v))?;
        }
        set(&mut provider, &["models", &model.id], item)?;
    }
    if let Some(model) = profile
        .models
        .iter()
        .find(|model| model.id == profile.model)
    {
        if let Some(v) = model.context_window {
            set(
                value,
                &["providers", id, "settings", "contextWindow"],
                json!(v),
            )?;
        }
        if let Some(v) = model.max_output_tokens {
            set(value, &["providers", id, "settings", "maxTokens"], json!(v))?;
        }
    }
    if models.get("version").is_none() {
        set(&mut models, &["version"], json!(1))?;
    }
    set(&mut models, &["providers", id], provider)?;
    change.after = Some(files::render(ConfigFormat::Json, &original, &models)?);
    Ok(vec![change])
}
