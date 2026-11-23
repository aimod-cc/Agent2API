use std::sync::Arc;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub use super::error_policy_types::*;

pub const KEY_ERROR_POLICY: &str = "routingErrorPolicy";

#[derive(Clone, Debug)]
pub struct ErrorPolicySnapshot {
    pub settings: ErrorSettings,
    pub configured: bool,
    pub legacy: bool,
}

impl Default for ErrorPolicySnapshot {
    fn default() -> Self { Self { settings: ErrorSettings::default(), configured: false, legacy: false } }
}

pub(crate) fn from_raw(raw: &Map<String, Value>) -> Arc<ErrorPolicySnapshot> {
    if let Some(value) = raw.get(KEY_ERROR_POLICY) {
        if let Ok(mut settings) = serde_json::from_value::<ErrorSettings>(value.clone()) {
            if settings.validate().is_ok() {
                migrate_untouched_v1_defaults(&mut settings);
                return Arc::new(ErrorPolicySnapshot { settings, configured: true, legacy: false });
            }
        }
    }
    let legacy = [super::KEY_RETRY_COUNT, super::KEY_RETRY_ACCOUNT_SWITCH_COUNT, super::KEY_RETRY_INTERVAL_SECONDS,
        super::KEY_RETRY_NO_RETRY_CODES, super::KEY_ROUTING_FAILOVER_BACKOFF, super::KEY_ROUTING_SINK,
        super::KEY_ROUTING_BREAKER_ENABLED, super::KEY_ROUTING_BREAKER_THRESHOLD, super::KEY_ROUTING_BREAKER_TIMEOUT]
        .iter().any(|key| raw.contains_key(*key));
    let mut settings = ErrorSettings::default();
    if legacy {
        let retry = super::parse::retry_from(raw);
        let routing = super::parse::routing_from(raw);
        settings.resends = retry.count.max(0) as u64;
        settings.switches = retry.account_switch_count.max(0) as u64;
        settings.interval_seconds = retry.interval_seconds.max(0) as u64;
        settings.backoff = routing.failover_backoff;
        settings.backoff_seconds = 900;
        settings.backoff_cap_seconds = 3600;
        settings.sink = routing.sink;
        settings.breaker = routing.breaker_enabled;
        settings.breaker_failures = routing.breaker_threshold.max(1) as u64;
        settings.breaker_seconds = routing.breaker_timeout_seconds.max(10) as u64;
        settings.breaker_successes = routing.breaker_success_threshold.max(1) as u64;
        if let Some(rule) = settings.rules.iter_mut().find(|rule| rule.kind == ErrorKind::RateLimit) {
            rule.fallback_seconds = 600; rule.exponential = true;
        }
        for status in retry.no_retry_codes.iter().copied().filter(|status| (400..600).contains(status)) {
            let kind = legacy_kind(status);
            if let Some(mut rule) = settings.rules.iter().find(|rule| rule.kind == kind).cloned() {
                rule.id = format!("legacy-{status}"); rule.name = format!("原直接换号设置 {status}"); rule.statuses = vec![status];
                rule.action = ErrorAction::SwitchAccount;
                if kind == ErrorKind::Unsupported { rule.scope = CooldownScope::Model; }
                settings.overrides.push(rule);
            }
        }
    }
    Arc::new(ErrorPolicySnapshot { settings, configured: false, legacy })
}

fn migrate_untouched_v1_defaults(settings: &mut ErrorSettings) {
    let v1 = v1_default_rules();
    let v2 = default_rules();
    for rule in &mut settings.rules {
        if let (Some(old), Some(next)) = (v1.iter().find(|item| item.kind == rule.kind), v2.iter().find(|item| item.kind == rule.kind)) {
            if *rule == *old {
                *rule = next.clone();
            }
        }
    }
}

pub fn snapshot() -> Arc<ErrorPolicySnapshot> {
    super::CONFIG.read().ok().and_then(|guard| guard.as_ref().map(|config| config.error_policy.clone())).unwrap_or_default()
}

pub fn revision(snapshot: &ErrorPolicySnapshot) -> String {
    format!("{:x}", Sha256::digest(json!({ "settings": snapshot.settings, "configured": snapshot.configured,
        "legacy": snapshot.legacy }).to_string().as_bytes()))
}

