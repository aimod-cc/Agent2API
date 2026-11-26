//! 指纹脱敏配置：`GET/PUT /api/sanitize`。
//! 允许部分更新；规则 null = 恢复内置词表，规则 [] = 空词表，提供商 null = 恢复默认（WorkBuddy 国内版与国际版），提供商 [] = 全部。

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config::{
    self, SanitizePatch, KEY_SANITIZE_FINGERPRINTS, KEY_SANITIZE_PROVIDERS, KEY_SANITIZE_RULES,
    MAX_SANITIZE_PROVIDERS,
};
use crate::server::core::{custom_providers, providers, sanitize};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

pub async fn get_sanitize(State(_state): State<ServerState>) -> Response {
    ok_json(sanitize_json())
}

pub async fn put_sanitize(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let patch = match parse_patch(&payload) {
        Ok(patch) => patch,
        Err(message) => return errors::management_error(400, message),
    };
    let enabled = patch.enabled;
    let saved = config::set_sanitize_settings(patch);
    if !saved {
        logging::log("[Config]", "⚠️  出站关键词替换设置写入失败，本次运行内仍生效");
    }
    logging::log(
        "[Config]",
        match enabled {
            Some(true) => "出站关键词替换已开启",
            Some(false) => "出站关键词替换已关闭：请求内容将不再按替换规则改写",
            None => "出站关键词替换规则 / 提供商范围已更新",
        },
    );
    let mut result = sanitize_json();
    result["saved"] = Value::Bool(saved);
    ok_json(result)
}

fn parse_patch(payload: &Value) -> Result<SanitizePatch, String> {
    let object = payload.as_object().ok_or("请求体必须是 JSON 对象")?;
    if ![
        KEY_SANITIZE_FINGERPRINTS,
        KEY_SANITIZE_RULES,
        KEY_SANITIZE_PROVIDERS,
    ]
    .iter()
    .any(|key| object.contains_key(*key))
    {
        return Err("请提供关键词替换开关、规则列表或应用提供商".to_string());
    }
    let enabled = object
        .get(KEY_SANITIZE_FINGERPRINTS)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("{KEY_SANITIZE_FINGERPRINTS} 必须是 true 或 false"))
        })
        .transpose()?;
    let rules = match object.get(KEY_SANITIZE_RULES) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => {
            let entries = serde_json::from_value::<Vec<sanitize::SanitizeRule>>(value.clone())
                .map_err(|error| format!("替换规则必须是合法的规则数组：{error}"))?;
            Some(Some(Arc::new(sanitize::RuleSet::compile(entries)?)))
        }
    };
    let selected = match object.get(KEY_SANITIZE_PROVIDERS) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => Some(Some(parse_providers(value)?)),
    };
    Ok(SanitizePatch {
        enabled,
        rules,
        providers: selected,
    })
}

fn parse_providers(value: &Value) -> Result<Vec<String>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{KEY_SANITIZE_PROVIDERS} 必须是字符串数组"))?;
    if items.len() > MAX_SANITIZE_PROVIDERS {
        return Err(format!("应用提供商最多 {MAX_SANITIZE_PROVIDERS} 个"));
    }
    let previous = config::current().sanitize_settings().providers.clone();
    let mut selected = Vec::with_capacity(items.len());
    for item in items {
        let id = item
            .as_str()
            .ok_or("应用提供商的元素必须是字符串")?
            .trim()
            .to_ascii_lowercase();
        if id.is_empty() {
            return Err("应用提供商 id 不能为空".to_string());
        }
        // 插件暂时卸载时保留已选范围，不能因为保存词表而扩大成全部提供商。
        if !providers::is_known_provider_id(&id)
            && !custom_providers::is_custom_provider_id(&id)
            && !previous.contains(&id)
        {
            return Err(format!("未知的提供商：{id}"));
        }
        selected.push(id);
    }
    selected.sort();
    selected.dedup();
    Ok(selected)
}

fn sanitize_json() -> Value {
    let snapshot = config::current();
    let settings = snapshot.sanitize_settings();
    let mut options = providers::summary_json(|_| 0, &[]);
    options.extend(custom_providers::summary_json(|_| 0));
    for id in &settings.providers {
        if !options
            .iter()
            .any(|item| item.get("id").and_then(Value::as_str) == Some(id))
        {
            options.push(json!({"id": id, "label": providers::label_of(id)}));
        }
    }
    json!({
        KEY_SANITIZE_FINGERPRINTS: settings.enabled,
        KEY_SANITIZE_RULES: settings.rules.entries(),
        KEY_SANITIZE_PROVIDERS: settings.providers,
        "defaultProviders": config::default_sanitize_providers(),
        "defaultRules": sanitize::default_rules().entries(),
        "customRules": settings.custom_rules,
        "providerOptions": options,
        "roleOptions": sanitize::SanitizeRole::ALL,
        "limits": {
            "maxRules": sanitize::MAX_RULES,
            "maxNameChars": sanitize::MAX_NAME_CHARS,
            "maxPatternChars": sanitize::MAX_PATTERN_CHARS,
            "maxReplacementChars": sanitize::MAX_REPLACEMENT_CHARS,
        },
    })
}
