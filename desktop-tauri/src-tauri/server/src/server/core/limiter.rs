//! 账号「限制器」：每账号可自定义的限制规则列表（余额 / Token 两类）与
//! Token 周期消耗的内存事实表。
//!
//! ── 这个模块解决什么问题 ─────────────────────────────────────
//! 旧的「余额不足处理」每账号只有一组写死的配置（`lowBalance {mode, threshold}`），
//! 表达不了「这账号每小时最多用 50 万 Token」。限制器把它升级成规则数组
//! （账号记录上的 `limiters` 键）：每条规则 = 类型（余额 / Token）+ 阈值 +
//! 触发动作（跳过 / 禁用）+ 启用开关，Token 规则另带**重置方式** —— 固定周期
//! （对齐自然时间的固定窗口，30 分钟 ~ 24 小时）或自然日（每天本地时区 0 点
//! 重置，有的账号过了 0 点额度就回来）。
//!
//! ── 与旧 `lowBalance` 的关系（读侧推导 + 写侧同步）──────────
//! 记录上**没有** `limiters` 键时（旧版本写入的记录），有效规则由
//! `lowBalance` / provider 缺省**推导**出至多一条余额规则 —— 行为与升级前
//! 完全一致（见 [`effective_rules`]）。写入侧（`store_crud::apply_patch`）
//! 保存 `limiters` 时顺手把首条余额规则回写进 `lowBalance`：用户降级回旧版
//! 时余额限制不丢（Token 规则旧版不认识，忽略即可）。
//!
//! ── Token 周期消耗怎么算 ────────────────────────────────────
//! 「当前窗口内消耗了多少」从 `requests` 表按账号聚合
//! （`SUM(total_tokens) WHERE ts >= 窗口起点`，schema v9 的复合索引撑着）。
//! 聚合结果住**内存事实表**（`TokenFact`，键 = 账号 × 周期秒数），两个写入口：
//!   - **全量刷新**：`usage_query` 的心跳循环每轮（10 秒）把当前启用中的全部
//!     Token 周期重算一遍，整表替换 —— 窗口翻页（对齐自然时间）后旧读数
//!     自动归零；
//!   - **增量记账**：请求收尾（`api::pipeline::record_entry`）把本次消耗当场
//!     加进当前窗口 —— 选路跳过的判定不必等下一轮刷新，转发热路径上没有 IO。
//!
//! 选路跳过（[`token_skip_blocked`]）与余额跳过同一形态：读内存、零 IO、
//! 严格按「拿得出证据才拦」—— 窗口读数缺（事实表没有这个键、或窗口已翻页）
//! 按 0 算，不拦。
//!
//! ── 硬约束：持锁（连接锁）期间不打日志 ──────────────────────
//! 与 `usage_records` 同一条：`Db::with*` 拿的是全局唯一的连接锁，闭包里打
//! 日志会当场死锁。日志一律在闭包之外打。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::server::core::account_store::store_util::js_truthy;
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::core::usage_records::{default_low_balance_mode, DEFAULT_LOW_BALANCE_THRESHOLD};
use crate::server::db::Db;
use crate::server::logging;

/// Cline 免费池的 provider id（与 `usage_records::CLINE_FREE_PROVIDER` 同源推导）。
const CLINE_FREE_PROVIDER: &str = kind_id(ProviderKind::ClineFree);
/// CodeArts 的 provider id：它家的缺省限制按 Token（见 [`default_rules`]）。
const CODEARTS_PROVIDER: &str = kind_id(ProviderKind::CodeArts);

/// CodeArts 缺省 Token 规则的阈值：**每日** 1000 万 Token（自然日 0 点重置）。
pub const DEFAULT_DAILY_TOKEN_LIMIT: f64 = 10_000_000.0;

/// Token 规则重置周期的上下限（秒）：30 分钟 ~ 24 小时。与写入侧
/// （`store_crud::apply_patch`）共用这一对常量 —— 两处各写一份「迟早会漂」。
/// 下限与查询间隔下限（30 秒）不同：固定窗口按自然时间对齐，窗口太短会让
/// 「刚恢复又被拦」反复出现，30 分钟是可用性与保护性的折中（原型评审定的）。
pub const MIN_TOKEN_PERIOD_SECONDS: i64 = 1_800;
pub const MAX_TOKEN_PERIOD_SECONDS: i64 = 86_400;

