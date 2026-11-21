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
        if let Ok(settings) = serde_json::from_value::<ErrorSettings>(value.clone()) {
            let settings = upgrade_rules(settings);
            if settings.validate().is_ok() { return Arc::new(ErrorPolicySnapshot { settings, configured: true, legacy: false }); }
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
        // 旧「指定错误码直接换号」名单 → **例外**规则（允许把错误透传给下游）。
        // 语义在 #644 之后反转：默认所有上游错误都换号，名单表达的是「这几个码
        // 不换、直接返回客户端」。所以这里生成的是 `ReturnError` 例外，而不是
        // 原来的 `SwitchAccount`。
        for status in retry.no_retry_codes.iter().copied().filter(|status| (400..600).contains(status)) {
            let kind = legacy_kind(status);
            if let Some(mut rule) = settings.rules.iter().find(|rule| rule.kind == kind).cloned() {
                rule.id = format!("legacy-{status}"); rule.name = format!("原错误码名单例外 {status}");
                rule.statuses = vec![status]; rule.action = ErrorAction::ReturnError;
                rule.cooldown = CooldownMode::None;
                if kind == ErrorKind::Unsupported { rule.scope = CooldownScope::Model; }
                settings.overrides.push(rule);
            }
        }
    }
    Arc::new(ErrorPolicySnapshot { settings, configured: false, legacy })
}

/// 把旧版本的存盘规则补到 [`RULES_VERSION`] 的口径。
///
/// 为什么必须做：规则表随版本出厂，用户在面板上真正关心的往往只是「要不要
/// 换号」，冷却那几格常年停留在出厂值。#644 之前出厂值是「请求有误 / 内容被拒 /
/// 其他错误一律不冷却」，于是本地构造期的 400（例如网页工具字符预算超限）
/// 既白烧换号预算、又不会给失败的 账号×模型 留任何限制 —— 下一条请求继续
/// 挑中它、继续 400。升级二进制却还带着旧存盘，表现就是「代码改了、线上原样」。
///
/// 升级只做**加法**，不动用户选的动作与数值：
///   1. 冷却为空、而当前出厂值要求「固定冷却 / 失败退避」的规则，按出厂值补上
///      （这就是「限制器」—— 有错就得挡一会儿，而不是任由下一条请求再撞）；
///   2. `未知错误` 的兜底动作若仍是「直接返回下游」，改回换号（旧出厂值就是这么
///      写的，而它和「路由组要保证下游不中断」直接冲突）。
///
/// 例外规则（`overrides`）是用户自己配的透传名单，任何时候都不动。
fn upgrade_rules(mut settings: ErrorSettings) -> ErrorSettings {
    if settings.rules_version >= RULES_VERSION { return settings; }
    let stock = default_rules();
    let mut cooled = 0;
    for rule in settings.rules.iter_mut() {
        let Some(default) = stock.iter().find(|item| item.kind == rule.kind) else { continue };
        if rule.cooldown == CooldownMode::None
            && matches!(default.cooldown, CooldownMode::Fixed | CooldownMode::Protection)
        {
            rule.cooldown = default.cooldown;
            if rule.fallback_seconds == 0 { rule.fallback_seconds = default.fallback_seconds; }
            rule.fallback_cap_seconds = rule.fallback_cap_seconds.max(rule.fallback_seconds);
            cooled += 1;
        }
        if rule.kind == ErrorKind::Unknown && rule.action == ErrorAction::ReturnError {
            rule.action = ErrorAction::SwitchAccount;
        }
    }
    let versioned = settings.rules_version;
    settings.rules_version = RULES_VERSION;
    crate::server::logging::console_line("[Config]", &format!(
        "错误处理规则已从旧版本（v{versioned}）升级到 v{RULES_VERSION}：{cooled} 条规则补上冷却，未知错误恢复换号"
    ));
    settings
}

pub fn snapshot() -> Arc<ErrorPolicySnapshot> {
    super::CONFIG.read().ok().and_then(|guard| guard.as_ref().map(|config| config.error_policy.clone())).unwrap_or_default()
}

pub fn revision(snapshot: &ErrorPolicySnapshot) -> String {
    format!("{:x}", Sha256::digest(json!({ "settings": snapshot.settings, "configured": snapshot.configured,
        "legacy": snapshot.legacy }).to_string().as_bytes()))
}

pub fn save(mut settings: ErrorSettings, expected: &str) -> Result<Arc<ErrorPolicySnapshot>, crate::server::errors::GatewayError> {
    use crate::server::errors::GatewayError;
    // 前端提交的规则就是「当前版本的规则」：老前端不带 `rulesVersion`，被
    // serde 默认成 0 的话，保存一次就会被下一次加载当成旧配置又升级一遍，
    // 用户刚改的动作会被默认值覆盖。所以保存时统一盖上当前版本号。
    settings.rules_version = RULES_VERSION;
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
        .filter(|rule| rule.id.starts_with("legacy-") && rule.enabled && rule.action == ErrorAction::ReturnError)
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
                rule.id = format!("legacy-{status}"); rule.name = format!("原错误码名单例外 {status}");
                rule.statuses = vec![status]; rule.action = ErrorAction::ReturnError;
                rule.cooldown = CooldownMode::None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    /// 旧存盘里「所有规则都不冷却」时，升级要补上限制器，而不是放任旧口径。
    #[test]
    fn stale_rules_get_cooldowns_on_upgrade() {
        let mut settings = ErrorSettings::default();
        settings.rules_version = 0;
        for rule in settings.rules.iter_mut() { rule.cooldown = CooldownMode::None; }
        let mut raw = Map::new();
        raw.insert(KEY_ERROR_POLICY.into(), serde_json::to_value(&settings).expect("serialize"));
        let snapshot = from_raw(&raw);
        assert_eq!(snapshot.settings.rules_version, RULES_VERSION);
        for rule in &snapshot.settings.rules {
            if matches!(rule.kind, ErrorKind::BadRequest | ErrorKind::Content | ErrorKind::Unknown) {
                assert_ne!(rule.cooldown, CooldownMode::None, "{} 升级后必须有冷却", rule.kind.id());
            }
        }
        assert!(snapshot.settings.validate().is_ok(), "升级后的设置必须自身合法");
    }

    /// 用户配的例外（透传名单）在升级时原样保留。
    #[test]
    fn user_overrides_survive_upgrade() {
        let mut settings = ErrorSettings::default();
        settings.rules_version = 0;
        let mut rule = default_rules().into_iter().find(|rule| rule.kind == ErrorKind::BadRequest).expect("bad_request 规则");
        rule.id = "legacy-400".into(); rule.statuses = vec![400];
        rule.action = ErrorAction::ReturnError; rule.cooldown = CooldownMode::None;
        settings.overrides.push(rule);
        let mut raw = Map::new();
        raw.insert(KEY_ERROR_POLICY.into(), serde_json::to_value(&settings).expect("serialize"));
        let snapshot = from_raw(&raw);
        assert_eq!(snapshot.settings.overrides.len(), 1, "例外不能被升级逻辑丢掉");
        assert_eq!(snapshot.settings.overrides[0].action, ErrorAction::ReturnError);
        assert_eq!(snapshot.settings.overrides[0].id, "legacy-400");
    }
}
