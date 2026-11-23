use serde_json::Value;

use super::{model_key_source, plugin_task_grants, save_raw, synced_fingerprint};

pub(crate) fn try_update_raw_field<T, E>(
    key: &str,
    mutate: impl FnOnce(&mut Value) -> Result<T, E>,
    storage_error: impl FnOnce() -> E,
) -> Result<T, E> {
    let usage = key == crate::server::core::custom_providers::KEY_CUSTOM_PROVIDERS;
    try_update_raw_fields_inner(usage, |raw| {
        let mut value = raw.get(key).cloned().unwrap_or(Value::Null);
        let before = value.clone();
        let result = mutate(&mut value)?;
        if value != before { raw.insert(key.to_string(), value); }
        Ok(result)
    }, storage_error)
}

pub(crate) fn try_update_raw_fields<T, E>(
    mutate: impl FnOnce(&mut serde_json::Map<String, Value>) -> Result<T, E>,
    storage_error: impl FnOnce() -> E,
) -> Result<T, E> {
    try_update_raw_fields_inner(true, mutate, storage_error)
}

fn try_update_raw_fields_inner<T, E>(
    usage: bool,
    mutate: impl FnOnce(&mut serde_json::Map<String, Value>) -> Result<T, E>,
    storage_error: impl FnOnce() -> E,
) -> Result<T, E> {
    let _usage_guard = usage.then(crate::server::core::usage_query::config::lock);
    let _guard = plugin_task_grants::update_lock();
    let Some(mut next) = super::write_snapshot_or_recover() else {
        return Err(storage_error());
    };
    let previous = next.clone();
    let result = mutate(&mut next.raw)?;
    if next.raw == previous.raw {
        return Ok(result);
    }
    let fingerprint = synced_fingerprint(&previous.raw);
    model_key_source::before_change(&previous, &next);
    plugin_task_grants::before_change(&previous.raw, &next.raw);
    if next.raw.get(crate::server::core::model_rules::KEY_MODEL_RULES)
        != previous.raw.get(crate::server::core::model_rules::KEY_MODEL_RULES) {
        next.model_rules = std::sync::Arc::new(crate::server::core::model_rules::ModelRules::from_raw(&next.raw));
    }
    if next.raw.get(super::error_policy::KEY_ERROR_POLICY) != previous.raw.get(super::error_policy::KEY_ERROR_POLICY) {
        next.error_policy = super::error_policy::from_raw(&next.raw);
        next.retry = super::parse::retry_from(&next.raw);
        next.routing = super::parse::routing_from(&next.raw);
    }
    if next.raw.get(super::KEY_ROUTING_PROFILES) != previous.raw.get(super::KEY_ROUTING_PROFILES) {
        next.routing_profiles = super::routing_profiles::from_raw(&next.raw);
    }
    if next.raw.get(super::compact_routes::KEY_COMPACT_ROUTES) != previous.raw.get(super::compact_routes::KEY_COMPACT_ROUTES) {
        next.compact_routes = super::compact_routes::from_raw(&next.raw);
    }
    if next.raw.get(super::runtime_capacity::KEY_RUNTIME_CAPACITY) != previous.raw.get(super::runtime_capacity::KEY_RUNTIME_CAPACITY) {
        next.runtime_capacity = super::runtime_capacity::from_raw(&next.raw);
    }
    if next.raw.get(super::client_identification::KEY_CLIENT_IDENTIFICATION) != previous.raw.get(super::client_identification::KEY_CLIENT_IDENTIFICATION) {
        next.client_identification = super::client_identification::from_raw(&next.raw);
    }
    if next.raw.get(super::vision_compat::KEY_VISION_COMPAT) != previous.raw.get(super::vision_compat::KEY_VISION_COMPAT) {
        next.vision_compat = super::vision_compat::from_raw(&next.raw);
    }
    if [super::prompt_rules::KEY_PROMPT_RULES, super::KEY_PROMPT_MODE, super::KEY_PROMPT_FILE,
        super::KEY_PROMPT_TEXT, super::KEY_PROMPT_PROVIDERS, super::KEY_PROMPT_GATEWAY,
        super::KEY_PROMPT_GATEWAY_TEXT].iter().any(|key| next.raw.get(*key) != previous.raw.get(*key)) {
        next.prompt = super::prompt_from(&next.raw);
    }
    if !save_raw(&next.raw) {
        // 落库失败时内存仍维持 previous，必须立即用 previous 重建新代次，避免 before_change 撤销后长期停在 401。
        model_key_source::published(&previous, false);
        return Err(storage_error());
    }
    let changed = fingerprint != synced_fingerprint(&next.raw);
    let catalog_changed = crate::server::core::providers::catalog::config_changed(&previous, &next);
    if catalog_changed { crate::server::core::providers::catalog::invalidate(); }
    let key_snapshot = next.clone();
    if !plugin_task_grants::publish(next) {
        model_key_source::fail_closed();
        return Err(storage_error());
    }
    model_key_source::published(&key_snapshot, false);
    if catalog_changed { crate::server::core::providers::catalog::invalidate(); }
    if changed {
        crate::server::core::cloud_sync::auto_sync::notify_change();
    }
    Ok(result)
}
