use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::prompt::rules::{self, CompiledRule, Condition, Content, Field, Logic, Matcher, Operator, Rule};
use crate::server::errors::GatewayError;

use super::{ProviderPrompt, PromptSettings, KEY_PROMPT_FILE, KEY_PROMPT_GATEWAY, KEY_PROMPT_GATEWAY_TEXT,
    KEY_PROMPT_MODE, KEY_PROMPT_PROVIDERS, KEY_PROMPT_TEXT};

pub const KEY_PROMPT_RULES: &str = "promptRules";

pub fn revision(raw: &Map<String, Value>) -> String {
    let values: Vec<_> = [KEY_PROMPT_RULES, KEY_PROMPT_MODE, KEY_PROMPT_FILE, KEY_PROMPT_TEXT,
        KEY_PROMPT_PROVIDERS, KEY_PROMPT_GATEWAY, KEY_PROMPT_GATEWAY_TEXT]
        .iter().map(|key| raw.get(*key).cloned().unwrap_or(Value::Null)).collect();
    format!("{:x}", Sha256::digest(Value::Array(values).to_string().as_bytes()))
}

pub(crate) fn from_raw(raw: &Map<String, Value>) -> (Vec<CompiledRule>, bool, Option<String>) {
    let Some(value) = raw.get(KEY_PROMPT_RULES) else { return (Vec::new(), false, None); };
    let result = serde_json::from_value::<Vec<Rule>>(value.clone())
        .map_err(|error| format!("提示词规则格式不正确：{error}"))
        .and_then(|rules| { rules::validate_rules(&rules)?; compile(rules) });
    match result {
        Ok(rules) => (rules, true, None),
        Err(error) => (Vec::new(), true, Some(error)),
    }
}

fn compile(rules: Vec<Rule>) -> Result<Vec<CompiledRule>, String> {
    rules.into_iter().map(|rule| {
        let matchers = rule.conditions.iter().map(Matcher::compile).collect::<Result<Vec<_>, _>>()?;
        let content = rule.content();
        let prompt = resolve(&content);
        Ok(CompiledRule { rule, prompt, matchers })
    }).collect()
}

pub fn resolve(content: &Content) -> ProviderPrompt {
    let choice = super::resolve_choice(content.mode,
        Some(content.file.clone()).filter(|text| !text.trim().is_empty()),
        Some(content.text.clone()).filter(|text| !text.trim().is_empty()));
    ProviderPrompt { mode: choice.mode, file: choice.file, inline: choice.inline,
        text: choice.text, source: choice.source, file_error: choice.file_error }
}

pub fn resolved_fallback(settings: &PromptSettings) -> ProviderPrompt {
    ProviderPrompt { mode: settings.mode, file: settings.file.clone(), inline: settings.inline.clone(),
        text: settings.text.clone(), source: settings.source, file_error: settings.file_error.clone() }
}

pub fn fallback(settings: &PromptSettings) -> Content {
    Content { mode: settings.mode, file: settings.file.clone().unwrap_or_default(),
        text: settings.inline.clone().unwrap_or_default() }
}

/// 旧覆盖只作等价投影，不在读取时写库，也不改变官方提示词开关。
pub fn legacy_rules(settings: &PromptSettings) -> Vec<Rule> {
    settings.providers.iter().enumerate().map(|(index, (provider, prompt))| Rule {
        id: format!("legacy-provider-{index}"),
        name: crate::server::core::providers::label_of(provider),
        enabled: true,
        logic: Logic::All,
        conditions: vec![Condition { id: format!("legacy-condition-{index}"), field: Field::Provider,
            op: Operator::Is, values: vec![provider.clone()] }],
        mode: prompt.mode,
        file: prompt.file.clone().unwrap_or_default(),
        text: prompt.inline.clone().unwrap_or_default(),
    }).collect()
}

fn validate_file(content: &Content) -> Result<(), GatewayError> {
    rules::validate_content(content).map_err(GatewayError::bad_request)?;
    if content.mode != crate::server::core::prompt::PromptMode::Passthrough
        && content.text.trim().is_empty() && !content.file.trim().is_empty() {
        super::read_prompt_file(&content.file).map_err(GatewayError::bad_request)?;
    }
    Ok(())
}

pub fn save(rules: Vec<Rule>, fallback: Content, expected: &str) -> Result<(), GatewayError> {
    rules::validate_rules(&rules).map_err(GatewayError::bad_request)?;
    validate_file(&fallback)?;
    for rule in &rules { if rule.enabled { validate_file(&rule.content())?; } }
    let rules = serde_json::to_value(&rules).map_err(|error| GatewayError::new(error.to_string()))?;
    super::try_update_raw_fields(|raw| {
        if revision(raw) != expected {
            return Err(GatewayError::with_status(409, "系统提示词已在其他位置修改，请刷新后再保存；当前草稿未丢失"));
        }
        raw.insert(KEY_PROMPT_RULES.into(), rules);
        raw.insert(KEY_PROMPT_MODE.into(), json!(fallback.mode.as_str()));
        for (key, value) in [(KEY_PROMPT_FILE, fallback.file), (KEY_PROMPT_TEXT, fallback.text)] {
            if value.trim().is_empty() { raw.remove(key); } else { raw.insert(key.into(), json!(value)); }
        }
        Ok(())
    }, || GatewayError::new("系统提示词保存失败，未应用本次修改"))
}

pub fn view_content(content: &Content, prompt: &ProviderPrompt) -> Value {
    json!({
        "mode": content.mode.as_str(), "file": content.file, "text": content.text,
        "effectiveText": prompt.text, "source": prompt.source.as_str(),
        "lines": prompt.text.lines().count(), "fileError": prompt.file_error,
    })
}

pub fn view_rule(rule: &Rule, prompt: &ProviderPrompt) -> Value {
    json!({
        "id": rule.id, "name": rule.name, "enabled": rule.enabled,
        "logic": rule.logic, "conditions": rule.conditions,
        "mode": rule.mode.as_str(), "file": rule.file, "text": rule.text,
        "effectiveText": prompt.text, "source": prompt.source.as_str(),
        "lines": prompt.text.lines().count(), "fileError": prompt.file_error,
    })
}
