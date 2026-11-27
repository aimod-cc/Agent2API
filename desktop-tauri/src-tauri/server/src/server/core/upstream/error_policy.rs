use serde_json::{json, Value};

use crate::server::config::error_policy::{self as config, CooldownMode, CooldownScope, DeclaredErrorPolicy, ErrorAction, ErrorKind, ErrorRule, RuleSource};
use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::errors::GatewayError;
use crate::server::logging;

#[path = "error_policy_evidence.rs"]
mod evidence;
pub(super) use evidence::{detail, gateway_class};

#[derive(Clone, Debug)]
pub(crate) struct ErrorDecision {
    pub rule: ErrorRule,
    pub rule_source: RuleSource,
    pub kind: ErrorKind,
    pub action: ErrorAction,
    pub scope: CooldownScope,
    pub reset_at: Option<i64>,
    pub hard_stop: bool,
    pub exception: bool,
    pub interval_ms: u64,
    pub floor_ms: u64,
    pub model: String,
}

impl ErrorDecision {
    pub fn retries(&self) -> bool { !self.hard_stop && self.action == ErrorAction::RetryThenSwitch }
    pub fn refreshes(&self) -> bool { !self.hard_stop && self.action == ErrorAction::RefreshThenSwitch }
    pub fn switches(&self) -> bool { !self.hard_stop && self.action != ErrorAction::ReturnError }
    pub fn skips_provider(&self) -> bool { self.scope == CooldownScope::Provider || self.action == ErrorAction::SwitchProvider }
    pub fn deprioritizes(&self) -> bool { !self.hard_stop && self.rule.cooldown == CooldownMode::Deprioritize }
    pub fn cooldown(&self) -> bool { !self.hard_stop && !matches!(self.rule.cooldown, CooldownMode::None | CooldownMode::Deprioritize) }
    pub(crate) fn cooldown_rule(&self, settings: &config::ErrorSettings) -> Option<ErrorRule> {
        if !self.cooldown() { return None; }
        let mut rule = self.rule.clone();
        if rule.cooldown == CooldownMode::Protection {
            if !settings.backoff { return None; }
            rule.fallback_seconds = settings.backoff_seconds;
            rule.fallback_cap_seconds = settings.backoff_cap_seconds;
            rule.exponential = true;
        }
        Some(rule)
    }
    pub fn retry_delay(&self, suggested: u64) -> u64 {
        let required = self.reset_at.map(|at| at.saturating_sub(logging::now_ms()).max(0) as u64).unwrap_or(0);
        self.interval_ms.max(suggested).max(required)
    }
}

pub(crate) fn decide(provider: &str, model: &str, class: &UpstreamErrorClass, status: u16, body: &Value,
    local: bool, explicit_stop: bool) -> ErrorDecision {
    decide_with(&config::snapshot(), provider, model, class, status, body, local, explicit_stop)
}

pub(crate) fn decide_with(snapshot: &config::ErrorPolicySnapshot, provider: &str, model: &str, class: &UpstreamErrorClass,
    status: u16, body: &Value, local: bool, explicit_stop: bool) -> ErrorDecision {
    let declared = crate::server::core::plugin_host::error_rules::for_provider(provider);
    decide_with_declared(snapshot, &declared, provider, model, class, status, body, local, explicit_stop)
}

