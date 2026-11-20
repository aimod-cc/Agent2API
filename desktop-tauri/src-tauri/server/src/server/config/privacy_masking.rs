use std::sync::Arc;

use serde_json::{Map, Value};

use crate::server::core::privacy_masking::{Rules, Settings};

use super::{try_update_raw_field, CONFIG};

pub(crate) const KEY: &str = "privacyMasking";
pub(crate) type Snapshot = Option<Arc<Result<Rules, &'static str>>>;

pub(crate) enum SaveError {
    Invalid(&'static str),
    Storage,
}

pub(super) fn from_raw(raw: &Map<String, Value>) -> Snapshot {
    raw.get(KEY).map(|value| {
        Arc::new(
            serde_json::from_value::<Settings>(value.clone())
                .map_err(|_| "privacyMasking 设置无效")
                .and_then(Rules::compile),
        )
    })
}

pub(crate) fn snapshot() -> Result<Snapshot, &'static str> {
    let config = CONFIG.read().map_err(|_| "隐私脱敏配置不可读")?;
    Ok(config.as_ref().and_then(|cfg| cfg.privacy_masking.clone()))
}

pub(crate) fn save(settings: Settings) -> Result<(), SaveError> {
    let rules = Rules::compile(settings).map_err(SaveError::Invalid)?;
    let value = serde_json::to_value(&rules.settings).map_err(|_| SaveError::Storage)?;
    try_update_raw_field(
        KEY,
        move |stored| {
            *stored = value;
            Ok(())
        },
        || SaveError::Storage,
    )
}

pub(super) fn refresh(next: &mut super::RuntimeConfig, previous: &super::RuntimeConfig) {
    if next.raw.get(KEY) != previous.raw.get(KEY) {
        next.privacy_masking = from_raw(&next.raw);
    }
}
