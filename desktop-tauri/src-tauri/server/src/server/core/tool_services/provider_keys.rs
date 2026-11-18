use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::search::{SearchError, SearchErrorCode};
use super::ProviderConfig;
use crate::server::core::crypto::{self, SecretScope};

pub const MAX_PROVIDER_KEYS: usize = 100;
const MAX_API_KEY_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyLimit {
    pub code: SearchErrorCode,
    pub message: String,
    pub reset_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderKeyConfig {
    pub id: String,
    pub encrypted_key: String,
    pub key_hint: String,
    pub updated_at: i64,
    pub limits: BTreeMap<String, KeyLimit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderKeyUpdate {
    pub id: Option<String>,
    pub api_key: Option<String>,
}

pub(super) fn normalize(entry: &mut ProviderConfig) {
    if entry.keys.is_empty() && !entry.encrypted_key.is_empty() {
        entry.keys.push(ProviderKeyConfig {
            id: stored_id(&entry.encrypted_key),
            encrypted_key: std::mem::take(&mut entry.encrypted_key),
            key_hint: std::mem::take(&mut entry.key_hint),
            updated_at: entry.updated_at,
            limits: BTreeMap::new(),
        });
    }
    entry.encrypted_key.clear();
    entry.key_hint.clear();
    let mut seen = BTreeSet::new();
    entry.keys.retain_mut(|key| {
        if key.encrypted_key.is_empty() {
            return false;
        }
        if key.id.is_empty() {
            key.id = stored_id(&key.encrypted_key);
        }
        seen.insert(key.id.clone())
    });
}

fn stored_id(encrypted_key: &str) -> String {
    let digest = Sha256::digest(encrypted_key.as_bytes());
    format!("key-{:x}", digest)[..28].to_string()
}

fn seal_key(value: &str) -> Result<ProviderKeyConfig, String> {
    if value.is_empty() {
        return Err("新密钥不能为空".into());
    }
    if value.len() > MAX_API_KEY_BYTES || value.chars().any(char::is_control) {
        return Err("API Key 格式无效".into());
    }
    let encrypted_key = crypto::seal(value.as_bytes(), SecretScope::Accounts)
        .map_err(|_| "无法加密 API Key".to_string())?;
    Ok(ProviderKeyConfig {
        id: stored_id(&encrypted_key),
        encrypted_key,
        key_hint: super::key_hint(value),
        updated_at: crate::server::logging::now_ms(),
        limits: BTreeMap::new(),
    })
}

pub(super) struct PreparedKeyUpdate {
    id: Option<String>,
    replacement: Option<ProviderKeyConfig>,
}

pub(super) fn prepare_updates(
    items: &[ProviderKeyUpdate],
) -> Result<Vec<PreparedKeyUpdate>, String> {
    if items.len() > MAX_PROVIDER_KEYS {
        return Err("每家提供商最多保存 100 个密钥".into());
    }
    let mut ids = BTreeSet::new();
    let mut secrets = BTreeSet::new();
    items
        .iter()
        .map(|item| {
            let id = item
                .id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(id) = id {
                if !ids.insert(id.to_string()) {
                    return Err("密钥列表包含重复项".into());
                }
            }
            let value = item
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(value) = value {
                if !secrets.insert(value) {
                    return Err("密钥列表包含重复项".into());
                }
            }
            if id.is_none() && value.is_none() {
                return Err("新密钥不能为空".into());
            }
            Ok(PreparedKeyUpdate {
                id: id.map(str::to_string),
                replacement: value.map(seal_key).transpose()?,
            })
        })
        .collect()
}

pub(super) fn apply_updates(
    entry: &mut ProviderConfig,
    items: Vec<PreparedKeyUpdate>,
) -> Result<(), String> {
    let mut keys = Vec::with_capacity(items.len());
    let mut secrets = BTreeSet::new();
    for item in items {
        let existing = match item.id {
            Some(id) => Some(
                entry
                    .keys
                    .iter()
                    .find(|key| key.id == id)
                    .ok_or_else(|| "密钥列表已变更，请刷新后重试".to_string())?,
            ),
            None => None,
        };
        let key = item
            .replacement
            .or_else(|| existing.cloned())
            .ok_or_else(|| "新密钥不能为空".to_string())?;
        if let Ok(bytes) = crypto::open(&key.encrypted_key, SecretScope::Accounts) {
            let bytes = zeroize::Zeroizing::new(bytes);
            let fingerprint = Sha256::digest(bytes.as_slice()).to_vec();
            if !secrets.insert(fingerprint) {
                return Err("密钥列表包含重复项".into());
            }
        }
        keys.push(key);
    }
    entry.keys = keys;
    entry.updated_at = crate::server::logging::now_ms();
    Ok(())
}

pub(super) fn replace_legacy_key(entry: &mut ProviderConfig, key: ProviderKeyConfig) {
    if entry.keys.is_empty() {
        entry.keys.push(key);
    } else {
        entry.keys[0] = key;
    }
    entry.updated_at = crate::server::logging::now_ms();
}

pub(super) fn prepare_legacy_key(value: &str) -> Result<ProviderKeyConfig, String> {
    seal_key(value)
}

pub(super) fn has_keys(entry: &ProviderConfig) -> bool {
    entry.keys.iter().any(|key| !key.encrypted_key.is_empty())
}

fn active_limit<'a>(
    key: &'a ProviderKeyConfig,
    operation: &str,
    now: i64,
) -> Option<(&'a str, &'a KeyLimit)> {
    ["all", operation]
        .into_iter()
        .filter_map(|scope| key.limits.get_key_value(scope))
        .filter(|(_, limit)| limit.reset_at.is_none_or(|time| time > now))
        .max_by_key(|(_, limit)| match limit.code {
            SearchErrorCode::InvalidProviderKey => 3,
            SearchErrorCode::QuotaExhausted => 2,
            _ => 1,
        })
        .map(|(scope, limit)| (scope.as_str(), limit))
}

pub(super) fn usable(entry: &ProviderConfig, operation: &str) -> bool {
    let now = crate::server::logging::now_ms();
    entry
        .keys
        .iter()
        .any(|key| active_limit(key, operation, now).is_none())
}

fn status_json(key: &ProviderKeyConfig, operation: &str, now: i64) -> Value {
    match active_limit(key, operation, now) {
        Some((scope, limit)) => json!({
            "status": limit.code.as_str(), "scope": scope,
            "reset_at": limit.reset_at, "message": limit.message,
        }),
        None => {
            json!({ "status": "ready", "scope": Value::Null, "reset_at": Value::Null, "message": Value::Null })
        }
    }
}

pub(super) fn provider_row(provider: &str, entry: &ProviderConfig) -> Value {
    let now = crate::server::logging::now_ms();
    let ready = |operation: &'static str| {
        entry
            .keys
            .iter()
            .filter(move |key| active_limit(key, operation, now).is_none())
    };
    let search_key = ready("search").next();
    let fetch_key = ready("fetch").next();
    let key_hint = search_key
        .or_else(|| entry.keys.first())
        .map(|key| key.key_hint.as_str());
    json!({
        "provider": provider, "display_name": super::provider_display_name(provider),
        "auth_type": "api_key", "supports_search": true, "supports_fetch": true,
        "available": true, "configured": has_keys(entry), "enabled": entry.enabled,
        "usable": search_key.is_some(), "fetch_usable": fetch_key.is_some(),
        "key_hint": key_hint, "updated_at": (entry.updated_at > 0).then_some(entry.updated_at),
        "active_key_id": search_key.filter(|_| entry.enabled).map(|key| &key.id),
        "active_fetch_key_id": fetch_key.filter(|_| entry.enabled).map(|key| &key.id),
        "available_key_count": ready("search").count(), "available_fetch_key_count": ready("fetch").count(),
        "max_keys": MAX_PROVIDER_KEYS,
        "keys": entry.keys.iter().enumerate().map(|(index, key)| json!({
            "id": key.id, "priority": index + 1, "key_hint": key.key_hint,
            "updated_at": (key.updated_at > 0).then_some(key.updated_at),
            "search": status_json(key, "search", now), "fetch": status_json(key, "fetch", now),
        })).collect::<Vec<_>>(),
    })
}

