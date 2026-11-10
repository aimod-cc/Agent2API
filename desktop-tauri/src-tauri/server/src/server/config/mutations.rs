use serde_json::Value;

use super::{current, save_raw, synced_fingerprint, CONFIG, CONFIG_UPDATE_LOCK};

/// 配置底稿更新后重建受影响的一项运行期快照
///
/// 派生快照必须与底稿在同一临界区内更新，否则保存接口读回旧配置与旧修订号，
/// 页面刷新回退，后续保存还会因修订号过期返回冲突
pub(super) fn refresh_derived(next: &mut super::RuntimeConfig, key: &str) {
    match key {
        crate::server::core::model_rules::KEY_MODEL_RULES =>
            next.model_rules = std::sync::Arc::new(crate::server::core::model_rules::ModelRules::from_raw(&next.raw)),
        super::KEY_ROUTING_PROFILES => next.routing_profiles = super::routing_profiles::from_raw(&next.raw),
        super::compact_routes::KEY_COMPACT_ROUTES => next.compact_routes = super::compact_routes::from_raw(&next.raw),
        super::title_routes::KEY_TITLE_ROUTES => next.title_routes = super::title_routes::from_raw(&next.raw),
        super::client_identification::KEY_CLIENT_IDENTIFICATION =>
            next.client_identification = super::client_identification::from_raw(&next.raw),
        super::vision_compat::KEY_VISION_COMPAT => next.vision_compat = super::vision_compat::from_raw(&next.raw),
        _ => {}
    }
}

pub(crate) fn try_update_raw_field<T, E>(
    key: &str,
    mutate: impl FnOnce(&mut Value) -> Result<T, E>,
    storage_error: impl FnOnce() -> E,
) -> Result<T, E> {
    let _usage_guard = (key == crate::server::core::custom_providers::KEY_CUSTOM_PROVIDERS)
        .then(crate::server::core::usage_query::config::lock);
    let _guard = CONFIG_UPDATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut next = current();
    let previous = next.clone();
    let mut value = next.raw.get(key).cloned().unwrap_or(Value::Null);
    let before = value.clone();
    let result = mutate(&mut value)?;
    if value == before {
        return Ok(result);
    }
    let fingerprint = synced_fingerprint(&next.raw);
    next.raw.insert(key.to_string(), value);
    refresh_derived(&mut next, key);
    if !save_raw(&next.raw) {
        return Err(storage_error());
    }
    let changed = fingerprint != synced_fingerprint(&next.raw);
    let catalog_changed = crate::server::core::providers::catalog::config_changed(&previous, &next);
    let mut config = CONFIG
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if catalog_changed { crate::server::core::providers::catalog::invalidate(); }
    *config = Some(next);
    drop(config);
    if catalog_changed { crate::server::core::providers::catalog::invalidate(); }
    if changed {
        crate::server::core::cloud_sync::auto_sync::notify_change();
    }
    Ok(result)
}