static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 装库句柄（`ServerState::bootstrap` 调一次，与 `usage_records::install` 并排）。
pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn database() -> Option<&'static Db> {
    DB.get().and_then(Option::as_ref)
}

// ─── 规则的形状与解析 ────────────────────────────────────────

/// 规则类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimiterKind {
    /// 余额：读数低于阈值触发（与旧「余额不足处理」同一口径，严格小于）。
    Balance,
    /// Token：重置窗口内累计消耗达到阈值触发（大于等于）。
    Token,
}

/// Token 规则的重置方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimiterReset {
    /// 固定周期：窗口对齐自然时间（整点 / 整 N 分钟一轮回），长度 = `period`。
    Fixed,
    /// 自然日：每天本地时区 0 点重置（有的账号过了 0 点额度就回来，
    /// 固定周期的「每 N 小时」表达不了这个）。不使用 `period`。
    Daily,
}

/// 触发动作：语义与旧两档一致 —— 跳过自动恢复（余额回升 / 窗口重置），
/// 禁用不自动恢复（需手动启用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimiterAction {
    Skip,
    Disable,
}

/// 一条限制规则（读侧形状；`period` 仅「固定周期」的 Token 规则有效，其余恒 0）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterRule {
    pub kind: LimiterKind,
    pub action: LimiterAction,
    /// 阈值：余额 = 余额数字下限；Token = 窗口内消耗上限。恒 > 0。
    pub threshold: f64,
    /// 重置周期（秒）；仅「固定周期」的 Token 规则有效。
    pub period: i64,
    /// 重置方式；余额规则恒 Fixed（跟随余额读数，无窗口概念）。
    pub reset: LimiterReset,
    pub enabled: bool,
}

impl LimiterRule {
    /// 公开形态的一个规则对象（字段恒齐，前端不必判「键缺失」）。
    /// daily 不带 period：它没有周期长度，只有「每天 0 点」这一个锚点。
    pub fn to_json(self) -> Value {
        let kind = match self.kind {
            LimiterKind::Balance => "balance",
            LimiterKind::Token => "token",
        };
        let action = match self.action {
            LimiterAction::Skip => "skip",
            LimiterAction::Disable => "disable",
        };
        let reset = match self.reset {
            LimiterReset::Fixed => "fixed",
            LimiterReset::Daily => "daily",
        };
        let mut object = json!({
            "type": kind,
            "action": action,
            "threshold": self.threshold,
            "reset": reset,
            "enabled": self.enabled,
        });
        if self.kind == LimiterKind::Token && self.reset == LimiterReset::Fixed {
            object["period"] = Value::from(self.period);
        }
        object
    }
}

/// 从账号记录的 `limiters` 键解析规则（**容错**：脏条目整条丢弃，不让一条
/// 手改的坏数据把其余规则全废掉）。非数组 / 缺失返回 `None` —— 调用方据此
/// 走「旧 lowBalance / provider 缺省」的推导。
fn parse_rules(value: Option<&Value>) -> Option<Vec<LimiterRule>> {
    let entries = value?.as_array()?;
    Some(
        entries
            .iter()
            .filter_map(|entry| {
                let fields = entry.as_object()?;
                let kind = match fields.get("type").and_then(Value::as_str) {
                    Some("balance") => LimiterKind::Balance,
                    Some("token") => LimiterKind::Token,
                    _ => return None,
                };
                let action = match fields.get("action").and_then(Value::as_str) {
                    Some("skip") => LimiterAction::Skip,
                    Some("disable") => LimiterAction::Disable,
                    _ => return None,
                };
                // reset 缺省 = fixed（旧记录没有这个键，行为与升级前一致）
                let reset = match fields.get("reset").and_then(Value::as_str) {
                    Some("daily") => LimiterReset::Daily,
                    _ => LimiterReset::Fixed,
                };
                let threshold = fields.get("threshold").and_then(Value::as_f64)?;
                if !threshold.is_finite() || threshold <= 0.0 {
                    return None;
                }
                // period 只对「固定周期」的 Token 规则要求；越界（手改脏值）整条
                // 丢弃，不夹逼 —— 「把 90 天夹成 24 小时」会悄悄改变用户配好的
                // 语义，宁可不生效。daily 规则没有周期长度，period 恒 0。
                let period = if kind == LimiterKind::Token && reset == LimiterReset::Fixed {
                    let period = fields.get("period").and_then(Value::as_i64)?;
                    if !(MIN_TOKEN_PERIOD_SECONDS..=MAX_TOKEN_PERIOD_SECONDS).contains(&period) {
                        return None;
                    }
                    period
                } else {
                    0
                };
                let enabled = fields.get("enabled").map(js_truthy).unwrap_or(true);
                Some(LimiterRule { kind, action, threshold, period, reset, enabled })
            })
            .collect(),
    )
}

