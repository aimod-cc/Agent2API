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
            resends: 3, switches: 5, interval_seconds: 5, wait_budget_seconds: 30, cooldown_floor_seconds: 30,
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

impl ErrorSettings {
    pub fn validate(&self) -> Result<(), String> {
        let bounds = [
            (self.resends, 0, 10, "原账号重发次数"), (self.switches, 0, 10, "换账号上限"),
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
            if matches!(rule.kind, ErrorKind::BadRequest | ErrorKind::Content) && (rule.action != ErrorAction::ReturnError || rule.cooldown != CooldownMode::None) {
                return Err("已识别的请求错误和内容拒绝不能设置为重复发送、换号或账号冷却".into());
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