pub(crate) fn decide_with_declared(snapshot: &config::ErrorPolicySnapshot, declared: &DeclaredErrorPolicy,
    provider: &str, model: &str, class: &UpstreamErrorClass,
    status: u16, body: &Value, local: bool, explicit_stop: bool) -> ErrorDecision {
    let settings = &snapshot.settings;
    let codes = evidence::candidate_codes(body);
    let code = codes.first().cloned().unwrap_or_default();
    let code_refs: Vec<&str> = codes.iter().map(String::as_str).collect();
    let classified = evidence::match_classification(&declared.classifications, status, &codes);
    let mut kind = evidence::kind(provider, class, status, &code, &codes, &declared.classifications);
    if (kind == ErrorKind::Unknown || (matches!(status, 400 | 403 | 409) && kind == ErrorKind::BadRequest))
        && !(local && matches!(code.as_str(), "bad_params" | "plugin_request_too_large" | "web_tools_invalid_request"))
    {
        if let Some(inferred) = settings.infer_exception_kind(&declared.rules, status, &code_refs, provider, model) {
            kind = inferred;
        }
    }
    let classified_specific = classified.is_some_and(|item| {
        !item.codes.is_empty() && !matches!(item.kind, ErrorKind::RateLimit | ErrorKind::Quota | ErrorKind::Unknown)
    });
    if !local && !classified_specific
        && matches!(kind, ErrorKind::RateLimit | ErrorKind::Quota | ErrorKind::Unknown)
        && evidence::balance_message_signal(body, class)
    {
        kind = ErrorKind::Balance;
    }
    let (resolved, rule_source) = settings.resolve_with(&declared.rules, kind, status, &code_refs, provider, model);
    let rule = resolved.clone();
    let upstream_scope = evidence::scope(body);
    let mut scope = match (rule.scope, upstream_scope) {
        (_, CooldownScope::Provider) => CooldownScope::Provider,
        (CooldownScope::Auto | CooldownScope::Model, CooldownScope::Account) => CooldownScope::Account,
        (CooldownScope::Auto, _) => {
            if upstream_scope == CooldownScope::Model {
                classified.and_then(|c| c.scope).unwrap_or(upstream_scope)
            } else {
                upstream_scope
            }
        }
        (scope, _) => scope,
    };
    let account_model = matches!(code.as_str(), "account_model_unavailable" | "account_model_permissions_unknown" | "model_not_available_for_account");
    let hard_stop = explicit_stop
        || evidence::hard_stop(status, &code, &codes)
        || (rule_source != RuleSource::Override && classified.is_some_and(|c| c.hard_stop))
        || local && matches!(code.as_str(), "bad_params" | "plugin_request_too_large" | "web_tools_invalid_request")
        || local && status == 501 && !account_model && !matches!(code.as_str(), "unsupported" | "no_method" | "not_implemented"
            | "route_not_supported" | "subscription_sharing_route_not_supported" | "subscription_sharing_unsupported_capability");
    let mut action = if hard_stop { ErrorAction::ReturnError } else { rule.action };
    if account_model {
        scope = CooldownScope::Model;
        if action == ErrorAction::SwitchProvider { action = ErrorAction::SwitchAccount; }
    } else if kind == ErrorKind::Unsupported && rule.scope == CooldownScope::Auto { scope = CooldownScope::Provider; }
    if matches!(kind, ErrorKind::Auth | ErrorKind::Balance) && rule.scope == CooldownScope::Auto && scope != CooldownScope::Provider {
        scope = CooldownScope::Account;
    }
    if kind != ErrorKind::Auth && action == ErrorAction::RefreshThenSwitch { action = ErrorAction::ReturnError; }
    let now = logging::now_ms();
    let mut reset_at = evidence::reset(body, now);
    if let UpstreamErrorClass::QuotaLimited { reset_at: Some(at), .. } = class {
        if *at > now && *at <= (1_i64 << 53) - 1 && chrono::DateTime::from_timestamp_millis(*at).is_some() {
            reset_at = Some(reset_at.unwrap_or(*at).max(*at));
        }
    }
    let exception = rule_source != RuleSource::Global;
    ErrorDecision { rule, rule_source, kind, action, scope, reset_at, hard_stop, exception, model: model.to_string(),
        interval_ms: settings.interval_seconds.saturating_mul(1000), floor_ms: if matches!(kind, ErrorKind::RateLimit | ErrorKind::Quota) {
            settings.cooldown_floor_seconds.saturating_mul(1000)
        } else { 0 } }
}

#[cfg(test)]
pub(super) fn for_error(provider: &str, model: &str, error: &GatewayError, explicit_stop: bool) -> ErrorDecision {
    for_error_with(&config::snapshot(), provider, model, error, explicit_stop)
}

pub(super) fn for_error_with(snapshot: &config::ErrorPolicySnapshot, provider: &str, model: &str,
    error: &GatewayError, explicit_stop: bool) -> ErrorDecision {
    let local = error.native_response.is_none() && error.conversation_class.is_none();
    decide_with(snapshot, provider, model, &evidence::gateway_class(error), error.status_code as u16, &evidence::detail(error), local, explicit_stop)
}