/// 记录字段上**没有** `limiters` 键时的推导：由 `lowBalance`（显式配置）或
/// provider 缺省档得出至多一条余额规则 —— 与限制器上线前的行为逐字一致。
/// 显式 `limiters`（含空数组）原样尊重：空数组 = 用户删光了规则 = 不限制。
/// 各 provider 的**缺省限制规则**（记录上没有 `limiters` 也没有显式 `lowBalance`
/// 配置时用）。
///
/// - **CodeArts → Token 自然日 1000 万，跳过**：这一家的余额读数常判不出
///   （免费版账号的统计里没有积分类计量表、`available` 为 null，额度全在福利
///   token 池），缺省的余额规则形同虚设；按自然日 Token 消耗判定才兜得住
///   「别把一个账号打爆」，而「过了 0 点额度就回来」正是它的额度形态。
/// - **Cline 免费池 → 无规则**：免费池余额贴 0 / 欠费是常态，原「不处理」
///   缺省的理由不变。
/// - **其余 provider → 余额 < 1 跳过**（历史缺省不变）。
pub fn default_rules(provider: &str) -> Vec<LimiterRule> {
    if provider == CODEARTS_PROVIDER {
        return vec![LimiterRule {
            kind: LimiterKind::Token,
            action: LimiterAction::Skip,
            threshold: DEFAULT_DAILY_TOKEN_LIMIT,
            period: 0,
            reset: LimiterReset::Daily,
            enabled: true,
        }];
    }
    if provider == CLINE_FREE_PROVIDER {
        return Vec::new();
    }
    vec![LimiterRule {
        kind: LimiterKind::Balance,
        action: LimiterAction::Skip,
        threshold: DEFAULT_LOW_BALANCE_THRESHOLD,
        period: 0,
        reset: LimiterReset::Fixed,
        enabled: true,
    }]
}

/// 记录字段上**没有** `limiters` 键时的推导：有显式 `lowBalance`（用户 / 旧版
/// 配过）就原样尊重；两者都没有时按 provider 给缺省规则（[`default_rules`]）——
/// CodeArts 从这里拿到的是一条 Token 自然日规则而不是余额规则。
fn derive_from_legacy(fields: &serde_json::Map<String, Value>) -> Vec<LimiterRule> {
    let provider = fields
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
    let Some(config) = fields.get("lowBalance") else {
        return default_rules(provider);
    };
    let mode = config
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or(default_low_balance_mode(provider));
    let threshold = config
        .get("threshold")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(DEFAULT_LOW_BALANCE_THRESHOLD);
    let (kind, action) = match mode {
        "skip" => (LimiterKind::Balance, LimiterAction::Skip),
        "disable" => (LimiterKind::Balance, LimiterAction::Disable),
        _ => return Vec::new(),
    };
    vec![LimiterRule { kind, action, threshold, period: 0, reset: LimiterReset::Fixed, enabled: true }]
}

/// 这条账号的**有效限制规则**：显式 `limiters` 优先，缺失时按旧配置推导。
/// 选路过滤（`rotate`）、自动禁用钩子（`usage_query`）、公开形态
/// （`store_view`）与写入侧比较基准（`store_crud`）共用这一个入口 ——
/// 「什么算一条规则」只有这一份答案。字段映射版（账号记录的 fields 直取）。
pub fn effective_rules_in(fields: &serde_json::Map<String, Value>) -> Vec<LimiterRule> {
    match parse_rules(fields.get("limiters")) {
        Some(rules) => rules,
        None => derive_from_legacy(fields),
    }
}

/// [`effective_rules_in`] 的账号 JSON 版（选路 / 钩子手里是整个账号对象）。
pub fn effective_rules(account: &Value) -> Vec<LimiterRule> {
    match account.as_object() {
        Some(fields) => effective_rules_in(fields),
        // 形态异常（非对象）的账号没有任何可判定的配置：不给规则（不拦），
        // 与「拿不出证据就不拦」的总体口径一致
        None => Vec::new(),
    }
}

