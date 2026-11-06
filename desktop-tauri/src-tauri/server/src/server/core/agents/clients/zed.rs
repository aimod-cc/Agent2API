use super::super::{Model, Profile};
use super::{native_entry, native_id, set, string};
use serde_json::{json, Value};

pub fn read(value: &Value) -> Result<Vec<Profile>, String> {
    if !value.is_object() {
        return Err("Zed settings.json 必须是对象".into());
    }
    let active_provider = value
        .pointer("/agent/default_model/provider")
        .and_then(Value::as_str)
        .unwrap_or("");
    let active_model = value
        .pointer("/agent/default_model/model")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut out = Vec::new();
    if let Some(providers) = value
        .pointer("/language_models/openai_compatible")
        .and_then(Value::as_object)
    {
        for (id, entry) in providers {
            let base = string(entry, "api_url");
            for protocol in ["openai_chat", "responses"] {
                let mut profile = Profile::new(&format!("{id}-{protocol}"), id, protocol, &base);
                profile.native.source_id = id.clone();
                profile.native.entry = entry.clone();
                profile.native.settings = json!({ "protocol": protocol });
                profile.enabled = false;
                profile.default = false;
                if let Some(models) = entry.get("available_models").and_then(Value::as_array) {
                    for entry in models {
                        let responses = entry
                            .pointer("/capabilities/chat_completions")
                            .and_then(Value::as_bool)
                            == Some(false);
                        if responses != (protocol == "responses") {
                            continue;
                        }
                        let id = string(entry, "name");
                        profile.models.push(Model {
                            id: id.clone(),
                            name: string(entry, "display_name"),
                            context_window: entry.get("max_tokens").and_then(Value::as_u64),
                            max_output_tokens: entry
                                .get("max_output_tokens")
                                .and_then(Value::as_u64),
                            supports_tools: entry
                                .pointer("/capabilities/tools")
                                .and_then(Value::as_bool),
                            supports_images: entry
                                .pointer("/capabilities/images")
                                .and_then(Value::as_bool),
                            supports_video: None,
                            supports_reasoning: entry.get("reasoning_effort").map(|_| true),
                            effort: string(entry, "reasoning_effort"),
                            reasoning_levels: entry
                                .get("reasoning_effort")
                                .and_then(Value::as_str)
                                .map(|effort| vec![effort.into()])
                                .unwrap_or_default(),
                        });
                        if active_provider == profile.native.source_id && id == active_model {
                            profile.model = id;
                            profile.enabled = true;
                            profile.default = true;
                        }
                    }
                }
                if profile.model.is_empty() {
                    profile.model = profile
                        .models
                        .first()
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                }
                if !profile.models.is_empty() {
                    out.push(profile);
                }
            }
        }
    }
    Ok(out)
}

pub fn write(
    value: &mut Value,
    profile: &Profile,
    operation: &str,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let id = native_id(profile);
    let current = value
        .pointer("/language_models/openai_compatible")
        .and_then(|v| v.get(&id));
    let mut entry = native_entry(current, &profile.native.entry)?;
    set(&mut entry, &["api_url"], json!(profile.base_url))?;
    let mut models = entry
        .get("available_models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for model in &profile.models {
        let source_protocol = profile.native.settings.get("protocol").and_then(Value::as_str).unwrap_or(&profile.protocol);
        let index = models.iter().position(|value| value.get("name").and_then(Value::as_str) == Some(&model.id)
            && (value.pointer("/capabilities/chat_completions").and_then(Value::as_bool) == Some(false)) == (source_protocol == "responses"));
        let mut item = index
            .map(|i| models[i].clone())
            .unwrap_or_else(|| json!({}));
        set(&mut item, &["name"], json!(model.id))?;
        set(
            &mut item,
            &["display_name"],
            json!(if model.name.is_empty() {
                &model.id
            } else {
                &model.name
            }),
        )?;
        if let Some(v) = model.context_window {
            set(&mut item, &["max_tokens"], json!(v))?;
        }
        if let Some(v) = model.max_output_tokens {
            set(&mut item, &["max_output_tokens"], json!(v))?;
        }
        set(
            &mut item,
            &["capabilities", "chat_completions"],
            json!(profile.protocol != "responses"),
        )?;
        if let Some(v) = model.supports_tools {
            set(&mut item, &["capabilities", "tools"], json!(v))?;
        }
        if let Some(v) = model.supports_images {
            set(&mut item, &["capabilities", "images"], json!(v))?;
        }
        if model.effort.is_empty() || model.effort == "default" {
            super::remove(&mut item, &["reasoning_effort"]);
        } else {
            set(&mut item, &["reasoning_effort"], json!(model.effort))?;
        }
        if let Some(i) = index {
            models[i] = item;
        } else {
            models.push(item);
        }
    }
    set(&mut entry, &["available_models"], json!(models))?;
    set(value, &["language_models", "openai_compatible", &id], entry)?;
    if operation == "default" {
        set(
            value,
            &["agent", "default_model"],
            json!({ "provider": id, "model": profile.model }),
        )?;
    }
    warnings.push(format!(
        "请在 Zed 的提供商「{id}」中填写 API Key，或设置 {}_API_KEY；此操作只写配置文件。",
        id.replace('-', "_").to_ascii_uppercase()
    ));
    Ok(())
}
