use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind { RateLimit, Quota, Auth, Balance, Transient, Unsupported, BadRequest, Content, Unknown }

impl ErrorKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::RateLimit => "rate_limit", Self::Quota => "quota", Self::Auth => "auth", Self::Balance => "balance",
            Self::Transient => "transient", Self::Unsupported => "unsupported", Self::BadRequest => "bad_request",
            Self::Content => "content", Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorAction { SwitchAccount, RetryThenSwitch, RefreshThenSwitch, SwitchProvider, ReturnError }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CooldownMode { Upstream, Fixed, Protection, None }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CooldownScope { Auto, Model, Account, Provider }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorRule {
    pub id: String,
    pub name: String,
    pub kind: ErrorKind,
    #[serde(default)]
    pub statuses: Vec<u16>,
    #[serde(default)]
    pub code: String,
    #[serde(default = "all_providers")]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub action: ErrorAction,
    pub cooldown: CooldownMode,
    pub fallback_seconds: u64,
    #[serde(default)]
    pub exponential: bool,
    pub fallback_cap_seconds: u64,
    pub scope: CooldownScope,
}

fn all_providers() -> String { "*".into() }
fn enabled() -> bool { true }

/// 换账号上限的上界：与转发层 `MAX_ROUTE_ATTEMPTS` 对齐。
///
/// 路由的意义是「试到有候选能答上来为止」，所以默认值取一个足够大的数 ——
/// 队列里有几个可用账号就试几个。上限放到 100（而不是 32）：账号多的部署
/// 一次排队就能排到几十个候选，「配了 100 却只换 32 次」等于又给队列截了尾。
pub const MAX_SWITCHES: u64 = 100;

/// 默认规则集的版本号。
///
/// 规则是**出厂默认**而不是用户手写的清单：升级到本版之后默认动作统一改成
/// 「换号 / 跳过这一家」，并给请求有误类错误补上短冷却（避免同一个已知坏掉的
/// 账号×模型被下一条请求反复挑中）。旧版本存下来的 `routingErrorPolicy`
/// 里那套 `return_error + 不冷却` 的组合必须跟着升级，否则用户升级了二进制
/// 却还在跑旧口径 —— 见 `config::error_policy::from_raw` 的升级分支。
pub const RULES_VERSION: u64 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorSettings {
    /// 默认规则集的版本；0 = 本字段出现之前的旧配置。
    ///
    /// 这里必须写字段级 `#[serde(default)]`：结构体上已有 `#[serde(default)]`，
    /// 缺字段时会回落到 `ErrorSettings::default()` 的值（= 最新版本），那样旧
    /// 存盘就会被当成「已经是最新口径」而跳过升级。字段级默认取 0 才对。
    #[serde(default)]
    pub rules_version: u64,
    pub resends: u64,
    pub switches: u64,
    pub interval_seconds: u64,
    pub wait_budget_seconds: u64,
    pub cooldown_floor_seconds: u64,
    pub backoff: bool,
    pub backoff_seconds: u64,
    pub backoff_cap_seconds: u64,
    pub sink: bool,
    pub sink_seconds: u64,
    pub breaker: bool,
    pub breaker_failures: u64,
    pub breaker_seconds: u64,
    pub breaker_successes: u64,
    pub rules: Vec<ErrorRule>,
    pub overrides: Vec<ErrorRule>,
}

impl Default for ErrorSettings {
    fn default() -> Self {
        Self {
            rules_version: RULES_VERSION,
            resends: 0, switches: MAX_SWITCHES, interval_seconds: 5, wait_budget_seconds: 30, cooldown_floor_seconds: 30,
            backoff: true, backoff_seconds: 30, backoff_cap_seconds: 300, sink: true, sink_seconds: 1800,
            breaker: false, breaker_failures: 5, breaker_seconds: 60, breaker_successes: 2,
            rules: default_rules(), overrides: Vec::new(),
        }
    }
}

