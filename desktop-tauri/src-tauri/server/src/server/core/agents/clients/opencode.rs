use super::super::Profile;
use super::{model, native_entry, native_id, remove, set, string};
use serde_json::{json, Value};

fn supported(value: &Value) -> Result<(), String> {
    if value.get("providers").is_some() {
        return Err(
            "检测到 OpenCode v2 的 providers 格式；当前仅适配 v1 provider 格式，已停止写入".into(),
        );
    }
    Ok(())
}

pub fn read(value: &Value) -> Result<Vec<Profile>, String> {
    supported(value)?;
    if !value.is_object() {
        return Err("OpenCode 配置必须是对象".into());
    }
    let selected = string(value, "model");
    let (active, active_model) = selected.split_once('/').unwrap_or(("", ""));
    let mut profiles = Vec::new();
    if let Some(providers) = value.get("provider").and_then(Value::as_object) {
        for (id, entry) in providers {
            let base = entry
                .pointer("/options/baseURL")
                .and_then(Value::as_str)
                .unwrap_or("");
            if base.is_empty() {
                continue;
            }
            let protocol = match string(entry, "npm").as_str() {
                "@ai-sdk/anthropic" => "anthropic",
                "@ai-sdk/openai" => "responses",
                _ => "openai_chat",
            };
            let name = string(entry, "name");
            let mut profile =
                Profile::new(id, if name.is_empty() { id } else { &name }, protocol, base);
            profile.enabled = id == active;
            profile.default = profile.enabled;
            profile.native.entry = entry.clone();
            profile.api_key = entry
                .pointer("/options/apiKey")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into();
            if let Some(env) = profile
                .api_key
                .strip_prefix("{env:")
                .and_then(|key| key.strip_suffix('}'))
            {
                profile.env_key = env.into();
                profile.api_key.clear();
                profile.auth_mode = "env".into();
            }
            if let Some(models) = entry.get("models").and_then(Value::as_object) {
                for (id, entry) in models {
                    let mut model = model(id, entry);
                    model.context_window = entry.pointer("/limit/context").and_then(Value::as_u64);
                    model.max_output_tokens =
                        entry.pointer("/limit/output").and_then(Value::as_u64);
                    model.supports_tools = entry.get("tool_call").and_then(Value::as_bool);
                    model.supports_images = entry
                        .pointer("/modalities/input")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().any(|v| v == "image"));
                    model.supports_video = entry.pointer("/modalities/input").and_then(Value::as_array).map(|values| values.iter().any(|value| value == "video"));
                    model.supports_reasoning = entry.get("reasoning").and_then(Value::as_bool);
                    let effort_key = if protocol == "anthropic" { "effort" } else { "reasoningEffort" };
                    if let Some(variants) = entry.get("variants").and_then(Value::as_object) {
                        model.reasoning_levels = variants
                            .values()
                            .filter(|variant| {
                                variant.get("disabled").and_then(Value::as_bool) != Some(true)
                            })
                            .filter_map(|variant| {
                                if protocol == "anthropic" && variant.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled") {
                                    Some("none")
                                } else { variant.get(effort_key).and_then(Value::as_str) }
                            })
                            .map(str::to_string)
                            .collect();
                    }
                    model.effort = if protocol == "anthropic" && entry.pointer("/options/thinking/type").and_then(Value::as_str) == Some("disabled") {
                        "none".into()
                    } else { entry.get("options").and_then(|options| options.get(effort_key)).and_then(Value::as_str).unwrap_or("").into() };
                    if !model.effort.is_empty() && !model.reasoning_levels.contains(&model.effort) {
                        model.reasoning_levels.push(model.effort.clone());
                    }
                    profile.models.push(model);
                }
            }
            profile.model = if profile.enabled {
                active_model.into()
            } else {
                profile
                    .models
                    .first()
                    .map(|m| m.id.clone())
                    .unwrap_or_default()
            };
            profiles.push(profile);
        }
    }
    if !selected.is_empty() && !profiles.iter().any(|p| p.enabled) {
        let mut profile = Profile::new(active, "OpenCode 内置模型", "openai_chat", "");
        profile.model = active_model.into();
        profile.native.settings = json!({ "model": selected });
        profiles.push(profile);
    }
    Ok(profiles)
}

fn write_effort(value: &mut Value, effort: &str, anthropic: bool) -> Result<(), String> {
    if !anthropic { return set(value, &["reasoningEffort"], json!(effort)); }
    remove(value, &["reasoningEffort"]);
    if matches!(effort, "none" | "off" | "disabled") {
        set(value, &["thinking"], json!({ "type": "disabled" }))?;
        remove(value, &["effort"]);
    } else {
        if !matches!(value.pointer("/thinking/type").and_then(Value::as_str), Some("enabled" | "adaptive")) {
            set(value, &["thinking"], json!({ "type": "adaptive" }))?;
        }
        set(value, &["effort"], json!(effort))?;
    }
    Ok(())
}

