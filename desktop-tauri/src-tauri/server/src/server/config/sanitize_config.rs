use std::sync::Arc;

use serde_json::{Map, Value};

use crate::server::core::sanitize::{self, RuleSet, SanitizeRule};

use super::{update, RuntimeConfig, KEY_SANITIZE_FINGERPRINTS};

pub const KEY_SANITIZE_RULES: &str = "sanitizeFingerprintRules";
pub const KEY_SANITIZE_PROVIDERS: &str = "sanitizeFingerprintProviders";
pub const MAX_SANITIZE_PROVIDERS: usize = 200;
pub const DEFAULT_SANITIZE_PROVIDERS: &[&str] = &["workbuddy", "workbuddy-intl"];

pub fn default_sanitize_providers() -> Vec<String> {
    DEFAULT_SANITIZE_PROVIDERS
        .iter()
        .map(|id| (*id).to_string())
        .collect()
}

#[derive(Clone, Debug)]
pub struct SanitizeSettings {
    pub enabled: bool,
    pub rules: Arc<RuleSet>,
    /// 默认仅应用 WorkBuddy 国内版与国际版；空列表表示用户显式指定的全部提供商（包含之后新增的插件与自定义提供商）。
    pub providers: Vec<String>,
    pub custom_rules: bool,
}

impl Default for SanitizeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            rules: sanitize::default_rules(),
            providers: default_sanitize_providers(),
            custom_rules: false,
        }
    }
}

impl SanitizeSettings {
    pub fn applies_to(&self, provider_id: &str) -> bool {
        self.enabled
            && (self.providers.is_empty()
                || self
                    .providers
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(provider_id)))
    }
}

#[derive(Default)]
pub struct SanitizePatch {
    pub enabled: Option<bool>,
    /// None = 不改，Some(None) = 恢复内置规则，Some(Some(...)) = 自定义词表。
    pub rules: Option<Option<Arc<RuleSet>>>,
    /// None = 不改，Some(None) = 恢复默认提供商范围，Some(Some(...)) = 显式指定范围（含空数组全部）。
    pub providers: Option<Option<Vec<String>>>,
}

pub(super) fn from_raw(raw: &Map<String, Value>) -> SanitizeSettings {
    let parsed = raw
        .get(KEY_SANITIZE_RULES)
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value::<Vec<SanitizeRule>>(value.clone()).ok())
        .and_then(|entries| RuleSet::compile(entries).ok());
    let custom_rules = parsed.is_some();
    let providers = match raw.get(KEY_SANITIZE_PROVIDERS) {
        Some(Value::Array(items)) => {
            if items.is_empty() {
                Vec::new()
            } else {
                let mut list: Vec<String> = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|id| id.trim().to_ascii_lowercase())
                    .filter(|id| !id.is_empty())
                    .collect();
                list.sort();
                list.dedup();
                if list.is_empty() {
                    default_sanitize_providers()
                } else {
                    list
                }
            }
        }
        _ => default_sanitize_providers(),
    };
    SanitizeSettings {
        enabled: raw
            .get(KEY_SANITIZE_FINGERPRINTS)
            .and_then(Value::as_bool)
            .unwrap_or(true),
        rules: parsed.map(Arc::new).unwrap_or_else(sanitize::default_rules),
        providers,
        custom_rules,
    }
}

impl RuntimeConfig {
    pub fn sanitize_settings(&self) -> &SanitizeSettings {
        &self.sanitize
    }
}

pub fn set_sanitize_settings(patch: SanitizePatch) -> bool {
    update(|config| {
        if let Some(enabled) = patch.enabled {
            config
                .raw
                .insert(KEY_SANITIZE_FINGERPRINTS.to_string(), Value::Bool(enabled));
            config.sanitize.enabled = enabled;
        }
        if let Some(rules) = patch.rules {
            match rules {
                Some(rules) => {
                    let entries =
                        serde_json::to_value(rules.entries()).expect("脱敏词表必须可序列化");
                    config.raw.insert(KEY_SANITIZE_RULES.to_string(), entries);
                    config.sanitize.rules = rules;
                    config.sanitize.custom_rules = true;
                }
                None => {
                    config.raw.remove(KEY_SANITIZE_RULES);
                    config.sanitize.rules = sanitize::default_rules();
                    config.sanitize.custom_rules = false;
                }
            }
        }
        if let Some(providers) = patch.providers {
            match providers {
                Some(providers) => {
                    config.raw.insert(
                        KEY_SANITIZE_PROVIDERS.to_string(),
                        Value::from(providers.clone()),
                    );
                    config.sanitize.providers = providers;
                }
                None => {
                    config.raw.remove(KEY_SANITIZE_PROVIDERS);
                    config.sanitize.providers = default_sanitize_providers();
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_applies_only_to_workbuddy_editions() {
        let defaults = SanitizeSettings::default();
        assert_eq!(defaults.providers, vec!["workbuddy", "workbuddy-intl"]);
        assert!(defaults.applies_to("workbuddy"));
        assert!(defaults.applies_to("WorkBuddy"));
        assert!(defaults.applies_to("workbuddy-intl"));
        assert!(!defaults.applies_to("antigravity"));
        assert!(!defaults.applies_to("zcode"));
        assert!(!defaults.applies_to("custom-x"));

        let from_empty = from_raw(&Map::new());
        assert_eq!(from_empty.providers, vec!["workbuddy", "workbuddy-intl"]);
        assert!(from_empty.applies_to("workbuddy"));
        assert!(from_empty.applies_to("workbuddy-intl"));
        assert!(!from_empty.applies_to("antigravity"));
    }

    #[test]
    fn explicit_empty_array_means_all_providers() {
        let mut raw = Map::new();
        raw.insert(KEY_SANITIZE_PROVIDERS.to_string(), json!([]));
        let settings = from_raw(&raw);
        assert!(settings.providers.is_empty());
        assert!(settings.applies_to("workbuddy"));
        assert!(settings.applies_to("antigravity"));
        assert!(settings.applies_to("custom-x"));
    }

    #[test]
    fn explicit_list_and_invalid_values_behave_consistently() {
        let mut raw = Map::new();
        raw.insert(
            KEY_SANITIZE_PROVIDERS.to_string(),
            json!([" ZCode ", "antigravity", "zcode", ""]),
        );
        let settings = from_raw(&raw);
        assert_eq!(settings.providers, vec!["antigravity", "zcode"]);
        assert!(settings.applies_to("zcode"));
        assert!(settings.applies_to("antigravity"));
        assert!(!settings.applies_to("workbuddy"));

        for invalid in [Value::Null, json!("workbuddy"), json!([123, ""])] {
            let mut bad_raw = Map::new();
            bad_raw.insert(KEY_SANITIZE_PROVIDERS.to_string(), invalid);
            let parsed = from_raw(&bad_raw);
            assert_eq!(parsed.providers, vec!["workbuddy", "workbuddy-intl"]);
        }
    }
}
