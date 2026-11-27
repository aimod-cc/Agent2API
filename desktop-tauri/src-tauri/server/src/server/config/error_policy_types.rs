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
pub enum CooldownMode { Upstream, Fixed, Protection, Deprioritize, None }

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginErrorRule {
    #[serde(flatten)]
    pub rule: ErrorRule,
    pub plugin_rule_id: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginErrorClassification {
    pub kind: ErrorKind,
    pub statuses: Vec<u16>,
    pub codes: Vec<String>,
    pub scope: Option<CooldownScope>,
    pub hard_stop: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclaredErrorPolicy {
    pub rules: Vec<PluginErrorRule>,
    pub classifications: Vec<PluginErrorClassification>,
}

/// 换账号上限的上界：与转发层 `MAX_ROUTE_ATTEMPTS` 对齐。
pub const MAX_SWITCHES: u64 = 100;

fn all_providers() -> String { "*".into() }
fn enabled() -> bool { true }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorSettings {
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
            resends: super::DEFAULT_RETRY_COUNT as u64,
            switches: super::DEFAULT_RETRY_SWITCH_COUNT as u64,
            interval_seconds: super::DEFAULT_RETRY_INTERVAL_SECONDS as u64,
            wait_budget_seconds: 30, cooldown_floor_seconds: 30,
            backoff: true, backoff_seconds: 30, backoff_cap_seconds: 300, sink: true, sink_seconds: 1800,
            breaker: false, breaker_failures: 5, breaker_seconds: 60, breaker_successes: 2,
            rules: default_rules(), overrides: Vec::new(),
        }
    }
}

pub fn v1_default_rules() -> Vec<ErrorRule> {
    use CooldownMode::*;
    use ErrorAction::*;
    use ErrorKind::*;
    let rule = |kind: ErrorKind, name: &str, statuses: &[u16], action, cooldown, fallback_seconds, exponential, scope| ErrorRule {
        id: kind.id().into(), name: name.into(), kind, statuses: statuses.to_vec(), code: String::new(), provider: "*".into(),
        model: String::new(), enabled: true, action, cooldown, fallback_seconds, exponential, fallback_cap_seconds: 1800, scope,
    };
    vec![
        rule(RateLimit, "请求限流", &[429], SwitchAccount, Upstream, 60, false, CooldownScope::Auto),
        rule(Quota, "额度耗尽", &[429], SwitchAccount, Upstream, 600, true, CooldownScope::Auto),
        rule(Auth, "登录失效", &[401], RefreshThenSwitch, None, 60, false, CooldownScope::Account),
        rule(Balance, "余额不足", &[402], SwitchAccount, Upstream, 600, false, CooldownScope::Account),
        rule(Transient, "连接或服务异常", &[408, 500, 502, 503, 504], RetryThenSwitch, Protection, 60, false, CooldownScope::Auto),
        rule(Unsupported, "能力不支持", &[501], SwitchProvider, None, 60, false, CooldownScope::Auto),
        rule(BadRequest, "请求有误", &[400, 413, 422], ReturnError, None, 60, false, CooldownScope::Model),
        rule(Content, "内容被拒绝", &[], ReturnError, None, 60, false, CooldownScope::Model),
        rule(Unknown, "其他错误", &[], ReturnError, None, 60, false, CooldownScope::Auto),
    ]
}

pub fn v2_default_balance_rule() -> ErrorRule {
    ErrorRule {
        id: ErrorKind::Balance.id().into(),
        name: "余额不足".into(),
        kind: ErrorKind::Balance,
        statuses: vec![402],
        code: String::new(),
        provider: "*".into(),
        model: String::new(),
        enabled: true,
        action: ErrorAction::SwitchAccount,
        cooldown: CooldownMode::Upstream,
        fallback_seconds: 600,
        exponential: false,
        fallback_cap_seconds: 1800,
        scope: CooldownScope::Account,
    }
}

