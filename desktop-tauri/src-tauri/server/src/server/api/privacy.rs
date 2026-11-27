//! 隐私脱敏配置与实时预览：`GET/PUT /api/privacy`、`POST /api/privacy/test`。

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Map, Value};

use crate::server::config::{self, PrivacySaveError};
use crate::server::core::privacy::{
    self, CompiledPrivacySettings, PrivacyAction, PrivacyCustomRule, PrivacyProviderMode,
    PrivacySettings, PrivacyVault,
};
use crate::server::core::{custom_providers, providers};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

pub async fn get_privacy(State(_state): State<ServerState>) -> Response {
    let snapshot = config::current();
    let compiled = snapshot.privacy_settings();
    let revision = snapshot.privacy_revision();
    ok_json(privacy_json(&compiled, revision, true))
}

pub async fn put_privacy(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(mut object) = payload.as_object().cloned() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let expected_revision = match object.remove("revision") {
        None | Some(Value::Null) => None,
        Some(Value::String(rev)) => {
            if rev.len() != 64 || !rev.bytes().all(|b| b.is_ascii_hexdigit()) {
                return errors::management_error(
                    400,
                    "隐私脱敏设置版本指纹无效，请刷新后再保存",
                );
            }
            Some(rev)
        }
        Some(_) => {
            return errors::management_error(400, "revision 必须是字符串");
        }
    };
    if object.is_empty() {
        return errors::management_error(400, "请提供至少一项需要修改的隐私脱敏设置");
    }

    let current = config::current().privacy_settings();
    let was_enabled = current.settings.enabled;
    let next_settings = match merge_settings(&current.settings, &object, true) {
        Ok(settings) => settings,
        Err(message) => return errors::management_error(400, message),
    };

    let (compiled, revision) =
        match config::save_privacy_settings(next_settings, expected_revision.as_deref()) {
            Ok(res) => res,
            Err(PrivacySaveError::Invalid(message)) => {
                return errors::management_error(400, message);
            }
            Err(PrivacySaveError::Conflict) => {
                return errors::management_error(
                    409,
                    "本地隐私脱敏设置已在其他位置修改，请刷新后再保存；当前草稿未丢失",
                );
            }
            Err(PrivacySaveError::Storage) => {
                logging::log("[Config]", "⚠️  隐私脱敏设置写入数据库失败");
                return errors::management_error(
                    500,
                    "隐私脱敏设置未能持久化，请检查配置目录",
                );
            }
        };

    if compiled.settings.enabled != was_enabled {
        logging::log(
            "[Config]",
            if compiled.settings.enabled {
                "隐私脱敏已开启：敏感信息在离开本机前将被自动保护"
            } else {
                "隐私脱敏已关闭"
            },
        );
    } else {
        logging::log("[Config]", "隐私脱敏设置已更新");
    }

    ok_json(privacy_json(&compiled, &revision, true))
}

pub async fn test_privacy(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let Some(text) = object.get("text").and_then(Value::as_str) else {
        return errors::management_error(400, "请提供要测试的文本");
    };
    if text.len() > privacy::MAX_TEST_TEXT_BYTES {
        return errors::management_error(
            400,
            format!("测试文本最多 {} 字节", privacy::MAX_TEST_TEXT_BYTES),
        );
    }

    let current = config::current().privacy_settings();
    let compiled = match object.get("config") {
        None | Some(Value::Null) => {
            let mut preview_settings = current.settings.clone();
            preview_settings.enabled = true;
            match CompiledPrivacySettings::compile(preview_settings) {
                Ok(compiled) => compiled,
                Err(message) => return errors::management_error(400, message),
            }
        }
        Some(cfg) => {
            let Some(cfg_obj) = cfg.as_object() else {
                return errors::management_error(400, "config 必须是 JSON 对象");
            };
            let mut merged = match merge_settings(&current.settings, cfg_obj, false) {
                Ok(settings) => settings,
                Err(message) => return errors::management_error(400, message),
            };
            merged.enabled = true;
            match CompiledPrivacySettings::compile(merged) {
                Ok(compiled) => compiled,
                Err(message) => return errors::management_error(400, message),
            }
        }
    };

    ok_json(privacy::preview_text(text, &compiled))
}