/// 规则列表 → 公开形态的 JSON 数组（写入侧存放 / 比较共用）。
pub fn rules_to_json(rules: &[LimiterRule]) -> Value {
    Value::Array(rules.iter().map(|rule| rule.to_json()).collect())
}

/// 有效规则 → 公开形态的 JSON 数组（`store_view` 注入 / 写入侧比较共用）。
pub fn effective_rules_json_in(fields: &serde_json::Map<String, Value>) -> Value {
    rules_to_json(&effective_rules_in(fields))
}

/// 写入侧的严格归一化（校验即权威）：每条都必须是完整合法的规则对象，
/// 否则整条报错（不静默丢 —— 用户在界面上一条条配的，配错了要让他看见）。
/// 非数组按「恢复缺省」处理（返回推导规则），与 `lowBalance` 非对象同一形态。
pub fn normalize_rules(
    value: &Value,
    fields: &serde_json::Map<String, Value>,
) -> Result<Vec<LimiterRule>, String> {
    let Some(entries) = value.as_array() else {
        return Ok(effective_rules_in(fields));
    };
    let mut rules = Vec::new();
    for entry in entries {
        let Some(fields) = entry.as_object() else {
            return Err("限制规则的每一项都必须是对象".to_string());
        };
        let kind = match fields.get("type").and_then(Value::as_str) {
            Some("balance") => LimiterKind::Balance,
            Some("token") => LimiterKind::Token,
            _ => return Err("限制规则的类型必须是「余额 / Token」之一".to_string()),
        };
        let action = match fields.get("action").and_then(Value::as_str) {
            Some("skip") => LimiterAction::Skip,
            Some("disable") => LimiterAction::Disable,
            _ => return Err("限制规则的触发动作必须是「跳过 / 禁用」之一".to_string()),
        };
        let threshold = fields
            .get("threshold")
            .and_then(Value::as_f64)
            .ok_or_else(|| "限制阈值必须是大于 0 的数字".to_string())?;
        if !threshold.is_finite() || threshold <= 0.0 {
            return Err("限制阈值必须是大于 0 的数字".to_string());
        }
        // reset 缺省 = fixed（前端旧草稿不带这个键也能存）；只认两值，不猜
        let reset = match fields.get("reset").and_then(Value::as_str) {
            None | Some("fixed") => LimiterReset::Fixed,
            Some("daily") => LimiterReset::Daily,
            Some(_) => return Err("Token 重置方式必须是「固定周期 / 自然日」之一".to_string()),
        };
        let period = if kind == LimiterKind::Token && reset == LimiterReset::Fixed {
            let period = fields.get("period").and_then(Value::as_i64).ok_or_else(|| {
                "Token 规则必须带重置周期（整数秒）".to_string()
            })?;
            if !(MIN_TOKEN_PERIOD_SECONDS..=MAX_TOKEN_PERIOD_SECONDS).contains(&period) {
                return Err(format!(
                    "Token 重置周期必须是 {} ~ {} 秒（30 分钟 ~ 24 小时）",
                    MIN_TOKEN_PERIOD_SECONDS, MAX_TOKEN_PERIOD_SECONDS
                ));
            }
            period
        } else {
            0
        };
        let enabled = fields.get("enabled").map(js_truthy).unwrap_or(true);
        rules.push(LimiterRule { kind, action, threshold, period, reset, enabled });
    }
    Ok(rules)
}

// ─── 判定（选路 / 自动禁用共用的口径）───────────────────────

/// 余额跳过判定：任一启用的余额 skip 规则满足「读数严格小于阈值」即拦。
/// `facts` 是 `usage_records::balance_facts()` 的内存读数（判不出 = 放行，
/// 那条口径不变）。取代旧版按 `lowBalance.mode` 单配置的读法 —— 推导规则
/// 让旧记录拿到同一个结论。
pub fn balance_skip_blocked(account: &Value, facts: &HashMap<String, crate::server::core::usage_records::BalanceFact>) -> bool {
    let Some(id) = account.get("id").and_then(Value::as_str) else { return false };
    let fact = facts.get(id);
    effective_rules(account).iter().any(|rule| {
        rule.kind == LimiterKind::Balance
            && rule.action == LimiterAction::Skip
            && rule.enabled
            && fact.is_some_and(|fact| matches!(fact.remaining, Some(remaining) if remaining < rule.threshold))
    })
}

