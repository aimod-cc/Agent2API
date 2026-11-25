use std::sync::Arc;

use serde_json::{Map, Value};

use crate::server::core::privacy::{CompiledPrivacySettings, PrivacySettings};

use super::{update, RuntimeConfig};

pub const KEY_PRIVACY_REDACTION: &str = "privacyRedaction";

pub(super) fn from_raw(raw: &Map<String, Value>) -> Arc<CompiledPrivacySettings> {
    let settings = raw
        .get(KEY_PRIVACY_REDACTION)
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value::<PrivacySettings>(value.clone()).ok())
        .unwrap_or_default();
    Arc::new(CompiledPrivacySettings::compile(settings).unwrap_or_default())
}

impl RuntimeConfig {
    pub fn privacy_settings(&self) -> Arc<CompiledPrivacySettings> {
        self.privacy.clone()
    }
}

pub fn set_privacy_settings(
    settings: PrivacySettings,
) -> Result<(bool, Arc<CompiledPrivacySettings>), String> {
    let compiled = Arc::new(CompiledPrivacySettings::compile(settings)?);
    let serialized = serde_json::to_value(&compiled.settings)
        .map_err(|error| format!("隐私脱敏配置序列化失败：{error}"))?;
    let next = compiled.clone();
    let saved = update(|config| {
        config
            .raw
            .insert(KEY_PRIVACY_REDACTION.to_string(), serialized);
        config.privacy = next;
    });
    Ok((saved, compiled))
}
