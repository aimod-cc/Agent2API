use super::super::{Model, Profile};
use super::{native_entry, native_id, set, string};
use serde_json::{json, Value};
use std::path::Path;

pub fn read(path: &Path, value: &Value) -> Result<Vec<Profile>, String> {
    if !value.is_object() {
        return Err("Kimi Code 配置必须是表".into());
    }
    let selected = string(value, "default_model");
    let legacy = super::super::paths::kimi_legacy(path);
    let models = value.get("models").and_then(Value::as_object);
    let providers = value.get("providers").and_then(Value::as_object);
    let mut out = Vec::new();
    if let Some(providers) = providers {
        for (id, entry) in providers {
            let base = string(entry, "base_url");
            let protocol = match string(entry, "type").as_str() {
                "kimi" | "openai" | "openai_legacy" => "openai_chat",
                "openai_responses" => "responses",
                "anthropic" => "anthropic",
                _ => "unsupported",
            };
            let mut profile = Profile::new(id, id, protocol, &base);
            profile.enabled = false;
            profile.default = false;
            profile.native.entry = entry.clone();
            profile.api_key = string(entry, "api_key");
            let mut native_models = serde_json::Map::new();
            if let Some(models) = models {
                for (key, entry) in models
                    .iter()
                    .filter(|(_, entry)| entry.get("provider").and_then(Value::as_str) == Some(id))
                {
                    let effective = if legacy { entry.clone() } else { super::merge(Some(entry), entry.get("overrides").unwrap_or(&Value::Null)) };
                    let model_id = string(entry, "model");
                    let capabilities = effective.get("capabilities").and_then(Value::as_array);
                    let capability = |name: &str| capabilities.is_some_and(|values| values.iter().any(|value| value == name)).then_some(true);
                    profile.models.push(Model {
                        id: model_id.clone(),
                        name: string(&effective, "display_name"),
                        context_window: effective.get("max_context_size").and_then(Value::as_u64),
                        max_output_tokens: if legacy || protocol != "anthropic" { None } else { effective.get("max_output_size").and_then(Value::as_u64) },
                        supports_tools: if legacy { None } else { capability("tool_use") },
                        supports_images: capability("image_in"),
                        supports_video: capability("video_in"),
                        supports_reasoning: capability("thinking").or_else(|| capability("always_thinking")),
                        reasoning_levels: if legacy { Vec::new() } else { effective.get("support_efforts").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() },
                        effort: if legacy { String::new() } else { string(&effective, "default_effort") },
                        ..Default::default()
                    });
                    native_models.insert(key.clone(), entry.clone());
                    if key == &selected {
                        profile.enabled = true;
                        profile.default = true;
                        profile.model = model_id;
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
            profile.native.model_entries = Value::Object(native_models);
            out.push(profile);
        }
    }
    Ok(out)
}

pub fn write(
    path: &Path,
    value: &mut Value,
    profile: &Profile,
    operation: &str,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let id = native_id(profile);
    let legacy = super::super::paths::kimi_legacy(path);
    let mut provider = native_entry(
        value.get("providers").and_then(|v| v.get(&id)),
        &profile.native.entry,
    )?;
    let protocol = match profile.protocol.as_str() {
        "anthropic" => "anthropic",
        "responses" => "openai_responses",
        _ => {
            if legacy {
                "openai_legacy"
            } else {
                "openai"
            }
        }
    };
    let endpoint_changed = string(&provider, "base_url") != profile.base_url || string(&provider, "type") != protocol;
    set(&mut provider, &["type"], json!(protocol))?;
    set(&mut provider, &["base_url"], json!(profile.base_url))?;
    super::apply_credential(&mut provider, &["api_key"], profile)?;
    if profile.auth_mode != "preserve" {
        super::remove(&mut provider, &["api_key_env"]);
        super::remove(&mut provider, &["oauth"]);
    }
    set(value, &["providers", &id], provider)?;
    let imported = profile.native.model_entries.as_object();
    let mut default_key = String::new();
    for model in &profile.models {
        let original_key = imported
            .and_then(|items| {
                items.iter().find(|(_, entry)| {
                    entry.get("model").and_then(Value::as_str) == Some(&model.id)
                })
            })
            .map(|(key, _)| key.clone());
        let key = original_key.or_else(|| value.get("models").and_then(Value::as_object).and_then(|items| items.iter()
            .find(|(_, entry)| string(entry, "provider") == id && string(entry, "model") == model.id).map(|(key, _)| key.clone())))
            .unwrap_or_else(|| format!("{id}/{}", model.id));
        let existing = value.get("models").and_then(|v| v.get(&key));
        if existing.is_some_and(|entry| string(entry, "provider") != id) {
            return Err(format!("Kimi 模型名称 {key} 已由其它提供商使用，拒绝覆盖"));
        }
        let original = imported.and_then(|v| v.get(&key)).unwrap_or(&Value::Null);
        let mut entry = native_entry(existing, original)?;
        set(&mut entry, &["provider"], json!(id))?;
        set(&mut entry, &["model"], json!(model.id))?;
        if endpoint_changed && !legacy {
            for key in ["base_url", "protocol"] { super::remove(&mut entry, &[key]); }
        }
        let has_overrides = !legacy && entry.get("overrides").is_some();
        let base_metadata = entry.clone();
        let metadata = if has_overrides {
            entry.get_mut("overrides").ok_or("Kimi 模型覆盖配置不可读取")?
        } else { &mut entry };
        set(
            metadata,
            &["display_name"],
            json!(if model.name.is_empty() { &model.id } else { &model.name }),
        )?;
        if let Some(v) = model.context_window {
            set(metadata, &["max_context_size"], json!(v))?;
        }
        if !legacy && profile.protocol == "anthropic" {
            if let Some(v) = model.max_output_tokens {
                set(metadata, &["max_output_size"], json!(v))?;
            }
        }
        let mut caps = metadata.get("capabilities").or_else(|| base_metadata.get("capabilities")).and_then(Value::as_array).cloned().unwrap_or_default();
        let mut changed = false;
        for (name, flag) in [("image_in", model.supports_images), ("video_in", model.supports_video),
            ("thinking", model.supports_reasoning), ("tool_use", if legacy { None } else { model.supports_tools })] {
            if flag == Some(true) && !caps.iter().any(|value| value == name) { caps.push(json!(name)); changed = true; }
        }
        if changed { set(metadata, &["capabilities"], json!(caps))?; }
        if !legacy {
            if model.supports_reasoning == Some(false) { super::remove(metadata, &["support_efforts"]); }
            else if !model.reasoning_levels.is_empty() { set(metadata, &["support_efforts"], json!(model.reasoning_levels))?; }
            if model.effort.is_empty() || model.effort == "default" { super::remove(metadata, &["default_effort"]); }
            else { set(metadata, &["default_effort"], json!(model.effort))?; }
        }
        if has_overrides {
            if model.effort.is_empty() || model.effort == "default" { super::remove(&mut entry, &["default_effort"]); }
            if model.supports_reasoning == Some(false) { super::remove(&mut entry, &["support_efforts"]); }
        }
        set(value, &["models", &key], entry)?;
        if model.id == profile.model {
            default_key = key;
        }
    }
    if operation == "default" {
        set(value, &["default_model"], json!(default_key))?;
    }
    if profile.models.iter().any(|model| [model.supports_tools, model.supports_images, model.supports_video, model.supports_reasoning].contains(&Some(false))) {
        warnings.push("Kimi 能力标签只能声明支持，不能关闭客户端内置识别的能力；不支持的标记不会写成关闭开关。".into());
    }
    if !legacy && profile.models.iter().any(|model| !model.effort.is_empty())
        && (value.pointer("/thinking/effort").is_some() || value.pointer("/thinking/enabled").and_then(Value::as_bool) == Some(false)) {
        warnings.push("Kimi 已有的全局思考设置会影响逐模型档位；本次保留全局设置，请在客户端调整后确认。".into());
    }
    Ok(())
}
