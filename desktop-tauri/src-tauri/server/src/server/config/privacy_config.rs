use std::sync::Arc;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::privacy::{CompiledPrivacySettings, PrivacySettings};

use super::{try_update_raw_field, RuntimeConfig};

pub const KEY_PRIVACY_REDACTION: &str = "privacyRedaction";

#[derive(Clone, Debug)]
pub(crate) struct PrivacyConfigSnapshot {
    pub compiled: Arc<CompiledPrivacySettings>,
    pub revision: String,
}

impl Default for PrivacyConfigSnapshot {
    fn default() -> Self {
        Self {
            compiled: Arc::new(CompiledPrivacySettings::default()),
            revision: revision(&Value::Null),
        }
    }
}

#[derive(Debug)]
pub enum PrivacySaveError {
    Invalid(String),
    Conflict,
    Storage,
}

pub fn revision(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

pub(super) fn from_raw(raw: &Map<String, Value>) -> PrivacyConfigSnapshot {
    let stored = raw.get(KEY_PRIVACY_REDACTION).unwrap_or(&Value::Null);
    let settings = raw
        .get(KEY_PRIVACY_REDACTION)
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value::<PrivacySettings>(value.clone()).ok())
        .unwrap_or_default();
    PrivacyConfigSnapshot {
        compiled: Arc::new(CompiledPrivacySettings::compile(settings).unwrap_or_default()),
        revision: revision(stored),
    }
}

impl RuntimeConfig {
    pub fn privacy_settings(&self) -> Arc<CompiledPrivacySettings> {
        self.privacy.compiled.clone()
    }

    pub fn privacy_revision(&self) -> &str {
        &self.privacy.revision
    }
}

pub fn save_privacy_settings(
    settings: PrivacySettings,
    expected_revision: Option<&str>,
) -> Result<(Arc<CompiledPrivacySettings>, String), PrivacySaveError> {
    let compiled =
        Arc::new(CompiledPrivacySettings::compile(settings).map_err(PrivacySaveError::Invalid)?);
    let serialized = serde_json::to_value(&compiled.settings)
        .map_err(|error| PrivacySaveError::Invalid(format!("隐私脱敏配置序列化失败：{error}")))?;
    let next_revision = revision(&serialized);
    let expected = expected_revision.map(str::to_owned);
    try_update_raw_field(
        KEY_PRIVACY_REDACTION,
        move |stored| {
            if let Some(ref exp) = expected {
                if revision(stored) != *exp {
                    return Err(PrivacySaveError::Conflict);
                }
            }
            *stored = serialized;
            Ok(())
        },
        || PrivacySaveError::Storage,
    )?;
    Ok((compiled, next_revision))
}
