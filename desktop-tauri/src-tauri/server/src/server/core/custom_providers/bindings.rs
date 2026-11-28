//! 自定义提供商的对外绑定；原始 ID 和别名各自控制可见性及路由。

use serde_json::{json, Value};

use crate::server::core::capability;
use crate::server::core::model_meta;
use crate::server::core::models::model_id;

use std::collections::{HashMap, HashSet};

use crate::server::errors::GatewayError;

pub(super) fn normalize(models: &mut [Value], mappings: &mut Vec<Value>) {
    for model in models.iter_mut() {
        let id = model_id(model);
        if let Some(legacy) = mappings.iter().find(|mapping| {
            text(mapping, "alias").eq_ignore_ascii_case(&id)
                && text(mapping, "target").eq_ignore_ascii_case(&id)
        }) {
            model["enabled"] = Value::Bool(enabled(model) && enabled(legacy));
            if !text(legacy, "reasoning").is_empty() {
                model["reasoning"] = legacy["reasoning"].clone();
            }
        }
    }
    mappings.retain(|mapping| {
        !(text(mapping, "alias").eq_ignore_ascii_case(text(mapping, "target"))
            && models
                .iter()
                .any(|model| model_id(model).eq_ignore_ascii_case(text(mapping, "target"))))
    });
    let mut claimed = models
        .iter()
        .filter(|model| enabled(model))
        .map(|model| model_id(model).to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for carried in [true, false] {
        for mapping in mappings.iter_mut().filter(|mapping| {
            models
                .iter()
                .any(|model| model_id(model).eq_ignore_ascii_case(text(mapping, "target")))
                == carried
        }) {
            if enabled(mapping) && !claimed.insert(text(mapping, "alias").to_ascii_lowercase()) {
                mapping["enabled"] = Value::Bool(false);
            }
        }
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn enabled(value: &Value) -> bool {
    value
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn bindings(models: &[Value], mappings: &[Value]) -> Vec<(String, String, bool)> {
    models
        .iter()
        .map(|model| {
            let id = model_id(model);
            (
                id.to_ascii_lowercase(),
                id.to_ascii_lowercase(),
                enabled(model),
            )
        })
        .chain(mappings.iter().map(|mapping| {
            (
                text(mapping, "alias").to_ascii_lowercase(),
                text(mapping, "target").to_ascii_lowercase(),
                enabled(mapping),
            )
        }))
        .collect()
}

pub(super) fn validate_change(
    previous: &Value,
    models: &[Value],
    mappings: &[Value],
) -> Result<(), GatewayError> {
    let old_models = previous
        .get("models")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let old_mappings = previous
        .get("mappings")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let previous = bindings(old_models, old_mappings);
    let mut active = HashMap::new();
    for (alias, target, enabled) in bindings(models, mappings) {
        if !previous
            .iter()
            .any(|(old_alias, old_target, _)| old_alias == &alias && old_target == &target)
            && previous
                .iter()
                .any(|(old_alias, _, enabled)| old_alias == &alias && *enabled)
        {
            return Err(GatewayError::with_status(
                409,
                format!("模型 ID「{alias}」在该提供商已有启用绑定，请先关闭它再添加"),
            ));
        }
        if enabled && active.insert(alias.clone(), target).is_some() {
            return Err(GatewayError::with_status(
                409,
                format!("模型 ID「{alias}」不能同时启用多个绑定"),
            ));
        }
    }
    for mapping in mappings {
        if !models
            .iter()
            .any(|model| model_id(model).eq_ignore_ascii_case(text(mapping, "target")))
        {
            let previous = old_mappings.iter().find(|old| {
                text(old, "alias").eq_ignore_ascii_case(text(mapping, "alias"))
                    && text(old, "target").eq_ignore_ascii_case(text(mapping, "target"))
            });
            if previous.is_none()
                || (enabled(mapping) && previous.is_some_and(|previous| !enabled(previous)))
            {
                return Err(GatewayError::bad_request(format!(
                    "该提供商的清单里没有模型「{}」",
                    text(mapping, "target")
                )));
            }
        }
    }
    Ok(())
}

pub fn resolve(provider: &Value, requested: &str) -> Option<(String, Option<String>)> {
    if !provider
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let models = provider.get("models").and_then(Value::as_array)?;
    let mappings = provider
        .get("mappings")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let level = |entry: &Value| {
        entry
            .get("reasoning")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    if let Some(model) = models
        .iter()
        .find(|model| model_id(model).eq_ignore_ascii_case(requested))
    {
        let default = mappings.iter().find(|mapping| {
            mapping
                .get("alias")
                .and_then(Value::as_str)
                .is_some_and(|alias| alias.eq_ignore_ascii_case(requested))
                && mapping
                    .get("target")
                    .and_then(Value::as_str)
                    .is_some_and(|target| target.eq_ignore_ascii_case(requested))
        });
        if model
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            && default.map_or(true, |mapping| {
                mapping
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            })
        {
            return Some((
                model_id(model),
                default.and_then(level).or_else(|| level(model)),
            ));
        }
    }
    mappings.iter().find_map(|mapping| {
        let alias = mapping.get("alias")?.as_str()?;
        let target = mapping.get("target")?.as_str()?;
        if !alias.eq_ignore_ascii_case(requested)
            || alias.eq_ignore_ascii_case(target)
            || !mapping
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        {
            return None;
        }
        // target 的 enabled 只控制原始 ID，不能阻止别名调用它。
        let model = models
            .iter()
            .find(|model| model_id(model).eq_ignore_ascii_case(target))?;
        Some((model_id(model), level(mapping)))
    })
}

/// 把一条自定义提供商模型条目补齐 `models.dev` 元数据并叠加用户能力覆盖。
///
/// 优先级与内置提供商一致：**用户覆盖 > models.dev > 未声明**。
/// 顺序必须为：先对上游真名运行 `model_meta::apply` 补齐能力、来源与参考思考档位，
/// 再叠加用户保存的稀疏 `capabilities` 覆盖并标记为 `"user"`。
/// 返回 `(enriched_item, overridden_keys)`：
/// - `enriched_item` 顶层平铺生效能力位、`capSources`、`reasoningLevels` 等字段；
/// - `overridden_keys` 是用户手动覆盖过的能力键列表（与内置家 `capOverrides` 同形）。
pub fn enrich_model(model_entry: &Value) -> (Value, Vec<String>) {
    let target_id = model_id(model_entry);
    let user_overrides = capability::normalize_object(model_entry.get("capabilities"));
    let overridden = capability::overridden_keys(&user_overrides);
    let mut item = json!({ "id": target_id });
    model_meta::apply(&mut item);
    if !user_overrides.is_empty() {
        capability::apply_overrides(&mut item, &user_overrides);
        model_meta::mark_user(&mut item, &overridden);
    }
    if user_overrides
        .get("supportsReasoning")
        .and_then(Value::as_bool)
        == Some(false)
    {
        if let Some(object) = item.as_object_mut() {
            object.insert("reasoningLevels".to_string(), json!([]));
            object.remove("reasoningDefaultLevel");
            object.remove("reasoningFixedLevel");
        }
    }
    (item, overridden)
}

/// 该家对外的模型条目（`{id, …能力位}`）—— 清单出口与 `/v1/models` 共用。
///
/// 条目的 id 是**对外名**（原始 ID 或某个存活的别名），能力位与 `models.dev` 元数据
/// 均按 **target 的上游真名与登记条目**解析：别名先复用 target 的识别与覆盖结果，
/// 再把 `id` 换成对外名，避免别名绕过 `models.dev` 识别或与原始 ID 声明分叉。
pub fn public_models(provider: &Value) -> Vec<Value> {
    let models = provider
        .get("models")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut names = Vec::new();
    names.extend(models.iter().map(model_id));
    if let Some(mappings) = provider.get("mappings").and_then(Value::as_array) {
        names.extend(mappings.iter().filter_map(|mapping| {
            mapping
                .get("alias")
                .and_then(Value::as_str)
                .map(str::to_string)
        }));
    }
    let mut result = Vec::new();
    for name in names {
        if name.is_empty()
            || result
                .iter()
                .any(|model| model_id(model).eq_ignore_ascii_case(&name))
        {
            continue;
        }
        let Some((target, _)) = resolve(provider, &name) else {
            continue;
        };
        let fallback_entry = json!({ "id": target });
        let target_entry = models
            .iter()
            .find(|model| model_id(model).eq_ignore_ascii_case(&target))
            .unwrap_or(&fallback_entry);
        let (mut item, _) = enrich_model(target_entry);
        if let Some(object) = item.as_object_mut() {
            object.insert("id".to_string(), Value::String(name));
        }
        result.push(item);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrich_model_fills_modelsdev_and_respects_user_overrides() {
        let entry = json!({
            "id": "deepseek-v4.1-flash",
            "enabled": true,
            "reasoning": ""
        });
        let (enriched, overrides) = enrich_model(&entry);
        assert!(overrides.is_empty());
        assert_eq!(enriched["maxInputTokens"], json!(1_000_000));
        assert_eq!(enriched["maxOutputTokens"], json!(384_000));
        assert_eq!(enriched["supportsToolCall"], json!(true));
        assert_eq!(enriched["supportsImages"], json!(true));
        assert_eq!(enriched["supportsVideo"], json!(false));
        assert_eq!(enriched["supportsReasoning"], json!(true));
        assert_eq!(enriched["capSources"]["maxInputTokens"], json!("modelsdev"));
        assert_eq!(enriched["capSources"]["supportsToolCall"], json!("modelsdev"));
        assert_eq!(enriched["reasoningLevels"], json!(["low", "high", "max"]));
        assert_eq!(enriched["reasoningSource"], json!("modelsdev"));

        let overridden_entry = json!({
            "id": "deepseek-v4.1-flash",
            "capabilities": {
                "maxInputTokens": 128_000,
                "supportsReasoning": false
            }
        });
        let (enriched, overrides) = enrich_model(&overridden_entry);
        assert_eq!(overrides, vec!["maxInputTokens", "supportsReasoning"]);
        assert_eq!(enriched["maxInputTokens"], json!(128_000));
        assert_eq!(enriched["maxOutputTokens"], json!(384_000));
        assert_eq!(enriched["supportsReasoning"], json!(false));
        assert_eq!(enriched["capSources"]["maxInputTokens"], json!("user"));
        assert_eq!(enriched["capSources"]["maxOutputTokens"], json!("modelsdev"));
        assert_eq!(enriched["capSources"]["supportsReasoning"], json!("user"));
        assert_eq!(enriched["reasoningLevels"], json!([]));
    }

    #[test]
    fn public_models_alias_inherits_target_modelsdev_and_overrides() {
        let provider = json!({
            "id": "custom-test12345678",
            "enabled": true,
            "models": [
                {
                    "id": "deepseek-v4.1-flash",
                    "enabled": true,
                    "reasoning": "",
                    "capabilities": { "maxOutputTokens": 65_536 }
                }
            ],
            "mappings": [
                {
                    "alias": "my-fast-alias",
                    "target": "deepseek-v4.1-flash",
                    "enabled": true,
                    "reasoning": ""
                }
            ]
        });
        let items = public_models(&provider);
        let alias_item = items
            .iter()
            .find(|item| model_id(item) == "my-fast-alias")
            .expect("alias item");
        assert_eq!(alias_item["maxInputTokens"], json!(1_000_000));
        assert_eq!(alias_item["maxOutputTokens"], json!(65_536));
        assert_eq!(alias_item["supportsToolCall"], json!(true));
        assert_eq!(alias_item["capSources"]["maxInputTokens"], json!("modelsdev"));
        assert_eq!(alias_item["capSources"]["maxOutputTokens"], json!("user"));
        assert_eq!(alias_item["reasoningLevels"], json!(["low", "high", "max"]));
    }
}