pub(super) struct KeyAttempt {
    pub key: ProviderKeyConfig,
}

impl KeyAttempt {
    pub fn decrypt(&self) -> Result<zeroize::Zeroizing<String>, SearchError> {
        let bytes = crypto::open(&self.key.encrypted_key, SecretScope::Accounts).map_err(|_| {
            SearchError::new(
                SearchErrorCode::InvalidProviderKey,
                "The saved provider API key cannot be decrypted; save it again",
            )
        })?;
        String::from_utf8(bytes)
            .ok()
            .filter(|key| !key.trim().is_empty())
            .map(zeroize::Zeroizing::new)
            .ok_or_else(|| {
                SearchError::new(
                    SearchErrorCode::InvalidProviderKey,
                    "The saved provider API key is invalid",
                )
            })
    }
}

pub(super) fn next_key(
    provider: &str,
    operation: &str,
    attempted: &BTreeSet<String>,
    forced_id: Option<&str>,
    allow_disabled: bool,
) -> Result<KeyAttempt, SearchError> {
    let config = super::load();
    let entry = config
        .providers
        .get(provider)
        .filter(|entry| entry.enabled || allow_disabled)
        .ok_or_else(|| {
            SearchError::new(
                SearchErrorCode::Configuration,
                "The search provider is disabled",
            )
        })?;
    if !has_keys(entry) {
        return Err(SearchError::new(
            SearchErrorCode::Configuration,
            "Provider is not configured with an API key",
        ));
    }
    let now = crate::server::logging::now_ms();
    if let Some(id) = forced_id {
        let key = entry
            .keys
            .iter()
            .find(|key| key.id == id && !attempted.contains(&key.id))
            .ok_or_else(|| {
                SearchError::new(
                    SearchErrorCode::Configuration,
                    "The selected provider API key no longer exists",
                )
            })?;
        return Ok(KeyAttempt { key: key.clone() });
    }
    if let Some(key) = entry
        .keys
        .iter()
        .find(|key| !attempted.contains(&key.id) && active_limit(key, operation, now).is_none())
    {
        return Ok(KeyAttempt { key: key.clone() });
    }
    let limits: Vec<&KeyLimit> = entry
        .keys
        .iter()
        .filter_map(|key| active_limit(key, operation, now).map(|(_, limit)| limit))
        .collect();
    let code = limits
        .first()
        .map(|limit| limit.code)
        .unwrap_or(SearchErrorCode::ProviderUnavailable);
    let all_same = limits.iter().all(|limit| limit.code == code);
    let message = match code {
        _ if !all_same => "No provider API key is available; check quota and key status",
        SearchErrorCode::QuotaExhausted => {
            "All configured provider API keys have exhausted their quota"
        }
        SearchErrorCode::RateLimited => {
            "All configured provider API keys are temporarily rate limited"
        }
        SearchErrorCode::InvalidProviderKey => "All configured provider API keys are invalid",
        _ => "No provider API key is available for this request",
    };
    let mut error = SearchError::new(code, message);
    error.reset_at = limits.iter().filter_map(|limit| limit.reset_at).min();
    Err(error)
}

