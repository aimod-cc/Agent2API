use serde_json::json;

use crate::server::config::error_policy::ErrorKind;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{error_policy, rotate, RouteTarget, UpstreamService};
use super::payload::ProviderContext;

pub(super) fn finish_failure(service: &UpstreamService, ctx: &ProviderContext<'_>, providers: &[&str], keys: &rotate::CooldownKeys<'_>,
    target: &RouteTarget, error: &mut GatewayError, tried: &mut Vec<String>, switches_left: &mut usize, switch_total: usize,
    conv_key: &str, sticky_model: &str, sticky_generation: u64) -> bool {
    // 失败即解除会话绑定：粘性只在成功时建立，绑着一个刚失败的候选只会让
    // 同一会话的下一条请求先撞一次同样的错误（429 时表现为「每次都先吃一个
    // 429」）。解绑放在最前面 —— 哪怕这一次最终收尾（hard_stop / 无候选），
    // 也不该把失败账号继续粘在这个会话上。
    if let Some(candidate) = target.attempt_key() {
        service.route.sticky_forget(conv_key, sticky_model, &candidate, sticky_generation);
    }
    let model = error.routing_error.as_ref().map(|decision| decision.model.clone()).unwrap_or_else(|| {
        target.account.as_ref().map(|account| keys.for_account(account)).unwrap_or_else(|| keys.for_provider(&target.provider))
    });
    let settings = &ctx.error_policy.settings;
    let decision = error.routing_error.clone().unwrap_or_else(|| error_policy::for_error_with(&ctx.error_policy, &target.provider, &model, error, false));
    error_policy::attach(error, &decision);
    error_policy::note(ctx.telemetry, &decision, None);
    // ── 「这家吃不下这份请求」要把整家铺上冷却，而不是只罚撞墙的那个账号 ──
    // provider_local 的 400（网页工具预算超限、该家不接受的参数）与账号无关：
    // 同一家的另一个账号拿到的是同一份 body、同一个预算，撞的是同一堵墙。只给
    // 失败账号记冷却的话，下一条请求会派同家的下一个账号再吃一次一模一样的
    // 400 —— 用户看到的就是「前面报过错的没有被熔断，后续请求继续往上撞」。
    // 冷却键仍是模型维度（这家别的模型照常可用），但铺到该家的全部账号。
    if decision.skips_provider() && decision.scope == crate::server::config::error_policy::CooldownScope::Model
        && ctx.pinned_account.is_none() {
        for account in rotate::scoped_accounts(service, providers, keys.request_model(), None, Some(&ctx.routing)) {
            if crate::server::core::routing::provider_of(&account) != target.provider { continue; }
            let Some(id) = crate::server::core::routing::account_id(&account) else { continue };
            if Some(id) == target.account_id.as_deref() { continue; }
            error_policy::remember_cooldown(service, settings, id, &model, error, &decision);
        }
    }
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
        if decision.kind == ErrorKind::Transient && decision.rule.cooldown == crate::server::config::error_policy::CooldownMode::Protection
            && settings.backoff {
            service.route.backoff_mark_for(account, logging::now_ms(), settings.backoff_seconds, settings.backoff_cap_seconds);
        }
        // ── 失败计数与熔断：任何一次失败都算，不分错误类型 ──────────────
        // 以前只在「连接或服务异常」上计数，于是 401 / 400 / 429 的账号永远不会
        // 真正「熔断」——冷却一到期又回到队首，下一次请求继续撞同一堵墙。用户
        // 的口径是「失败的账号就该熔断，然后轮换到其他供应商」：所以这里把每次
        // 失败都记进去，连续失败到阈值就把这个账号从轮转里摘出去一段时间
        // （`plan_route` 的 `open` 集合），别的家才有机会接活。
        if settings.breaker {
            // 登录失效一次就熔断：401 是「上游不认这个登录态」，续期又没救回来，
            // 再试一次只会再 401 一次 —— 立刻摘出轮转，别等攒够阈值。
            let threshold = if decision.kind == ErrorKind::Auth { 1 } else { settings.breaker_failures as i64 };
            let opened = service.route.breaker_failure(
                account, threshold, settings.breaker_seconds as i64, logging::now_ms());
            if opened {
                let label = target.account.as_ref().map(super::account_display).unwrap_or_default();
                logging::console_line("[Upstream]", &format!(
                    "账号「{label}」失败达 {threshold} 次，熔断 {} 秒后才会重新进入轮转", settings.breaker_seconds));
                ctx.telemetry.note_error_policy(json!({ "reason": "账号连续失败，已熔断" }));
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
