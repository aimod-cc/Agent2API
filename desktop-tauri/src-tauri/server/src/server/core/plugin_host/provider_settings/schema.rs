use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct Schema {
    pub version: u32,
    pub title: Value,
    #[serde(default)]
    pub description: Value,
    #[serde(default)]
    pub storage_provider: Option<String>,
    #[serde(default)]
    pub runtime_overrides: bool,
    #[serde(skip)]
    pub(super) group: Option<Group>,
    pub(super) fields: Vec<Field>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Group {
    pub label: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Field {
    pub key: String,
    pub label: Value,
    #[serde(default)]
    pub description: Value,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub trim: bool,
    #[serde(default)]
    pub empty_is_default: bool,
    #[serde(default)]
    pub min_length: Option<usize>,
    #[serde(default)]
    pub max_length: Option<usize>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub minimum: Option<f64>,
    #[serde(default)]
    pub maximum: Option<f64>,
    #[serde(default)]
    pub options: Vec<Choice>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Choice {
    pub value: Value,
    pub label: Value,
}

pub(super) fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && !matches!(value, "__proto__" | "prototype" | "constructor")
}

pub(super) fn text(value: &Value, optional: bool) -> bool {
    let valid = |value: &str| value.len() <= 8192 && !value.contains('\0') && (optional || !value.trim().is_empty());
    match value {
        Value::Null => optional,
        Value::String(value) => valid(value),
        Value::Object(values) => !values.is_empty() && values.len() <= 16
            && values.iter().all(|(locale, value)| identifier(locale) && value.as_str().is_some_and(valid)),
        _ => false,
    }
}

fn setting_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['_', '-', '.'], "");
    identifier(key) && !key.starts_with("host.") && !matches!(key,
        crate::server::config::PLUGIN_SETTING_GATEWAY_PROMPT
        | crate::server::config::PLUGIN_SETTING_GATEWAY_PROMPT_TEXT
        | crate::server::core::plugin_host::rpc_cache::SETTING_MODEL_CATALOG)
        && normalized != "key" && !["token", "secret", "password", "cookie", "authorization", "apikey", "credential"]
            .iter().any(|word| normalized.contains(word))
}

impl Field {
    pub fn normalize(&self, value: &Value) -> Result<Value, String> {
        if value.is_null() { return Ok(Value::Null); }
        if value.to_string().len() > 16 * 1024 { return Err("设置值超过大小限制".into()); }
        match self.kind.as_str() {
            "string" => {
                let value = value.as_str().ok_or("设置值必须是文本")?;
                let value = if self.trim { value.trim() } else { value };
                if value.is_empty() && self.empty_is_default { return Ok(Value::Null); }
                let length = value.chars().count();
                if value.contains('\0') || length < self.min_length.unwrap_or(0) || length > self.max_length.unwrap_or(8192) {
                    return Err("文本长度不符合插件声明的范围".into());
                }
                if let Some(pattern) = &self.pattern {
                    let regex = regex::Regex::new(pattern).map_err(|_| "插件设置校验规则无效")?;
                    if !regex.is_match(value) { return Err("文本格式不符合插件声明的规则".into()); }
                }
                Ok(Value::String(value.into()))
            }
            "number" => {
                let number = value.as_f64().filter(|value| value.is_finite()).ok_or("设置值必须是数字")?;
                if self.minimum.is_some_and(|minimum| number < minimum) || self.maximum.is_some_and(|maximum| number > maximum) {
                    return Err("数字不在插件声明的范围内".into());
                }
                Ok(value.clone())
            }
            "boolean" if value.is_boolean() => Ok(value.clone()),
            "select" if self.options.iter().any(|option| option.value == *value) => Ok(value.clone()),
            _ => Err("设置值不符合插件声明的类型或选项".into()),
        }
    }
}