pub(super) fn record_failure(
    provider: &str,
    operation: &str,
    attempt: &KeyAttempt,
    error: &SearchError,
) -> Result<(), SearchError> {
    let scope = match error.code {
        SearchErrorCode::InvalidProviderKey => "all",
        SearchErrorCode::QuotaExhausted if matches!(provider, "tavily" | "firecrawl") => "all",
        SearchErrorCode::QuotaExhausted | SearchErrorCode::RateLimited => operation,
        _ => return Ok(()),
    };
    super::update(|config| {
        let Some(key) = config.providers.get_mut(provider).and_then(|entry| {
            entry.keys.iter_mut().find(|key| {
                key.id == attempt.key.id && key.encrypted_key == attempt.key.encrypted_key
            })
        }) else {
            return;
        };
        let now = crate::server::logging::now_ms();
        let reset_at = error.reset_at.or_else(|| {
            key.limits
                .get(scope)
                .filter(|limit| limit.code == error.code)
                .and_then(|limit| limit.reset_at)
                .filter(|time| *time > now)
        });
        key.limits.insert(
            scope.to_string(),
            KeyLimit {
                code: error.code,
                message: error.message.clone(),
                reset_at,
                updated_at: now,
            },
        );
    })
    .map(|_| ())
    .map_err(|_| {
        SearchError::new(
            SearchErrorCode::Configuration,
            "Unable to save the provider API key status",
        )
    })
}

pub(super) fn record_success(
    provider: &str,
    operation: &str,
    attempt: &KeyAttempt,
) -> Result<(), SearchError> {
    if !["all", operation]
        .iter()
        .any(|scope| attempt.key.limits.contains_key(*scope))
    {
        return Ok(());
    }
    super::update(|config| {
        let Some(key) = config.providers.get_mut(provider).and_then(|entry| {
            entry.keys.iter_mut().find(|key| {
                key.id == attempt.key.id && key.encrypted_key == attempt.key.encrypted_key
            })
        }) else {
            return;
        };
        // 较早发出的成功请求不能清除并发请求刚发现的额度限制。
        for scope in ["all", operation] {
            if key
                .limits
                .get(scope)
                .is_some_and(|limit| Some(limit) == attempt.key.limits.get(scope))
            {
                key.limits.remove(scope);
            }
        }
    })
    .map(|_| ())
    .map_err(|_| {
        SearchError::new(
            SearchErrorCode::Configuration,
            "Unable to save the provider API key status",
        )
    })
}
