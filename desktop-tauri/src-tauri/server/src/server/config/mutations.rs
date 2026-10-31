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
    let mut value = next.raw.get(key).cloned().unwrap_or(Value::Null);
    let before = value.clone();
    let result = mutate(&mut value)?;
    if value == before {
        return Ok(result);
    }
    let fingerprint = synced_fingerprint(&next.raw);
    next.raw.insert(key.to_string(), value);
    if !save_raw(&next.raw) {
        return Err(storage_error());
    }
    let changed = fingerprint != synced_fingerprint(&next.raw);
    let mut config = CONFIG
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *config = Some(next);
    drop(config);
    if changed {
        crate::server::core::cloud_sync::auto_sync::notify_change();
    }
    Ok(result)
}