pub fn default_rules() -> Vec<ErrorRule> {
    use CooldownMode::*;
    use ErrorAction::*;
    use ErrorKind::*;
    let rule = |kind: ErrorKind, name: &str, statuses: &[u16], action, cooldown, fallback_seconds, exponential, scope| ErrorRule {
        id: kind.id().into(), name: name.into(), kind, statuses: statuses.to_vec(), code: String::new(), provider: "*".into(),
        model: String::new(), enabled: true, action, cooldown, fallback_seconds, exponential, fallback_cap_seconds: 1800, scope,
    };
    // ── 默认动作统一为「换下一个候选」─────────────────────────
    // 路由的职责是让下游不中断：上游任何一次失败都说明「这个账号 × 这个模型
    // 这一次没答上来」，换一个候选（同家另一个账号、另一家、路由组里的另一个
    // 成员模型）仍可能成功。所以除登录失效（先续期再换）与能力不支持（换家）
    // 之外，全部直接顺延，**不做同一账号的原地重发** —— 那只会让客户端干等，
    // 并且把「上游已经明确拒绝」当成「再试一次也许就好了」。
    //
    // 冷却只给真正属于账号/额度状态的类型：限流、额度、余额、连接或服务异常
    // （退避 + 熔断计数）、登录失效（账号级）。请求本身有误（400/413/422）、
    // 内容被拒（内容策略）与能力不支持也**留一段短冷却**：不是为了惩罚账号，
    // 而是为了让「刚在同一个 账号×模型 上失败过的组合」在接下来的几十秒里别
    // 再被同一条坏请求抢着试一遍 —— 这就是用户要的「限制器」。
    //
    // 要「允许直接把错误返回下游」的码，走「提供商例外」（`overrides`）配置，
    // 见 `config::error_policy` 对旧「指定错误码」名单的映射。
    vec![
        rule(RateLimit, "请求限流", &[429], SwitchAccount, Upstream, 60, false, CooldownScope::Auto),
        rule(Quota, "额度耗尽", &[429], SwitchAccount, Upstream, 600, true, CooldownScope::Auto),
        rule(Auth, "登录失效", &[401], RefreshThenSwitch, None, 60, false, CooldownScope::Account),
        rule(Balance, "余额不足", &[402], SwitchAccount, Upstream, 600, false, CooldownScope::Account),
        rule(Transient, "连接或服务异常", &[408, 500, 502, 503, 504], SwitchAccount, Protection, 60, false, CooldownScope::Auto),
        rule(Unsupported, "能力不支持", &[501], SwitchProvider, Fixed, 60, false, CooldownScope::Model),
        rule(BadRequest, "请求有误", &[400, 413, 422], SwitchAccount, Fixed, 60, false, CooldownScope::Model),
        rule(Content, "内容被拒绝", &[], SwitchAccount, Fixed, 60, false, CooldownScope::Model),
        rule(Unknown, "其他错误", &[], SwitchAccount, Protection, 60, false, CooldownScope::Auto),
    ]
}

impl ErrorSettings {
    pub fn validate(&self) -> Result<(), String> {
        let bounds = [
            (self.resends, 0, 10, "原账号重发次数"), (self.switches, 0, MAX_SWITCHES, "换账号上限"),
            (self.interval_seconds, 0, 300, "重发间隔"), (self.wait_budget_seconds, 0, 300, "累计等待上限"),
            (self.cooldown_floor_seconds, 1, 60, "最短冷却"), (self.backoff_seconds, 1, 3600, "首次失败休息"),
            (self.backoff_cap_seconds, self.backoff_seconds, 3600, "最长失败休息"), (self.sink_seconds, 30, 3600, "降低优先级时长"),
            (self.breaker_failures, 1, 20, "连续失败阈值"), (self.breaker_seconds, 10, 600, "连续失败保护时长"),
            (self.breaker_successes, 1, 10, "恢复成功次数"),
        ];
        for (value, min, max, name) in bounds {
            if !(min..=max).contains(&value) { return Err(format!("{name}必须是 {min}–{max} 的整数")); }
        }
        if self.rules.len() != 9 || self.overrides.len() > 100 { return Err("全局错误规则必须完整，提供商例外最多 100 条".into()); }
        if self.rules_version > RULES_VERSION { return Err("这份设置来自更新的版本，请升级后再保存".into()); }
        let mut ids = std::collections::HashSet::new();
        let mut kinds = std::collections::HashSet::new();
        for (rule, global) in self.rules.iter().map(|rule| (rule, true)).chain(self.overrides.iter().map(|rule| (rule, false))) {
            for (text, max, name) in [(&rule.id, 80, "规则标识"), (&rule.name, 100, "规则名称"), (&rule.provider, 256, "提供商"), (&rule.model, 256, "模型"), (&rule.code, 256, "业务码")] {
                if text.len() > max || text.chars().any(char::is_control) || text.trim() != text { return Err(format!("{name}包含空白、控制字符或超过长度上限")); }
            }
            if rule.id.is_empty() || rule.name.is_empty() || rule.provider.is_empty() || !ids.insert(&rule.id) { return Err("规则标识、名称或提供商为空，或规则标识重复".into()); }
            if global && (rule.id != rule.kind.id() || !rule.enabled || !kinds.insert(rule.kind.id()) || rule.provider != "*" || !rule.model.is_empty() || !rule.code.is_empty()) {
                return Err("全局规则按错误类型唯一，提供商、模型和业务码条件请使用例外规则".into());
            }
            if rule.statuses.len() > 50 || rule.statuses.iter().any(|status| !(400..600).contains(status)) { return Err("错误状态码必须在 400–599 范围，最多 50 项".into()); }
            if !(1..=86400).contains(&rule.fallback_seconds) || !(rule.fallback_seconds..=86400).contains(&rule.fallback_cap_seconds) { return Err("冷却时间必须在 1–86400 秒范围，上限不能短于首次时间".into()); }
            if rule.kind != ErrorKind::Auth && rule.action == ErrorAction::RefreshThenSwitch {
                return Err("仅登录失效规则可以使用凭证续期".into());
            }
            // 请求有误 / 内容被拒：既可以换号（默认）、跳过这一家，也可以透传下游；
            // 冷却只能是「固定时长 / 失败退避 / 不冷却」—— 上游这类响应通常不带
            // `resetAt`，按上游时间冷却等于把账号无限期挂住。
            if matches!(rule.kind, ErrorKind::BadRequest | ErrorKind::Content) && (rule.cooldown == CooldownMode::Upstream
                || !matches!(rule.action, ErrorAction::ReturnError | ErrorAction::SwitchAccount | ErrorAction::SwitchProvider)) {
                return Err("已识别的请求错误和内容拒绝只能选择换账号、跳过该提供商或返回下游，且不能按上游时间冷却".into());
            }
            if rule.kind == ErrorKind::Unknown && !rule.enabled { return Err("其他错误的兜底规则不能关闭".into()); }
        }
        Ok(())
    }