fn merge_settings(
    base: &PrivacySettings,
    patch: &Map<String, Value>,
    validate_providers: bool,
) -> Result<PrivacySettings, String> {
    let mut next = base.clone();

    if let Some(val) = patch.get("enabled") {
        next.enabled = val
            .as_bool()
            .ok_or_else(|| "enabled 必须是 true 或 false".to_string())?;
    }
    if let Some(val) = patch.get("reversible") {
        next.reversible = val
            .as_bool()
            .ok_or_else(|| "reversible 必须是 true 或 false".to_string())?;
    }
    if let Some(val) = patch.get("ephemeralPlaceholders") {
        next.ephemeral_placeholders = val
            .as_bool()
            .ok_or_else(|| "ephemeralPlaceholders 必须是 true 或 false".to_string())?;
    }
    if let Some(val) = patch.get("action") {
        next.action = serde_json::from_value::<PrivacyAction>(val.clone())
            .map_err(|_| "action 必须是 mask、warn 或 block".to_string())?;
    }
    if let Some(val) = patch.get("secrets") {
        next.secrets = val
            .as_bool()
            .ok_or_else(|| "secrets 必须是 true 或 false".to_string())?;
    }
    if let Some(val) = patch.get("personal") {
        next.personal = val
            .as_bool()
            .ok_or_else(|| "personal 必须是 true 或 false".to_string())?;
    }
    if let Some(val) = patch.get("categories") {
        let map = serde_json::from_value::<BTreeMap<String, bool>>(val.clone())
            .map_err(|err| format!("categories 格式不正确：{err}"))?;
        for (k, v) in map {
            if next.categories.contains_key(&k) {
                next.categories.insert(k, v);
            }
        }
    }
    if let Some(val) = patch.get("kindActions") {
        if val.is_null() {
            next.kind_actions.clear();
        } else {
            next.kind_actions =
                serde_json::from_value::<BTreeMap<String, PrivacyAction>>(val.clone())
                    .map_err(|err| format!("kindActions 格式不正确：{err}"))?;
        }
    }
    if let Some(val) = patch.get("words") {
        next.words = serde_json::from_value::<Vec<String>>(val.clone())
            .map_err(|err| format!("保密词列表必须是字符串数组：{err}"))?;
    }
    if let Some(val) = patch.get("wordsIgnoreCase") {
        next.words_ignore_case = val
            .as_bool()
            .ok_or_else(|| "wordsIgnoreCase 必须是 true 或 false".to_string())?;
    }
    if let Some(val) = patch.get("wordsWholeWord") {
        next.words_whole_word = val
            .as_bool()
            .ok_or_else(|| "wordsWholeWord 必须是 true 或 false".to_string())?;
    }
    if let Some(val) = patch.get("customRules") {
        next.custom_rules = serde_json::from_value::<Vec<PrivacyCustomRule>>(val.clone())
            .map_err(|err| format!("自定义识别规则格式不正确：{err}"))?;
    }
    if let Some(val) = patch.get("allowlist") {
        next.allowlist = serde_json::from_value::<Vec<String>>(val.clone())
            .map_err(|err| format!("白名单必须是字符串数组：{err}"))?;
    }
    if let Some(val) = patch.get("providerMode") {
        next.provider_mode = serde_json::from_value::<PrivacyProviderMode>(val.clone())
            .map_err(|_| "providerMode 必须是 exclude 或 include".to_string())?;
    }
    if let Some(val) = patch.get("providers") {
        next.providers = parse_providers(val, &base.providers, validate_providers)?;
    }

    Ok(next)
}

fn parse_providers(
    value: &Value,
    previous: &[String],
    validate_known: bool,
) -> Result<Vec<String>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let items = value
        .as_array()
        .ok_or_else(|| "providers 必须是字符串数组".to_string())?;
    if items.len() > privacy::MAX_PROVIDERS {
        return Err(format!("提供商列表最多 {} 个", privacy::MAX_PROVIDERS));
    }
    let mut selected = Vec::with_capacity(items.len());
    for item in items {
        let id = item
            .as_str()
            .ok_or_else(|| "提供商元素必须是字符串".to_string())?
            .trim()
            .to_ascii_lowercase();
        if id.is_empty() {
            return Err("提供商 id 不能为空".to_string());
        }
        if validate_known
            && !providers::is_known_provider_id(&id)
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

fn privacy_json(compiled: &CompiledPrivacySettings, revision: &str, saved: bool) -> Value {
    let settings = &compiled.settings;
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
        "enabled": settings.enabled,
        "reversible": settings.reversible,
        "ephemeralPlaceholders": settings.ephemeral_placeholders,
        "action": settings.action,
        "secrets": settings.secrets,
        "personal": settings.personal,
        "categories": settings.categories,
        "kindActions": settings.kind_actions,
        "words": settings.words,
        "wordsIgnoreCase": settings.words_ignore_case,
        "wordsWholeWord": settings.words_whole_word,
        "customRules": settings.custom_rules,
        "allowlist": settings.allowlist,
        "providerMode": settings.provider_mode,
        "providers": settings.providers,
        "providerOptions": options,
        "categoryOptions": privacy::category_catalog(),
        "vaultEntries": PrivacyVault::global().stats(),
        "revision": revision,
        "saved": saved,
        "limits": {
            "maxWords": privacy::MAX_WORDS,
            "maxWordChars": privacy::MAX_WORD_CHARS,
            "maxAllowlist": privacy::MAX_ALLOWLIST,
            "maxCustomRules": privacy::MAX_CUSTOM_RULES,
            "maxNameChars": privacy::MAX_NAME_CHARS,
            "maxKindChars": privacy::MAX_KIND_CHARS,
            "minPrefixChars": privacy::MIN_PREFIX_CHARS,
            "maxPrefixChars": privacy::MAX_PREFIX_CHARS,
            "maxRegexChars": privacy::MAX_REGEX_CHARS,
            "maxTestTextBytes": privacy::MAX_TEST_TEXT_BYTES,
        },
    })
}