pub(super) fn classified_error(class: &UpstreamErrorClass, body: &Value) -> GatewayError {
    let (status, message, code) = match class {
        UpstreamErrorClass::QuotaLimited { status, message, upstream_code, .. }
        | UpstreamErrorClass::ContentBlocked { status, message, upstream_code }
        | UpstreamErrorClass::Fatal { status, message, upstream_code } => (*status, message, *upstream_code),
        UpstreamErrorClass::TokenExpired { message } => (401, message, body["code"].as_i64()),
    };
    let mut error = GatewayError::with_status(i32::from(status), if message.is_empty() {
        body["message"].as_str().unwrap_or("上游请求失败").to_string()
    } else { message.clone() }).with_optional_code(code);
    let machine = evidence::machine_code(body);
    if !machine.is_empty() { error.code = Some(machine); }
    error.conversation_class = Some(class.clone());
    error
}

pub(super) fn plugin_stops(_provider: &str, policy: &crate::server::core::providers::policy::ErrorPolicy,
    _class: &UpstreamErrorClass, status: u16, body: &Value) -> bool {
    if !policy.stop { return false; }
    let code = evidence::machine_code(body);
    let codes = evidence::candidate_codes(body);
    policy.stop_switching || evidence::hard_stop(status, &code, &codes)
}

pub(super) fn attach(error: &mut GatewayError, decision: &ErrorDecision) {
    error.routing_error = Some(decision.clone());
    if let Some(at) = decision.reset_at {
        let left = at.saturating_sub(logging::now_ms()).max(0) as u64;
        if left > 0 { error.retry_after_secs = Some(left.div_ceil(1000)); }
    }
}

pub(crate) fn note(telemetry: &super::usage::RequestTelemetry, decision: &ErrorDecision, reason: Option<&str>) {
    let action = match decision.action {
        ErrorAction::SwitchAccount => "直接换账号", ErrorAction::RetryThenSwitch => "先重发，再换账号",
        ErrorAction::RefreshThenSwitch => "续期一次，失败再换号", ErrorAction::SwitchProvider => "跳过提供商",
        ErrorAction::ReturnError => "直接返回错误",
    };
    let time_source = if decision.deprioritizes() {
        "sink"
    } else if !decision.cooldown() {
        "none"
    } else if decision.reset_at.is_some() {
        "upstream"
    } else {
        "fallback"
    };
    telemetry.note_error_policy(json!({
        "ruleId": decision.rule.id,
        "ruleName": decision.rule.name,
        "source": decision.rule_source,
        "kind": decision.kind,
        "action": decision.action,
        "scope": decision.scope,
        "cooldownMode": decision.rule.cooldown,
        "resetAt": decision.reset_at,
        "timeSource": time_source,
        "exception": decision.exception,
        "reason": reason.unwrap_or(action)
    }));
}

pub(super) fn remember_cooldown(service: &super::UpstreamService, settings: &config::ErrorSettings, account_id: &str, model: &str,
    error: &GatewayError, decision: &ErrorDecision) -> Option<Value> {
    if account_id.is_empty() { return None; }
    let rule = decision.cooldown_rule(settings)?;
    let balance_baseline = if decision.kind == ErrorKind::Balance && decision.scope == CooldownScope::Account {
        crate::server::core::usage_records::balance_facts()
            .get(account_id)
            .and_then(|fact| fact.remaining)
            .filter(|value| value.is_finite())
    } else {
        None
    };
    crate::server::core::blocking::in_place(|| service.store.mark_policy_cooldown_with_baseline(
        account_id, model, decision.scope, error.status_code, error.upstream_code,
        decision.reset_at, &error.message, &rule, decision.floor_ms, balance_baseline,
    ))
}

pub(super) fn exclude_provider(service: &super::UpstreamService, providers: &[&str], model: &str, provider: &str,
    tried: &mut Vec<String>, pinned: Option<&str>, routing: &super::route_context::RequestRouting) {
    for account in super::rotate::scoped_accounts(service, providers, model, pinned, Some(routing)) {
        if crate::server::core::routing::provider_of(&account) == provider {
            let id = crate::server::core::routing_groups::attempt_key(&account);
            if !tried.contains(&id) { tried.push(id); }
        }
    }
}
