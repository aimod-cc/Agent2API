use super::super::Profile;
use super::{model, native_entry, set, string, trim_chat_url};
use serde_json::{json, Value};

fn entries(value: &Value) -> Result<&Vec<Value>, String> {
    value
        .as_array()
        .or_else(|| value.get("models").and_then(Value::as_array))
        .ok_or_else(|| "WorkBuddy models.json 必须是模型数组或包含 models 数组的对象".into())
}

pub fn read(value: &Value) -> Result<Vec<Profile>, String> {
    let entries = entries(value)?;
    let mut profiles: Vec<Profile> = Vec::new();
    for item in entries {
        let url = string(item, "url");
        let id = string(item, "id");
        if url.is_empty() || id.is_empty() {
            continue;
        }
        let base = trim_chat_url(&url);
        let key = string(item, "apiKey");
        let enabled = !item
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && value
                .get("availableModels")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty())
                .map(|a| a.iter().any(|v| v.as_str() == Some(&id)))
                .unwrap_or(true);
        let index = profiles
            .iter()
            .position(|p| p.base_url == base && p.api_key == key && p.enabled == enabled);
        let index = if let Some(index) = index {
            index
        } else {
            let stable = super::super::store::digest(format!("{base}:{key}:{enabled}").as_bytes());
            let mut profile = Profile::new(
                &format!("wb-{}", &stable[..16]),
                &format!("{} · {}", string(item, "vendor"), base),
                "openai_chat",
                &base,
            );
            profile.api_key = key;
            profile.enabled = enabled;
            profile.default = false;
            profile.native.entry = json!([]);
            profiles.push(profile);
            profiles.len() - 1
        };
        let profile = &mut profiles[index];
        let mut model = model(&id, item);
        model.context_window = item.get("maxInputTokens").and_then(Value::as_u64);
        model.max_output_tokens = item.get("maxOutputTokens").and_then(Value::as_u64);
        model.supports_tools = item.get("supportsToolCall").and_then(Value::as_bool);
        model.supports_images = item.get("supportsImages").and_then(Value::as_bool);
        model.supports_reasoning = item.get("supportsReasoning").and_then(Value::as_bool);
        model.reasoning_levels = item
            .pointer("/reasoning/supportedEfforts")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        model.effort = item
            .pointer("/reasoning/defaultEffort")
            .or_else(|| item.pointer("/reasoning/effort"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .into();
        profile.models.push(model);
        profile
            .native
            .entry
            .as_array_mut()
            .ok_or("模型原生数据不一致")?
            .push(item.clone());
    }
    Ok(profiles)
}

pub fn write(value: &mut Value, profile: &Profile, warnings: &mut Vec<String>) -> Result<(), String> {
    let original = entries(value)?.clone();
    let imported = profile.native.entry.as_array().cloned().unwrap_or_default();
    let mut result = original.clone();
    let key = super::super::validate::credential(profile)?;
    for model in &profile.models {
        let old = original
            .iter()
            .find(|v| v.get("id").and_then(Value::as_str) == Some(&model.id));
        let known = imported
            .iter()
            .find(|v| v.get("id").and_then(Value::as_str) == Some(&model.id));
        if old.is_some()
            && known.is_none()
            && old.and_then(|v| v.get("vendor")).and_then(Value::as_str) != Some("Agent2API")
        {
            warnings.push(format!(
                "模型 ID {} 已存在，确认后将用当前配置覆盖该条目；如需保留，请返回编辑并改用其他模型 ID。",
                model.id
            ));
        }
        let mut item = native_entry(old, known.unwrap_or(&Value::Null))?;
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
        if known.is_none() {
            set(&mut item, &["vendor"], json!("Agent2API"))?;
        }
        set(
            &mut item,
            &["url"],
            json!(format!(
                "{}/chat/completions",
                trim_chat_url(&profile.base_url)
            )),
        )?;
        set(&mut item, &["useCustomProtocol"], json!(false))?;
        set(&mut item, &["disabled"], json!(!profile.enabled))?;
        if let Some(key) = &key {
            set(&mut item, &["apiKey"], json!(key))?;
        }
        if let Some(v) = model.context_window {
            set(&mut item, &["maxInputTokens"], json!(v))?;
        }
        if let Some(v) = model.max_output_tokens {
            set(&mut item, &["maxOutputTokens"], json!(v))?;
        }
        if let Some(v) = model.supports_tools {
            set(&mut item, &["supportsToolCall"], json!(v))?;
        }
        if let Some(v) = model.supports_images {
            set(&mut item, &["supportsImages"], json!(v))?;
        }
        if let Some(v) = model.supports_reasoning {
            set(&mut item, &["supportsReasoning"], json!(v))?;
        }
        if !model.reasoning_levels.is_empty() {
            set(
                &mut item,
                &["reasoning", "supportedEfforts"],
                json!(model.reasoning_levels),
            )?;
        }
        if !model.effort.is_empty() && model.effort != "default" {
            set(
                &mut item,
                &["reasoning", "defaultEffort"],
                json!(model.effort),
            )?;
        } else {
            super::remove(&mut item, &["reasoning", "defaultEffort"]);
            super::remove(&mut item, &["reasoning", "effort"]);
        }
        if let Some(index) = result
            .iter()
            .position(|v| v.get("id").and_then(Value::as_str) == Some(&model.id))
        {
            result[index] = item;
        } else {
            result.push(item);
        }
    }
    if value.is_array() {
        *value = Value::Array(result);
    } else {
        set(value, &["models"], Value::Array(result))?;
        if let Some(available) = value
            .get_mut("availableModels")
            .and_then(Value::as_array_mut)
            .filter(|v| !v.is_empty())
        {
            for model in &profile.models {
                available.retain(|v| v.as_str() != Some(&model.id));
                if profile.enabled {
                    available.push(json!(model.id));
                }
            }
        }
    }
    Ok(())
}
