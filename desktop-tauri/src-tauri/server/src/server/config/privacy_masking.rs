use super::{try_update_raw_field, CONFIG};
use crate::server::core::privacy_masking::{Rules, Settings};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(crate) const KEY: &str = "privacyMasking";

#[derive(Debug)]
pub(crate) struct Policy {
    pub(crate) rules: Rules,
    pub(crate) revision: String,
}
pub(crate) type Snapshot = Option<Arc<Result<Policy, &'static str>>>;

pub(crate) enum SaveError {
    Invalid(&'static str),
    Conflict,
    Storage,
}

// Same content-revision convention as config::prompt_rules. The version is
// derived from the exact stored value, not a second independently read snapshot.
pub(crate) fn revision(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

pub(super) fn from_raw(raw: &Map<String, Value>) -> Snapshot {
    raw.get(KEY).map(|value| {
        Arc::new(
            serde_json::from_value::<Settings>(value.clone())
                .map_err(|_| "privacyMasking 设置无效")
                .and_then(Rules::compile)
                .map(|rules| Policy {
                    rules,
                    revision: revision(value),
                }),
        )
    })
}

pub(crate) fn snapshot() -> Result<Snapshot, &'static str> {
    let config = CONFIG.read().map_err(|_| "隐私脱敏配置不可读")?;
    Ok(config.as_ref().and_then(|cfg| cfg.privacy_masking.clone()))
}

pub(crate) fn save(settings: Settings, expected: &str) -> Result<(), SaveError> {
    let rules = Rules::compile(settings).map_err(SaveError::Invalid)?;
    let value = serde_json::to_value(&rules.settings).map_err(|_| SaveError::Storage)?;
    try_update_raw_field(
        KEY,
        move |stored| {
            // This comparison and mutation both execute under the existing config
            // update lock. A conflicting editor never changes disk or live policy.
            if revision(stored) != expected {
                return Err(SaveError::Conflict);
            }
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