impl Schema {
    pub fn owner<'a>(&'a self, provider: &'a str) -> &'a str {
        self.storage_provider.as_deref().unwrap_or(provider)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || !text(&self.title, false) || !text(&self.description, true)
            || self.fields.is_empty() || self.fields.len() > 64
            || self.storage_provider.as_deref().is_some_and(|owner| !identifier(owner)) {
            return Err("插件设置声明的版本、标题或范围无效".into());
        }
        let mut keys = HashSet::new();
        for field in &self.fields {
            let mut choices = HashSet::new();
            if field.options.iter().any(|option| !choices.insert(option.value.to_string())) {
                return Err("插件设置选项值不能重复".into());
            }
            if !setting_key(&field.key) || !keys.insert(&field.key) || !text(&field.label, false) || !text(&field.description, true)
                || !matches!(field.kind.as_str(), "string" | "number" | "boolean" | "select")
                || field.kind != "string" && (field.trim || field.empty_is_default || field.min_length.is_some() || field.max_length.is_some() || field.pattern.is_some())
                || field.kind != "number" && (field.minimum.is_some() || field.maximum.is_some())
                || field.kind != "select" && !field.options.is_empty()
                || field.min_length.unwrap_or(0) > field.max_length.unwrap_or(8192) || field.max_length.unwrap_or(8192) > 8192
                || field.minimum.zip(field.maximum).is_some_and(|(min, max)| min > max)
                || field.options.len() > 64 || field.kind == "select" && field.options.is_empty()
                || field.options.iter().any(|option| !text(&option.label, false) || !option.value.is_string()
                    || option.value.as_str().is_some_and(|value| value.is_empty() || value.len() > 1024 || value.contains('\0'))) {
                return Err("插件设置字段、选项或限制无效；凭据应通过账号入口保存".into());
            }
            if let Some(pattern) = &field.pattern {
                if pattern.len() > 1024 || regex::Regex::new(pattern).is_err() { return Err("插件设置校验规则无效".into()); }
            }
            if field.normalize(&field.default)? != field.default { return Err("插件设置默认值不是规范值".into()); }
        }
        Ok(())
    }
}

pub(in super::super) fn parse(host: &super::super::Host, value: &Value) -> Result<BTreeMap<String, Schema>, String> {
    let providers = value.as_array().ok_or("插件握手必须返回提供商数组")?;
    let mut schemas = BTreeMap::new();
    for provider in providers {
        let Some(value) = provider.get("settings_schema").filter(|value| !value.is_null()) else {
            if provider.get("settings_group").is_some_and(|value| !value.is_null()) {
                return Err("提供商设置分组必须同时声明设置项".into());
            }
            continue;
        };
        let id = provider["id"].as_str().filter(|id| identifier(id)).ok_or("插件设置提供商标识无效")?;
        if !host.callbacks.provider_allowed(id) { continue; }
        if host.options.script { return Err("脚本插件不能通过原生握手声明设置".into()); }
        if host.options.wasm.as_ref().is_some_and(|options| !options.allows("settings.schema.v1")) {
            return Err("Wasm 插件未声明 settings.schema.v1 能力".into());
        }
        if value.to_string().len() > 128 * 1024 { return Err("插件设置声明超过大小限制".into()); }
        let mut schema: Schema = serde_json::from_value(value.clone()).map_err(|_| "插件设置声明格式无效")?;
        schema.validate()?;
        if let Some(group) = provider.get("settings_group").filter(|value| !value.is_null()) {
            let group: Group = serde_json::from_value(group.clone()).map_err(|_| "插件设置分组格式无效")?;
            if !text(&group.label, false) { return Err("插件设置分组名称无效".into()); }
            schema.group = Some(group);
        }
        if !host.callbacks.provider_allowed(schema.owner(id)) {
            return Err("插件设置只能使用本包获授权的提供商命名空间".into());
        }
        if schemas.insert(id.to_string(), schema).is_some() { return Err("插件设置提供商重复".into()); }
    }
    if host.options.wasm.as_ref().is_some_and(|options| options.allows("settings.schema.v1")) && schemas.is_empty() {
        return Err("Wasm 插件声明了设置能力，但没有提供设置说明".into());
    }
    let mut owners = BTreeMap::new();
    for (provider, schema) in &schemas {
        let encoded = serde_json::to_value((schema, &schema.group)).map_err(|_| "插件设置序列化失败")?;
        if owners.insert(schema.owner(provider), encoded.clone()).is_some_and(|old| old != encoded) {
            return Err("共用命名空间的插件设置与分组声明必须一致".into());
        }
    }
    Ok(schemas)
}
