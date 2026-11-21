use serde_json::json;

use crate::server::config::error_policy::ErrorKind;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{error_policy, rotate, RouteTarget, UpstreamService};
use super::payload::ProviderContext;

pub(super) fn finish_failure(service: &UpstreamService, ctx: &ProviderContext<'_>, providers: &[&str], keys: &rotate::CooldownKeys<'_>,
    target: &RouteTarget, error: &mut GatewayError, tried: &mut Vec<String>, switches_left: &mut usize, switch_total: usize) -> bool {
    let model = error.routing_error.as_ref().map(|decision| decision.model.clone()).unwrap_or_else(|| {
        target.account.as_ref().map(|account| keys.for_account(account)).unwrap_or_else(|| keys.for_provider(&target.provider))
    });
    let settings = &ctx.error_policy.settings;
    let decision = error.routing_error.clone().unwrap_or_else(|| error_policy::for_error_with(&ctx.error_policy, &target.provider, &model, error, false));
    error_policy::attach(error, &decision);
    error_policy::note(ctx.telemetry, &decision, None);
    if decision.hard_stop || ctx.telemetry.replay_guard().blocked() || ctx.telemetry.is_cancelled() || ctx.telemetry.response_started() { return false; }
    if let Some(account) = target.account_id.as_deref() {
        if let Some(entry) = error_policy::remember_cooldown(service, settings, account, &model, error, &decision) {
            let reset = entry["resetAt"].as_i64().unwrap_or(0);
            if reset > logging::now_ms() { error.retry_after_secs = Some(reset.saturating_sub(logging::now_ms()).max(0) as u64 / 1000 + 1); }
            ctx.telemetry.note_error_policy(json!({ "resetAt": entry["resetAt"], "timeSource": entry["timeSource"] }));
        }
        if matches!(decision.kind, ErrorKind::RateLimit | ErrorKind::Quota) && settings.sink && ctx.pinned_account.is_none() {
            service.route.sink_mark_for(account, logging::now_ms(), settings.sink_seconds);
        }
        // ── 上游说「余额/积分不足」→ 把本地余额读数按 0 落一次 ──────────
        // 余额是查上游接口读回来的，字段形状一变就会读出一个明显偏高的数字；
        // 限制器与低余额跳过都按这个数字判断，于是账号一直显得「有钱」，每条
        // 请求都被派上去再吃一次 402「积分余额不足」。上游的 402 比本地读数更
        // 权威：这里直接落 0，余额列显示 0、低余额规则立刻拦下它，充值后下一
        // 次成功查询会自己把 0 顶掉。
        if decision.kind == ErrorKind::Balance || says_no_money(&error.message) {
            crate::server::core::usage_records::force_zero_balance(
                account, "上游报余额不足，余额读数已按 0 处理；充值后自动恢复");
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

/// 上游文案里明说「余额/积分不足」——包括被 429 外壳包住的情况
/// （`上游返回 429: 上游返回 402: 积分余额不足，请获取积分后再试`）。
///
/// 为什么要看文案：402 会直接分到 `Balance`，但「429 包 402」的顶层状态码是
/// 429，分类只看状态码/机器码时会被当成限流，这个账号的钱包事实就丢了。限流
/// 词先排除，避免把真正的 429 也当欠费（那会误清余额读数）。
fn says_no_money(message: &str) -> bool {
    let lower = message.to_lowercase();
    if ["rate limit", "too many requests", "限流", "频率", "rpm", "tpm"].iter().any(|word| lower.contains(word)) {
        return false;
    }
    ["积分余额不足", "余额不足", "积分不足", "欠费", "没有可用积分", "insufficient balance", "insufficient credit",
        "insufficient_quota", "payment required", "payment_required"]
        .iter()
        .any(|word| lower.contains(word))
}