/// 余额自动禁用的阈值：任一启用的余额 disable 规则满足「读数 < 阈值」就该禁，
/// 等价于取其中**最大**的阈值（`remaining < max` ⇔ 至少一条命中）。
/// `None` = 没有这条规则（缺省不启用禁用的口径不变）。
pub fn balance_disable_threshold(account: &Value) -> Option<f64> {
    effective_rules(account)
        .iter()
        .filter(|rule| {
            rule.kind == LimiterKind::Balance && rule.action == LimiterAction::Disable && rule.enabled
        })
        .map(|rule| rule.threshold)
        .fold(None, |max: Option<f64>, threshold| {
            Some(match max {
                Some(current) if current >= threshold => current,
                _ => threshold,
            })
        })
}

/// 当前时刻所在窗口的起点（毫秒）：对齐自然时间（整点 / 整 N 分钟一轮回）。
/// 固定窗口的「重置」因此可预测（界面能给出倒计时）。
pub fn window_start(now_ms: i64, period_seconds: i64) -> i64 {
    let period_ms = period_seconds * 1000;
    now_ms - now_ms.rem_euclid(period_ms)
}

/// 本地时区**当日 0 点**的毫秒值（自然日窗口的锚点）。
///
/// 时区口径与 `request_stats::clock` / 签到中心同一套（`chrono::Local`）：用户
/// 心智里的「过了 0 点就重置」是本机时区的 0 点。DST 模糊时段取 `single()`
/// 失败时回落整点对齐（极端情形读数偏一小时，好过 panic —— release 无 unwrap）。
fn local_midnight_ms(now_ms: i64) -> i64 {
    use chrono::{Local, TimeZone};
    let Some(local) = Local.timestamp_millis_opt(now_ms).single() else {
        return window_start(now_ms, 3600);
    };
    let midnight = local
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single());
    match midnight {
        Some(midnight) => midnight.timestamp_millis(),
        // DST 跳变日 0 点不存在（极罕见）：回落到该时刻所在的小时窗
        None => window_start(now_ms, 3600),
    }
}

/// 规则 → 事实表 / 读数行的**窗口种类键**：固定周期 = 周期秒数（最小 1800，
/// 不会撞 0），自然日 = 0。同键的规则共享同一次聚合与同一条读数。
fn rule_kind(rule: &LimiterRule) -> i64 {
    match rule.reset {
        LimiterReset::Daily => 0,
        LimiterReset::Fixed => rule.period,
    }
}

/// 种类键 → 当前窗口起点（毫秒）：0 = 本地自然日 0 点，其余按固定周期对齐。
fn kind_window_start(kind: i64, now_ms: i64) -> i64 {
    if kind == 0 {
        local_midnight_ms(now_ms)
    } else {
        window_start(now_ms, kind)
    }
}

/// Token 跳过判定：任一启用的 Token skip 规则在**当前窗口**内消耗达到阈值即拦。
/// 窗口读数缺（事实表没有 / 窗口已翻页）按 0 算 —— 拦是对「这个账号这个窗口
/// 已经用超」的断言，拿不出证据就不拦（与余额跳过同一句口径）。
pub fn token_skip_blocked(
    account: &Value,
    facts: &HashMap<(String, i64), TokenFact>,
    now_ms: i64,
) -> bool {
    let Some(id) = account.get("id").and_then(Value::as_str) else { return false };
    effective_rules(account).iter().any(|rule| {
        if rule.kind != LimiterKind::Token || rule.action != LimiterAction::Skip || !rule.enabled {
            return false;
        }
        let start = kind_window_start(rule_kind(&rule), now_ms);
        let used = facts
            .get(&(id.to_string(), rule_kind(&rule)))
            .filter(|fact| fact.window_start == start)
            .map(|fact| fact.used)
            .unwrap_or(0);
        used as f64 >= rule.threshold
    })
}

