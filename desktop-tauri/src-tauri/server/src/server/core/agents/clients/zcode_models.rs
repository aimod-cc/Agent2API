use serde_json::{json, Value};

use super::{merge, string};
use super::super::{Model, Profile};

fn set(value: &mut Value, path: &[&str], item: Value) -> Result<(), String> {
    let (key, rest) = path.split_first().ok_or("ZCode 模型字段路径不能为空")?;
    if value.is_null() { *value = json!({}); }
    let object = value.as_object_mut().ok_or("ZCode 模型字段不是对象，拒绝覆盖")?;
    if rest.is_empty() { object.insert((*key).into(), item); return Ok(()); }
    set(object.entry((*key).to_string()).or_insert_with(|| json!({})), rest, item)
}

fn entries(value: &Value, key: &str) -> Result<Vec<Value>, String> {
    let Some(value) = value.get("config").and_then(|value| value.get("modelConfigRules")).and_then(|value| value.get(key)) else {
        return Ok(Vec::new());
    };
    value.as_array().cloned().ok_or_else(|| format!("ZCode {key} 必须是数组，拒绝覆盖"))
}

fn import(rules: &mut Vec<Value>, imported: &Value, provider: &str) {
    for entry in imported.as_array().into_iter().flatten() {
        let model = string(entry, "modelId");
        if !rules.iter().any(|rule| string(rule, "providerId") == provider && string(rule, "modelId") == model) {
            rules.push(entry.clone());
        }
    }
}

pub(super) fn write(value: &mut Value, profile: &Profile, provider: &str) -> Result<(), String> {
    let mut smart = entries(value, "providerModelRules")?;
    let mut manual = entries(value, "manualProviderModelRules")?;
    import(&mut smart, &profile.native.model_entries, provider);
    import(&mut manual, &profile.native.manual_model_entries, provider);
    for model in &profile.models {
        let matches = |rule: &Value| string(rule, "providerId") == provider && string(rule, "modelId") == model.id;
        for rule in smart.iter().chain(&manual).filter(|rule| matches(rule)) {
            if !rule.get("config").is_some_and(Value::is_object) {
                return Err(format!("ZCode 模型 {} 的配置不是对象，拒绝覆盖", model.id));
            }
        }
        let old_index = smart.iter().position(|rule| matches(rule)).unwrap_or(smart.len());
        let mut entry = json!({ "providerId": provider, "modelId": model.id, "config": {} });
        for rule in smart.iter().filter(|rule| matches(rule)).chain(manual.iter().filter(|rule| matches(rule))) {
            entry = merge(Some(&entry), rule);
        }
        // 手动规则要求完整字段，部分能力覆盖只能放在智能规则中；同一模型不能同时存在于两处。
        smart.retain(|rule| !matches(rule));
        manual.retain(|rule| !matches(rule));
        if entry.get("config").is_none_or(Value::is_null) { entry["config"] = json!({}); }
        if profile.id == "quick-zcode" { set(&mut entry, &["config", "enabled"], json!(true))?; }
        if let Some(value) = model.context_window { set(&mut entry, &["config", "properties", "contextWindow"], json!(value))?; }
        if let Some(value) = model.max_output_tokens { set(&mut entry, &["config", "optionSpecs", "maxOutputTokens", "max"], json!(value))?; }
        if let Some(value) = model.supports_tools { set(&mut entry, &["config", "properties", "supportsToolCall"], json!(value))?; }
        if let Some(value) = model.supports_images { set(&mut entry, &["config", "properties", "inputFormat", "supportsImage"], json!(value))?; }
        if let Some(value) = model.supports_video { set(&mut entry, &["config", "properties", "inputFormat", "supportsVideo"], json!(value))?; }
        if profile.kind == "gateway" { gateway_options(&mut entry, profile, model)?; }
        smart.insert(old_index.min(smart.len()), entry);
    }
    set(value, &["config", "modelConfigRules", "providerModelRules"], json!(smart))?;
    set(value, &["config", "modelConfigRules", "manualProviderModelRules"], json!(manual))
}

fn gateway_options(entry: &mut Value, profile: &Profile, model: &Model) -> Result<(), String> {
    if model.max_output_tokens.is_some() {
        let map = if profile.protocol == "responses" { r#"{"max_output_tokens": maxOutputTokens}"# }
            else { r#"{"max_tokens": maxOutputTokens}"# };
        set(entry, &["config", "optionSpecs", "maxOutputTokens", "map"], json!(map))?;
    }
    if model.supports_reasoning == Some(false) {
        set(entry, &["config", "optionSpecs", "reasoningLevel"], json!({ "values": ["disabled"], "map": "{}" }))?;
    } else if !model.reasoning_levels.is_empty() {
        let map = match profile.protocol.as_str() {
            "responses" => r#"{"reasoning": {"effort": reasoningLevel}}"#,
            "anthropic" => r#"reasoningLevel == "none" || reasoningLevel == "off" || reasoningLevel == "disabled" ? {"thinking": {"type": "disabled"}} : {"thinking": {"type": "adaptive"}, "output_config": {"effort": reasoningLevel}}"#,
            _ => r#"{"reasoning_effort": reasoningLevel}"#,
        };
        set(entry, &["config", "optionSpecs", "reasoningLevel"], json!({ "values": model.reasoning_levels, "map": map }))?;
    }
    Ok(())
}
