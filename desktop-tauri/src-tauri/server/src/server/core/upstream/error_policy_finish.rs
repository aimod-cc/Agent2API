use serde_json::json;

use crate::server::config::error_policy::ErrorKind;
use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

pub(super) fn persist_account_action(
    store: &AccountStore,
    account_id: &str,
    suspend: bool,
    cooldown_until: Option<i64>,
    reason: &str,
) {
    if account_id.is_empty() {
        return;
    }
    if suspend {
        if let Err(err) = crate::server::core::blocking::in_place(|| store.suspend_upstream_account(account_id, reason)) {
            logging::log_with_level("[Upstream]", &format!("标记上游账号挂起失败 ({account_id}): {err}"), "warn");
        }
    }
    if let Some(until) = cooldown_until {
        if let Err(err) = crate::server::core::blocking::in_place(|| store.cooldown_upstream_account(account_id, until, reason)) {
            logging::log_with_level("[Upstream]", &format!("记录上游账号冷却失败 ({account_id}): {err}"), "warn");
        }
    }
}

use super::{error_policy, rotate, RouteTarget, UpstreamService};
use super::payload::ProviderContext;

#[derive(Clone, Copy)]
pub(super) struct StickyContext<'a> {
    pub key: &'a str,
    pub model: &'a str,
    pub generation: u64,
}

/// 候选失败后立即解除会话粘性绑定（固定指定账号测试时不改动正常会话粘性）。
pub(super) fn forget_sticky(
    service: &UpstreamService,
    ctx: &ProviderContext<'_>,
    sticky: StickyContext<'_>,
    candidate: Option<&str>,
) {
    if ctx.pinned_account.is_some() {
        return;
    }
    if let Some(candidate) = candidate.filter(|value| !value.is_empty()) {
        service.route.sticky_forget(sticky.key, sticky.model, candidate, sticky.generation);
    }
}

pub(super) fn finish_failure(service: &UpstreamService, ctx: &ProviderContext<'_>, providers: &[&str], keys: &rotate::CooldownKeys<'_>,
    target: &RouteTarget, error: &mut GatewayError, tried: &mut Vec<String>, switches_left: &mut usize, switch_total: usize,
    sticky: StickyContext<'_>) -> bool {
    forget_sticky(service, ctx, sticky, target.attempt_key().as_deref());
    let model = error.routing_error.as_ref().map(|decision| decision.model.clone()).unwrap_or_else(|| {
        target.account.as_ref().map(|account| keys.for_account(account)).unwrap_or_else(|| keys.for_provider(&target.provider))
    });
    let settings = &ctx.error_policy.settings;
    let decision = error.routing_error.clone().unwrap_or_else(|| error_policy::for_error_with(&ctx.error_policy, &target.provider, &model, error, false));
    error_policy::attach(error, &decision);
    error_policy::note(ctx.telemetry, &decision, None);
    let replay_or_started = ctx.telemetry.replay_guard().blocked() || ctx.telemetry.response_started();
    if decision.hard_stop || ctx.telemetry.is_cancelled() { return false; }
    if let Some(account) = target.account_id.as_deref() {
        if decision.deprioritizes() {
            let until = service.route.deprioritize_mark_scoped(account, &target.provider, &model, decision.scope, &decision.rule, logging::now_ms());
            if until > logging::now_ms() {
                error.retry_after_secs.get_or_insert(until.saturating_sub(logging::now_ms()).max(0) as u64 / 1000 + 1);
            }
            ctx.telemetry.note_error_policy(json!({ "resetAt": until, "timeSource": "sink" }));
        } else if let Some(entry) = error_policy::remember_cooldown(service, settings, account, &model, error, &decision) {
            let reset = entry["resetAt"].as_i64().unwrap_or(0);
            if reset > logging::now_ms() { error.retry_after_secs = Some(reset.saturating_sub(logging::now_ms()).max(0) as u64 / 1000 + 1); }
            ctx.telemetry.note_error_policy(json!({ "resetAt": entry["resetAt"], "timeSource": entry["timeSource"] }));
        }
        if !decision.deprioritizes() && matches!(decision.kind, ErrorKind::RateLimit | ErrorKind::Quota) && settings.sink && ctx.pinned_account.is_none() {
            service.route.sink_mark_for(account, logging::now_ms(), settings.sink_seconds);
        }
        if decision.kind == ErrorKind::Transient && decision.rule.cooldown == crate::server::config::error_policy::CooldownMode::Protection {
            if settings.backoff { service.route.backoff_mark_for(account, logging::now_ms(), settings.backoff_seconds, settings.backoff_cap_seconds); }
            if settings.breaker {
                service.route.breaker_failure(account, settings.breaker_failures as i64, settings.breaker_seconds as i64, logging::now_ms());
            }
        }
        if decision.kind == ErrorKind::Auth && target.account.as_ref().is_some_and(|account| account["needsRelogin"] == true) {
            ctx.telemetry.note_error_policy(json!({ "reason": "该账号需要重新登录" }));
        }
    }
    if replay_or_started { return false; }
    if !decision.switches() { return false; }
    if decision.scope == crate::server::config::error_policy::CooldownScope::Account {
        if let Some(id) = target.account_id.as_deref() {
            for candidate in rotate::scoped_accounts(service, providers, keys.request_model(), ctx.pinned_account, Some(&ctx.routing)) {
                if crate::server::core::routing::account_id(&candidate) == Some(id) {
                    let key = crate::server::core::routing_groups::attempt_key(&candidate);
                    if !tried.contains(&key) { tried.push(key); }
                }
            }
        }
    }
    if let Some(account) = target.account_id.as_deref() {
        if !rotate::allows_route_skip(service, account) {
            ctx.telemetry.note_error_policy(json!({ "reason": "当前账号已关闭失败换号" })); return false;
        }
    }
    if ctx.pinned_account.is_some() || ctx.routing.requires_account() {
        ctx.telemetry.note_error_policy(json!({ "reason": "本次请求明确指定账号，未更换指定对象" })); return false;
    }
    if decision.skips_provider() {
        error_policy::exclude_provider(service, providers, keys.request_model(), &target.provider, tried, ctx.pinned_account, &ctx.routing);
    }
    if let Some(id) = target.attempt_key() { tried.retain(|other| other != &id); tried.push(id); }
    let Some(next) = rotate::pick_next_account(service, providers, keys, tried, ctx.pinned_account, &ctx.routing) else {
        ctx.telemetry.note_error_policy(json!({ "reason": "当前路由没有其他可用对象" })); return false;
    };
    if !super::provider_loop::take_switch(switches_left, switch_total) {
        ctx.telemetry.note_error_policy(json!({ "reason": "换账号次数已用尽，未继续发送" })); return false;
    }
    logging::console_line("[Upstream]", &format!("统一错误规则「{}」：HTTP {}，顺延到 {}（换号 {}/{}）",
        decision.rule.name, error.status_code, super::account_display(&next), switch_total - *switches_left, switch_total));
    true
}