/// Token 自动禁用的判定（自动禁用钩子用）：返回命中「当前窗口消耗 ≥ 阈值」
/// 的规则与实际消耗（打日志要把「用了多少 / 上限多少」说全）。同一账号多条
/// 命中时返回阈值最小的那条 —— 报告「最先压线的那条」最接近用户的心智。
pub fn token_disable_hit(
    account: &Value,
    facts: &HashMap<(String, i64), TokenFact>,
    now_ms: i64,
) -> Option<(LimiterRule, i64)> {
    let id = account.get("id").and_then(Value::as_str)?;
    let hits: Vec<(LimiterRule, i64)> = effective_rules(account)
        .into_iter()
        .filter(|rule| rule.kind == LimiterKind::Token && rule.action == LimiterAction::Disable && rule.enabled)
        .filter_map(|rule| {
            let start = kind_window_start(rule_kind(&rule), now_ms);
            let used = facts
                .get(&(id.to_string(), rule_kind(&rule)))
                .filter(|fact| fact.window_start == start)
                .map(|fact| fact.used)
                .unwrap_or(0);
            (used as f64 >= rule.threshold).then_some((rule, used))
        })
        .collect();
    hits.into_iter()
        .min_by(|(a, _), (b, _)| a.threshold.partial_cmp(&b.threshold).unwrap_or(std::cmp::Ordering::Equal))
}

/// 秒数 → 人能读的周期 / 间隔文案（`每 90 分钟` 这类）：整小时 / 整分钟进位，
/// 其余按秒。变更提示（store_crud）与自动禁用日志（usage_query）共用这一份，
/// 与前端的 `formatIntervalSeconds` 同一口径。
pub fn describe_period(seconds: i64) -> String {
    if seconds > 0 && seconds % 3600 == 0 {
        format!("{} 小时", seconds / 3600)
    } else if seconds > 0 && seconds % 60 == 0 {
        format!("{} 分钟", seconds / 60)
    } else {
        format!("{seconds} 秒")
    }
}

/// 一条规则 → 人能读的一句话（变更提示用）。f64 的 Display 打整数不带小数点
/// （`100.0` → `100`），与余额列的数字口径一致。
pub fn describe_rule(rule: &LimiterRule) -> String {
    let action = match rule.action {
        LimiterAction::Skip => "跳过",
        LimiterAction::Disable => "禁用",
    };
    match rule.kind {
        LimiterKind::Balance => format!("余额 < {} 时{action}", rule.threshold),
        // 自然日不带周期：「每日」本身就是窗口说明
        LimiterKind::Token if rule.reset == LimiterReset::Daily => {
            format!("每日消耗 ≥ {} Token 时{action}", rule.threshold)
        }
        LimiterKind::Token => format!(
            "每{}消耗 ≥ {} Token 时{action}",
            describe_period(rule.period),
            rule.threshold
        ),
    }
}

/// 规则列表 → 变更提示的一句话。
pub fn describe_rules(rules: &[LimiterRule]) -> String {
    if rules.is_empty() {
        return "限制器 → 无规则（不限制）".to_string();
    }
    let (enabled, disabled): (Vec<&LimiterRule>, Vec<&LimiterRule>) =
        rules.iter().partition(|rule| rule.enabled);
    let mut parts: Vec<String> = enabled.iter().map(|rule| describe_rule(rule)).collect();
    if !disabled.is_empty() {
        parts.push(format!("{} 条已停用", disabled.len()));
    }
    format!("限制器 → {}", parts.join("；"))
}

// ─── Token 周期消耗的内存事实表 ──────────────────────────────

/// 一个 (账号, 周期) 的窗口读数。
#[derive(Clone, Copy, Debug)]
pub struct TokenFact {
    /// 这个读数所属窗口的起点（毫秒）。与当前窗口对不上 = 旧窗口的读数，
    /// 判定按 0 算（窗口已翻页、新窗口还没刷新到）。
    pub window_start: i64,
    /// 窗口内的累计消耗（`requests.total_tokens` 之和）。
    pub used: i64,
}

fn facts_slot() -> &'static Mutex<HashMap<(String, i64), TokenFact>> {
    static FACTS: OnceLock<Mutex<HashMap<(String, i64), TokenFact>>> = OnceLock::new();
    FACTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Token 事实快照（选路 / 快照接口读；调用方克隆一份小 Map）。
pub fn token_facts() -> HashMap<(String, i64), TokenFact> {
    facts_slot()
        .lock()
        .map(|facts| facts.clone())
        .unwrap_or_default()
}

