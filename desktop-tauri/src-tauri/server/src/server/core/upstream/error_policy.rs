use serde_json::{json, Value};

use crate::server::config::error_policy::{self as config, CooldownMode, CooldownScope, ErrorAction, ErrorKind, ErrorRule};
use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::errors::GatewayError;
use crate::server::logging;

#[path = "error_policy_evidence.rs"]
mod evidence;
pub(super) use evidence::{detail, gateway_class};

#[derive(Clone, Debug)]
pub(crate) struct ErrorDecision {
    pub rule: ErrorRule,
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
    pub fn cooldown(&self) -> bool { self.rule.cooldown != CooldownMode::None && !self.hard_stop }
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
    let settings = &snapshot.settings;
    let code = evidence::machine_code(body);
    let mut kind = evidence::kind(provider, class, status, &code);
    // 429/402 里包着的「积分/余额/套餐」按文案还原（见 `evidence::resource_kind`）：
    // 这类是账号级资源耗尽，方向是「排掉这个账号、换下一家」，而不是「等一会儿
    // 在同一账号重试」。不还原的话，路由组会拿同一个账号换着成员模型再撞一次。
    if matches!(kind, ErrorKind::RateLimit | ErrorKind::Quota | ErrorKind::Balance | ErrorKind::Unknown) {
        if let Some(actual) = evidence::resource_kind(body.get("message").and_then(Value::as_str).unwrap_or("")) { kind = actual; }
    }
    let mut rule = settings.resolve(kind, status, &code, provider, model).clone();
    let upstream_scope = evidence::scope(body);
    let mut scope = match (rule.scope, upstream_scope) {
        (_, CooldownScope::Provider) => CooldownScope::Provider,
        (CooldownScope::Auto | CooldownScope::Model, CooldownScope::Account) => CooldownScope::Account,
        (CooldownScope::Auto, _) => upstream_scope,
        (scope, _) => scope,
    };
    let account_model = matches!(code.as_str(), "account_model_unavailable" | "account_model_permissions_unknown" | "model_not_available_for_account");
    // 「请求有误 / 内容被拒」不再写死成硬终止（#644）：默认它们也换号，
    // 因为换一家/换一个账号的审核与参数兼容性仍可能不同。想让它们直接回到
    // 下游，用例外名单（`overrides` 里的 `ReturnError`）显式表达。
    let hard_stop = explicit_stop || evidence::hard_stop(status, &code);
    let mut action = if hard_stop { ErrorAction::ReturnError } else { rule.action };
    // 没有拿到上游响应（status 0）= 链路/本地问题：换账号解决不了，避免把
    // 整个候选队列当成「试错池」白打一遍。
    if status == 0 { action = ErrorAction::ReturnError; }
    // ── 本地就判定「这家吃不下这份请求」───────────────────────
    // 典型是网页工具（`toolEmulation`）按模型预算在**本机**判定的 400：
    // `网页工具请求超过 16000 字符预算`。这类结论与账号无关 —— 同一家的
    // 另一个账号拿到的是同一份 body、同一个预算，撞的是同一堵墙。逐个账号
    // 试过去只会把换号预算烧在同一家身上，让真正能答上来的候选（例如不做
    // 工具模拟的模型）永远轮不到。
    //
    // 所以这里升级成「跳过这家」：本次请求里把这家其余账号一并排除（见
    // `ErrorDecision::skips_provider`），同时给失败的 账号×模型 记一段冷却，
    // 让下一条请求也不会先撞一次。冷却只落在模型维度，不是整家拉黑 ——
    // 这家别的模型仍可能正常。
    let provider_local = matches!(kind, ErrorKind::BadRequest | ErrorKind::Unsupported)
        && (evidence::provider_local(&code) || (local && matches!(status, 400 | 413 | 415 | 422 | 501)));
    if provider_local && action != ErrorAction::ReturnError {
        action = ErrorAction::SwitchProvider;
        if scope == CooldownScope::Provider { scope = CooldownScope::Model; }
        if rule.cooldown == CooldownMode::None { rule.cooldown = CooldownMode::Fixed; }
    }
    if account_model {
        scope = CooldownScope::Model;
        if action == ErrorAction::SwitchProvider { action = ErrorAction::SwitchAccount; }
    } else if kind == ErrorKind::Unsupported && rule.scope == CooldownScope::Auto { scope = CooldownScope::Provider; }
    if matches!(kind, ErrorKind::Auth | ErrorKind::Balance) && rule.scope == CooldownScope::Auto && scope != CooldownScope::Provider {
        scope = CooldownScope::Account;
    }
    // 限流 / 额度 / 余额消耗的是**账号**的池子，不是某个模型的独立额度：冷却和
    // 本次请求的排除都按账号维度走。上游若声成整家共享（Provider）则保留，
    // 其余（含上游写 `scope: model` 的）一律升到账号级 —— 否则同一个账号会
    // 被路由组的不同成员模型各撞一次，日志里就是「连着两个 429 没换模型」。
    if matches!(kind, ErrorKind::RateLimit | ErrorKind::Quota) && scope != CooldownScope::Provider && !account_model {
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
    let exception = settings.overrides.iter().any(|item| item.id == rule.id);
    ErrorDecision { rule, kind, action, scope, reset_at, hard_stop, exception, model: model.to_string(),
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

/// 插件报的 `stop` 是否等于「这家别再原样发」。
///
/// ── `stop` 不等于「整条请求到此为止」（#644，2026-11-21 再收窄一次）──
/// 插件声明 `stop` 的本义是「这份 body 不要再往这家重发」。曾经的实现把它当成
/// 硬终止：账号级失败（被封、限额、WAF、渠道未授权）静默收尾，队列里明明还有
/// 健康账号与别的提供商却一个都不试。实测 21:34 一条 `pro` 组请求就是被
/// WorkBuddy 的 400「unapproved channel」+ `stop_switching` 掐断的，下游拿到
/// 400，而同一时刻另外几家都能答。
///
/// 现在调用方（`error_policy_send`）把这里的返回值解释成 **跳过这一家**
/// （`SwitchProvider`），只有 `evidence::hard_stop` 那类证据判据（重复执行 /
/// 重复计费 / 已取消）才算真正的硬终止 —— 它由 [`decide_with`] 自己按证据
/// 打开，与本函数的返回值无关。
///
/// `suspend_account` 不进这里 —— 停用当前账号之后，正该换下一个账号继续。
pub(super) fn plugin_stops(_provider: &str, policy: &crate::server::core::providers::policy::ErrorPolicy,
    _class: &UpstreamErrorClass, status: u16, body: &Value) -> bool {
    if !policy.stop { return false; }
    let code = evidence::machine_code(body);
    policy.stop_switching || evidence::hard_stop(status, &code)
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
    telemetry.note_error_policy(json!({ "ruleId": decision.rule.id, "ruleName": decision.rule.name,
        "kind": decision.kind, "action": decision.action, "scope": decision.scope,
        "resetAt": decision.reset_at, "timeSource": if !decision.cooldown() { "none" } else if decision.reset_at.is_some() { "upstream" } else { "fallback" },
        "exception": decision.exception, "reason": reason.unwrap_or(action) }));
}

pub(super) fn remember_cooldown(service: &super::UpstreamService, settings: &config::ErrorSettings, account_id: &str, model: &str,
    error: &GatewayError, decision: &ErrorDecision) -> Option<Value> {
    if account_id.is_empty() { return None; }
    let rule = decision.cooldown_rule(settings)?;
    crate::server::core::blocking::in_place(|| service.store.mark_policy_cooldown(account_id, model, decision.scope,
        error.status_code, error.upstream_code, decision.reset_at, &error.message, &rule, decision.floor_ms))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::config::error_policy::{
        default_rules, CooldownMode, ErrorAction, ErrorKind, ErrorPolicySnapshot, ErrorSettings,
    };

    fn snapshot() -> ErrorPolicySnapshot {
        ErrorPolicySnapshot { settings: ErrorSettings::default(), configured: true, legacy: false }
    }

    fn fatal(status: u16, message: &str) -> UpstreamErrorClass {
        UpstreamErrorClass::Fatal { status, message: message.into(), upstream_code: None }
    }

    fn decide(status: u16, body: serde_json::Value) -> ErrorDecision {
        decide_with(&snapshot(), "demo", "demo-model", &fatal(status, "upstream said no"), status, &body, false, false)
    }

    /// 429 默认就是「冷却这个账号 × 模型，然后换下一个候选」。
    #[test]
    fn rate_limit_switches_and_cools_down() {
        let decision = decide(429, serde_json::json!({ "message": "too many requests" }));
        assert_eq!(decision.kind, ErrorKind::RateLimit);
        assert_eq!(decision.action, ErrorAction::SwitchAccount);
        assert!(decision.switches() && decision.cooldown());
    }

    /// 上游 403（被封 / 无权）属于账号级结论，必须顺延而不是收尾。
    #[test]
    fn forbidden_switches_instead_of_stopping() {
        let decision = decide(403, serde_json::json!({ "code": "forbidden" }));
        assert!(!decision.hard_stop, "403 不再算硬终止");
        assert_eq!(decision.action, ErrorAction::SwitchAccount);
        assert!(decision.switches());
    }

    /// 5xx 不再原地重发，第一次失败就换候选。
    #[test]
    fn transient_does_not_resend_in_place() {
        let decision = decide(502, serde_json::json!({ "message": "bad gateway" }));
        assert_eq!(decision.kind, ErrorKind::Transient);
        assert!(!decision.retries(), "默认不做同一账号原地重发");
        assert_eq!(decision.action, ErrorAction::SwitchAccount);
    }

    /// 请求本身有误也换号（有别的候选仍可能成功），并留一段短冷却 ——
    /// 「限制器」的意义：刚在这个 账号×模型 上失败过的组合，别让下一条请求
    /// 又抢着试一遍。
    #[test]
    fn bad_request_switches_and_cools_briefly() {
        let decision = decide(400, serde_json::json!({ "code": "invalid_request" }));
        assert_eq!(decision.kind, ErrorKind::BadRequest);
        assert_eq!(decision.action, ErrorAction::SwitchAccount);
        assert!(decision.switches());
        assert!(decision.cooldown(), "请求有误也要落一段冷却，避免反复挑中同一个坏组合");
        assert_eq!(decision.scope, CooldownScope::Model, "冷却只落在模型维度，不整家拉黑");
    }

    /// 例外名单（旧「指定错误码」键，语义已反转为「允许透传下游」）命中时
    /// 不再换号，直接把错误交回客户端。
    #[test]
    fn passthrough_exception_returns_error() {
        let mut settings = ErrorSettings::default();
        let mut rule = default_rules().into_iter().find(|rule| rule.kind == ErrorKind::BadRequest).unwrap();
        rule.id = "legacy-400".into();
        rule.name = "原错误码名单例外 400".into();
        rule.statuses = vec![400];
        rule.action = ErrorAction::ReturnError;
        rule.cooldown = CooldownMode::None;
        settings.overrides.push(rule);
        let snapshot = ErrorPolicySnapshot { settings, configured: true, legacy: false };
        let decision = decide_with(&snapshot, "demo", "demo-model",
            &fatal(400, "upstream said no"), 400, &serde_json::json!({ "code": "invalid_request" }), false, false);
        assert_eq!(decision.action, ErrorAction::ReturnError);
        assert!(!decision.switches(), "命中例外就不该再换号");
    }

    /// 网页工具预算超限是**本地**按模型 profile 判定的 400：同一家的另一个
    /// 账号拿到的是同一份 body、同一个预算，撞的是同一堵墙。所以要直接跳过
    /// 这一家（而不是把预算逐个账号烧下去），并给失败的 账号×模型 记冷却。
    #[test]
    fn web_tools_budget_error_skips_provider() {
        let decision = decide(400, serde_json::json!({ "code": "web_tools_invalid_request" }));
        assert_eq!(decision.action, ErrorAction::SwitchProvider);
        assert!(decision.switches(), "跳过这一家也要继续顺延");
        assert!(decision.skips_provider(), "本次请求不该再试这一家的其他账号");
        assert!(decision.cooldown(), "失败组合要落冷却，别让下一条请求再撞一次");
        assert_eq!(decision.scope, CooldownScope::Model);
    }

    /// 真正「本地构造就失败」的 400（`local = true`）同样按跳过这家处理。
    #[test]
    fn local_build_failure_skips_provider() {
        let decision = decide_with(&snapshot(), "demo", "demo-model",
            &fatal(400, "网页工具请求超过 16000 字符预算"), 400, &serde_json::json!({}), true, false);
        assert_eq!(decision.action, ErrorAction::SwitchProvider);
        assert!(decision.skips_provider());
    }

    /// 429 里包着的「积分余额不足」（聚合上游常把它套在 429 上）是账号级资源
    /// 耗尽：按账号冷却，本次请求把这个账号的其余候选一起排掉 —— 否则路由组会
    /// 拿同一个账号换着成员模型再撞一次，日志里就是连着两条 429。
    #[test]
    fn balance_wrapped_in_429_cools_account() {
        let decision = decide(429, serde_json::json!({ "message": "上游返回 429: 上游返回 402: 积分余额不足，请获取积分后再试" }));
        assert_eq!(decision.kind, ErrorKind::Balance);
        assert_eq!(decision.action, ErrorAction::SwitchAccount);
        assert_eq!(decision.scope, CooldownScope::Account, "账号级资源耗尽要按账号冷却");
        assert!(decision.switches() && decision.cooldown());
    }

    /// 「套餐池没有有效套餐」同理：账号级额度，不是某一个模型的额度。
    #[test]
    fn subscription_quota_cools_account() {
        let decision = decide(429, serde_json::json!({ "message": "ZCode 套餐池当前没有有效套餐（1005）" }));
        assert_eq!(decision.kind, ErrorKind::Quota);
        assert_eq!(decision.scope, CooldownScope::Account);
        assert!(decision.cooldown());
    }

    /// 真正的频率限制也按账号冷却（限流消耗的是账号的池子）。
    #[test]
    fn rate_limit_cools_account() {
        let decision = decide(429, serde_json::json!({ "message": "too many requests, rate limit exceeded" }));
        assert_eq!(decision.kind, ErrorKind::RateLimit);
        assert_eq!(decision.scope, CooldownScope::Account);
    }

    /// 但上游明确声明整家共享冷却时保留 Provider，不缩到账号级。
    #[test]
    fn provider_scope_is_preserved_for_rate_limit() {
        let decision = decide(429, serde_json::json!({ "message": "rate limit exceeded", "credentialScope": "provider" }));
        assert_eq!(decision.scope, CooldownScope::Provider);
    }

    /// 没有拿到上游响应（status 0）是链路问题：换账号解决不了，保持收尾。
    #[test]
    fn transport_failure_does_not_switch() {
        let decision = decide(0, serde_json::json!({ "message": "connect error" }));
        assert_eq!(decision.action, ErrorAction::ReturnError);
        assert!(!decision.switches());
    }

    /// 插件报 `stop` 只表示「别再原地重发」，不再顺手掐掉换号。
    #[test]
    fn plugin_stop_does_not_block_switching() {
        let policy = crate::server::core::providers::policy::ErrorPolicy {
            stop: true, stop_switching: false, account_cooldown_until: None, suspend_account: false,
            reason: String::new(),
        };
        assert!(!plugin_stops("demo", &policy, &fatal(503, "busy"), 503, &serde_json::json!({})));
        let explicit = crate::server::core::providers::policy::ErrorPolicy {
            stop: true, stop_switching: true, account_cooldown_until: None, suspend_account: false,
            reason: String::new(),
        };
        assert!(plugin_stops("demo", &explicit, &fatal(503, "busy"), 503, &serde_json::json!({})));
    }
}