    pub fn resolve(&self, kind: ErrorKind, status: u16, code: &str, provider: &str, model: &str) -> &ErrorRule {
        self.overrides.iter().find(|rule| rule.enabled && rule.kind == kind
            && (rule.provider == "*" || rule.provider == provider) && (rule.model.is_empty() || rule.model == model)
            && (rule.code.is_empty() || rule.code == code) && (rule.statuses.is_empty() || rule.statuses.contains(&status)))
            .or_else(|| self.rules.iter().find(|rule| rule.enabled && rule.kind == kind))
            .or_else(|| self.rules.iter().find(|rule| rule.kind == ErrorKind::Unknown))
            .unwrap_or(&self.rules[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认口径：除登录失效（先续期）与能力不支持（换家）之外，全部换号；
    /// 一次原地重发都不做，换号上限覆盖整条候选队列。
    #[test]
    fn defaults_fail_over_on_every_upstream_error() {
        let settings = ErrorSettings::default();
        assert_eq!(settings.resends, 0, "默认不做同一账号原地重发");
        assert_eq!(settings.switches, MAX_SWITCHES, "默认换号上限要能走完候选队列");
        assert!(settings.validate().is_ok(), "默认设置必须自身合法");
        for rule in &settings.rules {
            match rule.kind {
                ErrorKind::Auth => assert_eq!(rule.action, ErrorAction::RefreshThenSwitch),
                ErrorKind::Unsupported => assert_eq!(rule.action, ErrorAction::SwitchProvider),
                _ => assert_eq!(rule.action, ErrorAction::SwitchAccount, "{} 默认应换号", rule.kind.id()),
            }
        }
    }

    /// 请求有误 / 内容被拒默认也换号（并留短冷却）；显式配成 ReturnError 依然合法。
    #[test]
    fn bad_request_and_content_may_switch_or_passthrough() {
        let settings = ErrorSettings::default();
        assert!(settings.validate().is_ok());
        let mut passthrough = ErrorSettings::default();
        for rule in &mut passthrough.rules {
            if matches!(rule.kind, ErrorKind::BadRequest | ErrorKind::Content) {
                rule.action = ErrorAction::ReturnError;
                rule.cooldown = CooldownMode::None;
            }
        }
        assert!(passthrough.validate().is_ok());
    }

    /// 换号上限的新上界必须容得下默认值。
    #[test]
    fn switch_bounds_cover_default() {
        assert!(MAX_SWITCHES >= 10);
        let mut settings = ErrorSettings::default();
        settings.switches = MAX_SWITCHES;
        assert!(settings.validate().is_ok());
        settings.switches = MAX_SWITCHES + 1;
        assert!(settings.validate().is_err());
    }
}
