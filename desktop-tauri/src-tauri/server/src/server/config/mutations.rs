use serde_json::Value;

use super::{current, save_raw, synced_fingerprint, CONFIG, CONFIG_UPDATE_LOCK};

pub(crate) fn try_update_raw_field<T, E>(
    key: &str,
    mutate: impl FnOnce(&mut Value) -> Result<T, E>,
    storage_error: impl FnOnce() -> E,
) -> Result<T, E> {
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
    if key == crate::server::core::model_rules::KEY_MODEL_RULES {
        next.model_rules = std::sync::Arc::new(crate::server::core::model_rules::ModelRules::from_raw(&next.raw));
    }
    if key == super::KEY_ROUTING_PROFILES {
        next.routing_profiles = super::routing_profiles::from_raw(&next.raw);
    }
    if key == super::compact_routes::KEY_COMPACT_ROUTES {
        next.compact_routes = super::compact_routes::from_raw(&next.raw);
    }
    if key == super::client_identification::KEY_CLIENT_IDENTIFICATION {
        next.client_identification = super::client_identification::from_raw(&next.raw);
    }
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