pub fn write(value: &mut Value, profile: &Profile, operation: &str) -> Result<(), String> {
    supported(value)?;
    let id = native_id(profile);
    let mut entry = native_entry(
        value.get("provider").and_then(|v| v.get(&id)),
        &profile.native.entry,
    )?;
    set(&mut entry, &["name"], json!(profile.name))?;
    set(
        &mut entry,
        &["npm"],
        json!(match profile.protocol.as_str() {
            "anthropic" => "@ai-sdk/anthropic",
            "responses" => "@ai-sdk/openai",
            _ => "@ai-sdk/openai-compatible",
        }),
    )?;
    set(&mut entry, &["options", "baseURL"], json!(profile.base_url))?;
    if profile.auth_mode == "env" {
        set(
            &mut entry,
            &["options", "apiKey"],
            json!(format!("{{env:{}}}", profile.env_key)),
        )?;
    } else {
        super::apply_credential(&mut entry, &["options", "apiKey"], profile)?;
    }
    for model in &profile.models {
        let mut item = entry
            .get("models")
            .and_then(|v| v.get(&model.id))
            .cloned()
            .unwrap_or_else(|| json!({}));
        set(
            &mut item,
            &["name"],
            json!(if model.name.is_empty() {
                &model.id
            } else {
                &model.name
            }),
        )?;
        if let Some(value) = model.context_window {
            set(&mut item, &["limit", "context"], json!(value))?;
        }
        if let Some(value) = model.max_output_tokens {
            set(&mut item, &["limit", "output"], json!(value))?;
        }
        if let Some(value) = model.supports_tools {
            set(&mut item, &["tool_call"], json!(value))?;
        }
        if model.supports_images.is_some() || model.supports_video.is_some() {
            let mut input = item.pointer("/modalities/input").and_then(Value::as_array).cloned().unwrap_or_else(|| vec![json!("text")]);
            for (name, flag) in [("image", model.supports_images), ("video", model.supports_video)] {
                if let Some(flag) = flag { input.retain(|value| value != name); if flag { input.push(json!(name)); } }
            }
            set(&mut item, &["modalities", "input"], json!(input))?;
        }
        if let Some(value) = model.supports_reasoning {
            set(&mut item, &["reasoning"], json!(value))?;
        }
        if !model.reasoning_levels.is_empty() && model.supports_reasoning != Some(false) {
            for level in &model.reasoning_levels {
                let mut variant = item.get("variants").and_then(|variants| variants.get(level)).cloned().unwrap_or_else(|| json!({}));
                write_effort(&mut variant, level, profile.protocol == "anthropic")?;
                remove(&mut variant, &["disabled"]);
                set(&mut item, &["variants", level], variant)?;
            }
        }
        if model.supports_reasoning == Some(false) {
            if let Some(variants) = item.get_mut("variants").and_then(Value::as_object_mut) {
                variants.retain(|_, variant| variant.get("reasoningEffort").is_none() && variant.get("effort").is_none() && variant.get("thinking").is_none());
            }
        }
        if model.supports_reasoning == Some(false) || model.effort.is_empty() || model.effort == "default" {
            let had_effort = item.pointer("/options/effort").is_some();
            for key in ["reasoningEffort", "effort"] { remove(&mut item, &["options", key]); }
            if had_effort || model.supports_reasoning == Some(false) { remove(&mut item, &["options", "thinking"]); }
        } else {
            let mut options = item.get("options").cloned().unwrap_or_else(|| json!({}));
            write_effort(&mut options, &model.effort, profile.protocol == "anthropic")?;
            set(&mut item, &["options"], options)?;
        }
        set(&mut entry, &["models", &model.id], item)?;
    }
    set(value, &["provider", &id], entry)?;
    if operation == "default" {
        for key in ["disabled_providers"] {
            if let Some(providers) = value.get_mut(key).and_then(Value::as_array_mut) { providers.retain(|provider| provider != &id); }
        }
        if let Some(providers) = value.get_mut("enabled_providers").and_then(Value::as_array_mut) {
            if !providers.iter().any(|provider| provider == &id) { providers.push(json!(id)); }
        }
        if let Some(provider) = value.get_mut("provider").and_then(|providers| providers.get_mut(&id)) {
            if let Some(blacklist) = provider.get_mut("blacklist").and_then(Value::as_array_mut) { blacklist.retain(|id| !profile.models.iter().any(|model| id == &model.id)); }
            if let Some(whitelist) = provider.get_mut("whitelist").and_then(Value::as_array_mut) {
                for model in &profile.models { if !whitelist.iter().any(|id| id == &model.id) { whitelist.push(json!(model.id)); } }
            }
        }
        set(value, &["model"], json!(format!("{id}/{}", profile.model)))?;
    }
    Ok(())
}
