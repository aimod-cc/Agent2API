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
    // 面包屑：这一行出现 ⇒ 统一错误规则确实跑到了；没出现 ⇒ 这个错误是某个
    // 早退分支直接抛回下游的（那时明细里的 `errorPolicy` 会是 null）。
    // 用 `log_with_level` 而不是 `console_line`：前者会落进数据库的 `logs` 表，
    // 事后能直接查；`console_line` 只打到 stdout，没截屏就等于丢了。
    logging::log_with_level("[Upstream]", &format!(
        "错误结算 provider={} status={} 模型={} 已试={} 账号={}",
        target.provider, error.status_code,
        target.account.as_ref().and_then(|a| a["routingModel"].as_str()).unwrap_or("-"),
        tried.len(), target.account_id.as_deref().unwrap_or("-")), "info");
    let model = error.routing_error.as_ref().map(|decision| decision.model.clone()).unwrap_or_else(|| {
        target.account.as_ref().map(|account| keys.for_account(account)).unwrap_or_else(|| keys.for_provider(&target.provider))
    });
    let settings = &ctx.error_policy.settings;
    let decision = error.routing_error.clone().unwrap_or_else(|| error_policy::for_error_with(&ctx.error_policy, &target.provider, &model, error, false));
    error_policy::attach(error, &decision);
    error_policy::note(ctx.telemetry, &decision, None);
    // ── 「为什么没有再往下换一个」必须留在控制台 ─────────────────────
    // 以前这些理由只写进 `note_error_policy`（随 attempt 明细落库）。可一旦
    // attempt 记录先被 `finish_last_attempt` 收尾，note 就无处可写 —— 库里的
    // `errorPolicy` 成了 null，面板上只剩「上游 400」，看不出是「规则不让换」
    // 还是「确实没有下一个」。实测 22:49 那条 400 就是这样：账号被铺了冷却
    // （说明这里跑过了），但明细里 policy=null，谁也没法解释为什么停住。
    let model_label = model.clone();
    // 错误正文先取成字符串：闭包不能借用 `error`（后面还要改 `retry_after_secs`）。
    let head = format!("HTTP {} {}", error.status_code, error.message);
    let stop = |reason: &str| {
        ctx.telemetry.note_error_policy(json!({ "reason": reason }));
        logging::log_with_level("[Upstream]", &format!(
            "未继续顺延（{head} · 模型 {model_label}）：{reason}"), "warn");
        false
    };
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
    if decision.hard_stop {
        return stop(&format!("规则「{}」判定为硬终止，不再换对象", decision.rule.name));
    }
    if ctx.telemetry.replay_guard().blocked() || ctx.telemetry.is_cancelled() || ctx.telemetry.response_started() {
        return stop("回放拦截 / 已取消 / 响应已开始下发");
    }
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
        if decision.kind == ErrorKind::Balance {
            crate::server::core::usage_records::force_zero_balance(
                account, "上游报余额不足，余额读数已按 0 处理；充值后自动恢复");
        }
        if decision.kind == ErrorKind::Transient && decision.rule.cooldown == crate::server::config::error_policy::CooldownMode::Protection
            && settings.backoff {
            service.route.backoff_mark_for(account, logging::now_ms(), settings.backoff_seconds, settings.backoff_cap_seconds);
        }
        // ── 失败计数与熔断：任何一次失败都算，但「槽位满」不算 ──────────────
        // 以前只在「连接或服务异常」上计数，于是 401 / 400 / 429 的账号永远不会
        // 真正「熔断」——冷却一到期又回到队首，下一次请求继续撞同一堵墙。用户
        // 的口径是「失败的账号就该熔断，然后轮换到其他供应商」：所以这里把每次
        // 失败都记进去，连续失败到阈值就把这个账号从轮转里摘出去一段时间
        // （`plan_route` 的 `open` 集合），别的家才有机会接活。
        //
        // 例外是 `decision.capacity`（上游会话槽位满了）：账号是健康的，只是
        // 这一刻对话数到顶。把它记成失败，等于「被并发顶一次就摘出去 10 分钟」，
        // 账号池会在高峰期自己塌掉 —— 它只需要那 30 秒冷却先让位。
        if settings.breaker && !decision.capacity {
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
    // ── 兜底总闸：不让「规则动作」决定收尾 ────────────────────────────
    // 用户的诉求只有一句：**不管什么错误，先给当前对象记一次限流，再换下一个**，
    // 只要队列里还有真正可用（有凭证、已启用、没被限制器拦、没在熔断 / 冷却里）
    // 的候选。所以「规则动作是直接返回错误」在这里只留一条日志，不再提前收尾 ——
    // 决定收尾的判据只剩下面那一个：「下一个」确实不存在。
    if !decision.switches() {
        logging::log_with_level("[Upstream]", &format!(
            "规则「{}」本来会直接返回错误（HTTP {}），兜底改为继续换下一个可用对象",
            decision.rule.name, error.status_code), "warn");
    }
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
            return stop("该账号在设置里关了「失败换号」");
        }
    }
    if ctx.pinned_account.is_some() || ctx.routing.requires_account() {
        return stop("本次请求明确指定账号，不允许换对象");
    }
    if decision.skips_provider() {
        error_policy::exclude_provider(service, providers, keys.request_model(), &target.provider, tried, ctx.pinned_account, &ctx.routing);
    }
    if let Some(id) = target.attempt_key() { tried.retain(|other| other != &id); tried.push(id); }
    let Some(next) = rotate::pick_next_account(service, providers, keys, tried, ctx.pinned_account, &ctx.routing) else {
        // 这一句是「池子里到底还有没有下一个」的唯一权威答案：`plan_route`
        // 只记录候选，能不能真的拿出一个对象由这次查找决定。走到这里说明剩下
        // 的候选**确实不可用**（限制器拦下 / 熔断中 / 冷却中 / 已禁用 / 无凭证），
        // 不是规则把它挡了 —— 兜底也不越过这几条。
        return stop(&format!(
            "本队列已没有下一个可用对象（本轮已试 {} 个）：剩下的都在冷却、被限制器拦、熔断中、已禁用或无凭证",
            tried.len()));
    };
    // 换号预算用尽同样不作为收尾理由：预算只是「别一口气试太多」的软闸，
    // 真正的硬闸是转发层的 `MAX_ROUTE_ATTEMPTS`（100 次）。
    if !super::provider_loop::take_switch(switches_left, switch_total) {
        logging::log_with_level("[Upstream]", &format!(
            "换号预算已用尽（上限 {switch_total}），兜底继续换下一个可用对象"), "warn");
    }
    logging::log_with_level("[Upstream]", &format!("统一错误规则「{}」：HTTP {}，顺延到 {}（换号 {}/{}）",
        decision.rule.name, error.status_code, super::account_display(&next), switch_total - *switches_left, switch_total), "info");
    true
}