/// 启用中的 Token 规则的**去重窗口种类**集合（全量刷新按它跑 —— 固定周期同
/// 周期 / 全部自然日规则共享一次聚合查询；自然日的种类键是 0）。
pub fn enabled_token_kinds(accounts: &[Value]) -> Vec<i64> {
    let mut kinds: Vec<i64> = accounts
        .iter()
        .flat_map(effective_rules)
        .filter(|rule| rule.kind == LimiterKind::Token && rule.enabled)
        .map(|rule| rule_kind(&rule))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

/// 全量刷新：把给定窗口种类在**当前窗口**的每账号消耗重算一遍，整表替换。
///
/// 为什么整表替换而不是增量合并：翻页后的旧窗口读数必须清掉（判定按旧窗口
/// 算会把「刚重置」误判成「已用超」），而「逐条判断哪条过期」比整表重算
/// 更容易漏。心跳循环每 10 秒跑一次，每次按 distinct 种类一条聚合 SQL，
/// 走 schema v9 的 `(account_id, ts)` 索引 —— 常规库规模下是毫秒级小查询。
pub fn refresh_token_facts(kinds: &[i64]) {
    let Some(db) = database() else { return };
    let now = logging::now_ms();
    let mut fresh: HashMap<(String, i64), TokenFact> = HashMap::new();
    for &kind in kinds {
        let start = kind_window_start(kind, now);
        // 闭包返回 Vec（库不可用 / 语句失败给空表 —— 上层按「没消耗」降级，
        // 与余额事实的库不可用口径一致）；持锁期间不打日志。
        let rows = db.with(|conn| {
            let Ok(mut statement) = conn.prepare(
                "SELECT account_id, COALESCE(SUM(total_tokens), 0) FROM requests \
                 WHERE account_id != '' AND ts >= ?1 GROUP BY account_id",
            ) else {
                return Vec::new();
            };
            let Ok(rows) = statement.query_map([start], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            }) else {
                return Vec::new();
            };
            rows.filter_map(|item| item.ok()).collect::<Vec<_>>()
        });
        for (account_id, used) in rows.unwrap_or_default() {
            fresh.insert((account_id, kind), TokenFact { window_start: start, used });
        }
    }
    if let Ok(mut slot) = facts_slot().lock() {
        *slot = fresh;
    }
}

/// 增量记账：请求收尾把本次消耗当场加进当前窗口。
///
/// 事实表里**已有**的 (账号, 种类) 才更新 —— 新配的规则要等下一轮全量刷新
/// 才进表（≤ 10 秒），请求热路径不为它膨胀。窗口在两次请求之间翻页时
/// （固定周期跨过整点 / 自然日跨过 0 点），本次消耗就是新窗口的第一笔
/// （从零开始记）。
pub fn note_request_tokens(account_id: &str, tokens: i64, now_ms: i64) {
    if account_id.is_empty() || tokens <= 0 {
        return;
    }
    let Ok(mut facts) = facts_slot().lock() else { return };
    let kinds: Vec<i64> = facts
        .keys()
        .filter(|(id, _)| id == account_id)
        .map(|(_, kind)| *kind)
        .collect();
    for kind in kinds {
        let start = kind_window_start(kind, now_ms);
        if let Some(fact) = facts.get_mut(&(account_id.to_string(), kind)) {
            if fact.window_start == start {
                fact.used += tokens;
            } else {
                fact.window_start = start;
                fact.used = tokens;
            }
        }
    }
}

/// 单个账号的 Token 读数 → 快照出口的 JSON 行（每条启用中的 Token 规则窗口
/// 种类一项，`kind` = 固定周期的秒数或 0（自然日）；事实缺 / 旧窗口按 0 算
/// 但 `windowStart` 恒是**当前**窗口起点，前端的倒计时与「已用多少」因此永远
/// 是同一窗口的语义）。
pub fn token_usage_rows(account: &Value, facts: &HashMap<(String, i64), TokenFact>, now_ms: i64) -> Vec<Value> {
    let id = match account.get("id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return Vec::new(),
    };
    let mut kinds: Vec<i64> = effective_rules(account)
        .into_iter()
        .filter(|rule| rule.kind == LimiterKind::Token && rule.enabled)
        .map(|rule| rule_kind(&rule))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
        .into_iter()
        .map(|kind| {
            let start = kind_window_start(kind, now_ms);
            let used = facts
                .get(&(id.clone(), kind))
                .filter(|fact| fact.window_start == start)
                .map(|fact| fact.used)
                .unwrap_or(0);
            json!({ "kind": kind, "windowStart": start, "used": used })
        })
        .collect()
}