pub fn save(settings: ErrorSettings, expected: &str) -> Result<Arc<ErrorPolicySnapshot>, crate::server::errors::GatewayError> {
    use crate::server::errors::GatewayError;
    settings.validate().map_err(GatewayError::bad_request)?;
    super::try_update_raw_fields(|raw| {
        if revision(&from_raw(raw)) != expected { return Err(GatewayError::with_status(409, "错误处理设置已在其他窗口修改，请重新读取后保存")); }
        write_settings(raw, &settings)?;
        Ok(())
    }, || GatewayError::with_status(500, "错误处理设置保存失败，原设置保持不变"))?;
    Ok(snapshot())
}

pub(crate) fn write_settings(raw: &mut Map<String, Value>, settings: &ErrorSettings) -> Result<(), crate::server::errors::GatewayError> {
    let value = serde_json::to_value(settings).map_err(|_| crate::server::errors::GatewayError::new("错误处理设置无法保存"))?;
    raw.insert(KEY_ERROR_POLICY.into(), value);
    for (key, value) in [
        (super::KEY_RETRY_COUNT, settings.resends), (super::KEY_RETRY_ACCOUNT_SWITCH_COUNT, settings.switches),
        (super::KEY_RETRY_INTERVAL_SECONDS, settings.interval_seconds), (super::KEY_ROUTING_BREAKER_THRESHOLD, settings.breaker_failures),
        (super::KEY_ROUTING_BREAKER_TIMEOUT, settings.breaker_seconds), (super::KEY_ROUTING_BREAKER_SUCCESS, settings.breaker_successes),
    ] { raw.insert(key.into(), value.into()); }
    for (key, value) in [(super::KEY_ROUTING_FAILOVER_BACKOFF, settings.backoff), (super::KEY_ROUTING_SINK, settings.sink),
        (super::KEY_ROUTING_BREAKER_ENABLED, settings.breaker)] { raw.insert(key.into(), value.into()); }
    raw.insert(super::KEY_RETRY_NO_RETRY_CODES.into(), settings.overrides.iter()
        .filter(|rule| rule.id.starts_with("legacy-") && rule.enabled && rule.action == ErrorAction::SwitchAccount)
        .flat_map(|rule| rule.statuses.iter().copied()).map(Value::from).collect::<Vec<_>>().into());
    Ok(())
}

pub(crate) fn sync_legacy(raw: &mut Map<String, Value>, retry_codes_changed: bool) {
    if !raw.contains_key(KEY_ERROR_POLICY) { return; }
    let retry = super::parse::retry_from(raw);
    let routing = super::parse::routing_from(raw);
    let mut settings = from_raw(raw).settings.clone();
    settings.resends = retry.count.max(0) as u64; settings.switches = retry.account_switch_count.max(0) as u64;
    settings.interval_seconds = retry.interval_seconds.max(0) as u64;
    settings.backoff = routing.failover_backoff; settings.sink = routing.sink; settings.breaker = routing.breaker_enabled;
    settings.breaker_failures = routing.breaker_threshold.max(1) as u64;
    settings.breaker_seconds = routing.breaker_timeout_seconds.max(10) as u64;
    settings.breaker_successes = routing.breaker_success_threshold.max(1) as u64;
    if retry_codes_changed {
        settings.overrides.retain(|rule| !rule.id.starts_with("legacy-"));
        for status in retry.no_retry_codes.iter().copied().filter(|status| (400..600).contains(status)) {
            let kind = legacy_kind(status);
            if let Some(mut rule) = settings.rules.iter().find(|rule| rule.kind == kind).cloned() {
                rule.id = format!("legacy-{status}"); rule.name = format!("原直接换号设置 {status}");
                rule.statuses = vec![status]; rule.action = ErrorAction::SwitchAccount;
                if kind == ErrorKind::Unsupported { rule.scope = CooldownScope::Model; }
                settings.overrides.insert(0, rule);
            }
        }
    }
    if let Ok(value) = serde_json::to_value(settings) { raw.insert(KEY_ERROR_POLICY.into(), value); }
}

fn legacy_kind(status: u16) -> ErrorKind {
    match status {
        429 => ErrorKind::RateLimit, 401 => ErrorKind::Auth, 402 => ErrorKind::Balance,
        501 => ErrorKind::Unsupported, 400 | 413 | 422 => ErrorKind::BadRequest,
        408 | 500 | 502 | 503 | 504 => ErrorKind::Transient, _ => ErrorKind::Unknown,
    }
}