pub fn default_rules() -> Vec<ErrorRule> {
    use CooldownMode::*;
    use ErrorAction::*;
    use ErrorKind::*;
    let rule = |kind: ErrorKind, name: &str, statuses: &[u16], action, cooldown, fallback_seconds, exponential, fallback_cap_seconds, scope| ErrorRule {
        id: kind.id().into(), name: name.into(), kind, statuses: statuses.to_vec(), code: String::new(), provider: "*".into(),
        model: String::new(), enabled: true, action, cooldown, fallback_seconds, exponential, fallback_cap_seconds, scope,
    };
    vec![
        rule(RateLimit, "请求限流", &[429], SwitchAccount, Upstream, 60, false, 1800, CooldownScope::Auto),
        rule(Quota, "额度耗尽", &[429], SwitchAccount, Upstream, 600, true, 1800, CooldownScope::Auto),
        rule(Auth, "登录失效", &[401], RefreshThenSwitch, Fixed, 300, true, 1800, CooldownScope::Account),
        rule(Balance, "余额不足", &[402], SwitchAccount, Upstream, 600, true, 21_600, CooldownScope::Account),
        rule(Transient, "连接或服务异常", &[408, 500, 502, 503, 504], RetryThenSwitch, Protection, 60, false, 1800, CooldownScope::Auto),
        rule(Unsupported, "能力不支持", &[501], SwitchProvider, Deprioritize, 1800, false, 1800, CooldownScope::Auto),
        rule(BadRequest, "请求有误", &[400, 413, 422], SwitchProvider, Deprioritize, 1800, false, 1800, CooldownScope::Auto),
        rule(Content, "内容被拒绝", &[], SwitchProvider, Deprioritize, 1800, false, 1800, CooldownScope::Provider),
        rule(Unknown, "其他错误", &[], SwitchAccount, Deprioritize, 1800, false, 1800, CooldownScope::Auto),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource { Override, Plugin, Global }

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
        if self.rules.len() != 9 || self.overrides.len() > 200 { return Err("全局错误规则必须完整，提供商例外最多 200 条".into()); }
        let mut ids = std::collections::HashSet::new();
        let mut kinds = std::collections::HashSet::new();
        for (rule, global) in self.rules.iter().map(|rule| (rule, true)).chain(self.overrides.iter().map(|rule| (rule, false))) {
            for (text, max, name) in [(&rule.id, 128, "规则标识"), (&rule.name, 100, "规则名称"), (&rule.provider, 256, "提供商"), (&rule.model, 256, "模型"), (&rule.code, 256, "业务码")] {
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
            if rule.kind == ErrorKind::Unknown && global && !rule.enabled { return Err("其他错误的兜底规则不能关闭".into()); }
        }
        Ok(())
    }

    pub fn resolve(&self, kind: ErrorKind, status: u16, code: &str, provider: &str, model: &str) -> &ErrorRule {
        let codes = if code.is_empty() { Vec::new() } else { vec![code] };
        self.resolve_with(&[], kind, status, &codes, provider, model).0
    }

    pub fn resolve_with<'a>(&'a self, plugin_rules: &'a [PluginErrorRule], kind: ErrorKind, status: u16, codes: &[&str], provider: &str, model: &str) -> (&'a ErrorRule, RuleSource) {
        if let Some(rule) = self.overrides.iter().find(|rule| rule.kind == kind && matches_condition(rule, status, codes, provider, model)) {
            return (rule, RuleSource::Override);
        }
        let mut best: Option<(&PluginErrorRule, (bool, bool, bool))> = None;
        for item in plugin_rules {
            let rule = &item.rule;
            if rule.kind != kind || !matches_condition(rule, status, codes, provider, model) { continue; }
            if self.overrides.iter().any(|over| over.id == rule.id) { continue; }
            let score = (!rule.model.is_empty(), !rule.code.is_empty(), !rule.statuses.is_empty());
            if best.is_none_or(|(_, prev)| score > prev) { best = Some((item, score)); }
        }
        if let Some((item, _)) = best { return (&item.rule, RuleSource::Plugin); }
        let global = self.rules.iter().find(|rule| rule.enabled && rule.kind == kind)
            .or_else(|| self.rules.iter().find(|rule| rule.kind == ErrorKind::Unknown))
            .unwrap_or(&self.rules[0]);
        (global, RuleSource::Global)
    }

    pub fn infer_exception_kind(&self, plugin_rules: &[PluginErrorRule], status: u16, codes: &[&str], provider: &str, model: &str) -> Option<ErrorKind> {
        if let Some(rule) = self.overrides.iter().find(|rule| rule.provider == provider
            && (!rule.code.is_empty() || !rule.statuses.is_empty())
            && matches_condition(rule, status, codes, provider, model)) {
            return Some(rule.kind);
        }
        let mut best: Option<(&PluginErrorRule, (bool, bool, bool))> = None;
        for item in plugin_rules {
            let rule = &item.rule;
            if rule.provider != provider || (rule.code.is_empty() && rule.statuses.is_empty())
                || !matches_condition(rule, status, codes, provider, model) { continue; }
            if self.overrides.iter().any(|over| over.id == rule.id) { continue; }
            let score = (!rule.model.is_empty(), !rule.code.is_empty(), !rule.statuses.is_empty());
            if best.is_none_or(|(_, prev)| score > prev) { best = Some((item, score)); }
        }
        best.map(|(item, _)| item.rule.kind)
    }
}

fn matches_condition(rule: &ErrorRule, status: u16, codes: &[&str], provider: &str, model: &str) -> bool {
    rule.enabled
        && (rule.provider == "*" || rule.provider == provider)
        && (rule.model.is_empty() || rule.model == model)
        && (rule.code.is_empty() || codes.iter().any(|code| *code == rule.code))
        && (rule.statuses.is_empty() || rule.statuses.contains(&status))
}
