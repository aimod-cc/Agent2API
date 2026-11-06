use super::super::{Model, Profile};
use super::{merge, native_entry, native_id, set, string};
use serde_json::{json, Value};

pub fn read(value: &Value) -> Result<Vec<Profile>, String> {
    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    if version != 1 {
        return Err(format!("ZCode 配置 schema {version} 尚未适配，已停止写入"));
    }
    let Some(rules) = value
        .pointer("/config/providerConfigRules/providerRules")
        .and_then(Value::as_array)
    else {
        if value.as_object().is_some_and(|v| v.is_empty()) {
            return Ok(Vec::new());
        }
        return Err("ZCode 缺少 providerRules，不能使用通用配置写入".into());
    };
    let model_rules = value
        .pointer("/config/modelConfigRules/providerModelRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let manual_rules = value
        .pointer("/config/modelConfigRules/manualProviderModelRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut profiles = Vec::new();
    for rule in rules {
        let id = string(rule, "providerId");
        let base = rule
            .pointer("/config/api/baseUrl")
            .and_then(Value::as_str)
            .unwrap_or("");
        if id.is_empty() || base.is_empty() {
            continue;
        }
        let protocol = match rule
            .pointer("/config/api/type")
            .and_then(Value::as_str)
            .unwrap_or("")
        {
            "openai-chat-completions" => "openai_chat",
            "openai-responses" => "responses",
            "anthropic-messages" => "anthropic",
            other => other,
        };
        let mut profile = Profile::new(&id, &string(rule, "providerName"), protocol, base);
        profile.enabled = rule
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        profile.default = false;
        profile.api_key = rule
            .pointer("/config/access/apiKey")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into();
        profile.native.entry = rule.clone();
        let mine = |rule: &&Value| rule.get("providerId").and_then(Value::as_str) == Some(&id);
        profile.native.model_entries =
            Value::Array(model_rules.iter().filter(mine).cloned().collect());
        profile.native.manual_model_entries =
            Value::Array(manual_rules.iter().filter(mine).cloned().collect());
        let mut ids = rule
            .pointer("/config/personalModelIds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for item in model_rules.iter().chain(&manual_rules).filter(|item| string(item, "providerId") == id) {
            if let Some(model) = item.get("modelId").filter(|model| model.is_string()) {
                if !ids.contains(model) { ids.push(model.clone()); }
            }
        }
        for model_id in ids.iter().filter_map(Value::as_str) {
            let mut config = json!({});
            for item in model_rules.iter().chain(&manual_rules).filter(|rule| {
                rule.get("providerId").and_then(Value::as_str) == Some(&id)
                    && rule.get("modelId").and_then(Value::as_str) == Some(model_id)
            }) {
                config = merge(Some(&config), item.get("config").unwrap_or(&Value::Null));
            }
            let reasoning_levels = config.pointer("/optionSpecs/reasoningLevel/values")
                .and_then(Value::as_array).map(|values| values.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
                .unwrap_or_default();
            profile.models.push(Model {
                id: model_id.into(),
                name: model_id.into(),
                context_window: config
                    .pointer("/properties/contextWindow")
                    .and_then(Value::as_u64),
                max_output_tokens: config
                    .pointer("/optionSpecs/maxOutputTokens/max")
                    .and_then(Value::as_u64),
                supports_images: config
                    .pointer("/properties/inputFormat/supportsImage")
                    .and_then(Value::as_bool),
                supports_video: config.pointer("/properties/inputFormat/supportsVideo").and_then(Value::as_bool),
                supports_tools: config.pointer("/properties/supportsToolCall").and_then(Value::as_bool),
                supports_reasoning: (!reasoning_levels.is_empty()).then(|| reasoning_levels.iter()
                    .any(|level| !matches!(level.as_str(), "disabled" | "none" | "off"))),
                reasoning_levels,
                ..Default::default()
            });
        }
        profiles.push(profile);
    }
    Ok(profiles)
}

pub fn write(value: &mut Value, profile: &Profile) -> Result<(), String> {
    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    if version != 1 {
        return Err("ZCode 原生配置版本暂不支持".into());
    }
    if value
        .pointer("/config/providerConfigRules/providerRules")
        .is_none()
        && !value.as_object().is_some_and(|v| v.is_empty())
    {
        return Err("ZCode 配置缺少原生 providerRules".into());
    }
    let id = native_id(profile);
    let rules = value
        .pointer("/config/providerConfigRules/providerRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let existing = rules
        .iter()
        .find(|v| v.get("providerId").and_then(Value::as_str) == Some(&id));
    let mut rule = native_entry(existing, &profile.native.entry)?;
    set(&mut rule, &["providerId"], json!(id))?;
    set(&mut rule, &["providerName"], json!(profile.name))?;
    set(&mut rule, &["enabled"], json!(profile.enabled))?;
    if rule.pointer("/config/group").is_none() {
        set(&mut rule, &["config", "group"], json!("standard-personal"))?;
    }
    set(
        &mut rule,
        &["config", "api", "type"],
        json!(match profile.protocol.as_str() {
            "anthropic" => "anthropic-messages",
            "responses" => "openai-responses",
            _ => "openai-chat-completions",
        }),
    )?;
    set(
        &mut rule,
        &["config", "api", "baseUrl"],
        json!(profile.base_url),
    )?;
    if let Some(key) = super::super::validate::credential(profile)? {
        set(&mut rule, &["config", "access", "type"], json!("api-key"))?;
        set(&mut rule, &["config", "access", "apiKey"], json!(key))?;
    }
    let mut ids = rule
        .pointer("/config/personalModelIds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for model in &profile.models {
        if !ids.iter().any(|v| v.as_str() == Some(&model.id)) {
            ids.push(json!(model.id));
        }
    }
    set(&mut rule, &["config", "personalModelIds"], json!(ids))?;
    let mut order = rule
        .pointer("/config/modelOrder")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for id in &ids {
        if !order.contains(id) {
            order.push(id.clone());
        }
    }
    set(&mut rule, &["config", "modelOrder"], json!(order))?;
    let mut next = rules;
    if let Some(index) = next
        .iter()
        .position(|v| v.get("providerId").and_then(Value::as_str) == Some(&id))
    {
        next[index] = rule;
    } else {
        next.push(rule);
    }
    set(value, &["schemaVersion"], json!(1))?;
    set(
        value,
        &["config", "providerConfigRules", "providerRules"],
        json!(next),
    )?;
    let mut order = value
        .pointer("/config/providerOrder")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !order.iter().any(|v| v.as_str() == Some(&id)) {
        order.push(json!(id));
    }
    set(value, &["config", "providerOrder"], json!(order))?;
    super::zcode_models::write(value, profile, &id)
}
