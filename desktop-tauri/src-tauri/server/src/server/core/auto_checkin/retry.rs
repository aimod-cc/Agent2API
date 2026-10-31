//! 中性未领取的**重试记账**（`lastResult.retryAccounts`）+ 补签轮的目标集合。
//!
//! ── 两件不同的事，别混 ──────────────────────────────────────
//!   1. **等窗口重试**（本模块的 `due_accounts` / `merge`）：上一轮拿到「中性
//!      未领取」的账号，等插件给出的 `retryAt`（或条目配置的间隔）到了再签一次。
//!      它解决的是「窗口没开 / 上游一时没下发」，与「今天还没签过」无关。
//!   2. **补签轮**（[`unsigned_accounts`]）：今天到点该签的账号里，`checkinAt`
//!      不在今天的那些 —— 主轮因风控 / 网络抖动整批失败时，这一轮把它们捞回来。
//!
//! ── 为什么补签轮不靠 retryAccounts ─────────────────────────
//! `retryAccounts` 只记「上游说没活动」的账号；整批 429 / 网络失败的行是
//! **failed**，不进那张表（见 `retry_at` 的前两条 return）。而补签轮要覆盖的
//! 恰恰是这批 —— 判据只能是「今天签上了没有」这个账号级事实。
//!
//! ── 间隔与次数可配 ──────────────────────────────────────────
//! `RETRY_GAP_MS` 现在是缺省值；条目上的 `retryGapMinutes` / `retryLimit` 会覆盖
//! 它（见 `checkin_schedules::Schedule`）。`retryLimit = 0` 或
//! `retryGapMinutes = 0` 都表示「不重试」——用户明确要求「别反复打上游」时要能
//! 关掉，而改造前那是一个写死的 10 分钟。
//!
//! ── `retryLimit` 是**真的次数上限**（2026-10 修复）────────────────
//! 在这之前 `retryLimit` 只被当成开关用（`<= 0` 关掉重试），次数从来没被计过：
//! 一个账号只要一直拿到「中性未领取」，就会按 `retryGapMinutes` 重试一整天 ——
//! 实测 Qoder 国际版从 10:01 一路重试到晚上，签到记录里堆了几十条「未领取」，
//! 而界面写的是「每账号每天最多重试 N 次」，承诺与行为对不上。
//!
//! 现在按账号记 `lastResult.retryAttempts`（`{accountId: 今天已重试几次}`）：
//!   · 只有**窗口重试轮**计数（[`Round`]）——主轮与补签轮不是「重试」，
//!     把它们也计进去会让用户设的 3 次实际只够 1 次；
//!   · 次数到 `retryLimit` 后不再放回 `retryAccounts`（写入侧），
//!     `due_accounts_scoped` 再挡一道（读取侧）——升级前留下的旧表没有次数记账，
//!     只靠写入侧挡会让升级当天那一批继续按老行为重试；
//!   · 记账与名单都只认当天，跨天自动作废（与 `retryAccounts` 同一口径）。
//! 次数用尽的账号由 [`exhausted_accounts`] 报给界面，好让页面说清「今天不再重试、
//! 明天自动重置」，而不是让它从名单里消失、看起来像「没事了」。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::server::core::checkin_schedules::Schedule;
use crate::server::core::{account_store::AccountStore, beijing, billing::checkin};

/// 缺省重试间隔（条目没配时用它）
const RETRY_GAP_MS: i64 = 10 * 60 * 1000;

/// 一轮签到的身份：只有「窗口重试轮」才算一次重试次数。
///
/// ── 为什么必须分开（改这块之前先读）─────────────────────────
/// 条目上的 `retryLimit` 说的是「每个账号每天最多重试几次」。主轮是当天的第一次
/// 签到、补签轮是「今天还没签上的再补一次」，两者都不是「重试」；把它们也计进去，
/// 用户设的 3 次实际只够 1 次重试。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Round {
    /// 等窗口重试轮（`retryAccounts` 里到点的那批）。
    Window,
    /// 主轮 / 补签轮 / 手动：只维护名单，不计数。
    Other,
}

/// 一轮重试的三个标量（间隔 / 次数上限 / 轮次身份）。
///
/// 收成一个 struct 是为了调用点可读：`merge(..., RetryConfig::of(schedule, round))`
/// 比 `merge(..., gap_ms, limit, round)` 少一次「谁是谁」的对照。
#[derive(Clone, Copy, Debug)]
pub(super) struct RetryConfig {
    /// 重试间隔（毫秒），0 = 关掉重试。
    pub(super) gap_ms: i64,
    /// 每账号每天最多重试几次，0 = 关掉重试。
    pub(super) limit: i64,
    /// 这一轮的身份：只有 [`Round::Window`] 计次数。
    pub(super) round: Round,
}

impl RetryConfig {
    pub(super) fn of(schedule: Option<&Schedule>, round: Round) -> Self {
        Self {
            gap_ms: gap_ms_of(schedule),
            limit: limit_of(schedule),
            round,
        }
    }
}

/// 取**当天**的那份 `lastResult`（跨天 / 缺失给 `None`）。
///
/// 跨天的旧记账一律作废：否则第二天零点会替昨天的账号再打一遍上游，
/// 也会让昨天的重试次数吃掉今天的额度。
fn today_of(previous: Option<&Value>) -> Option<&Value> {
    previous.filter(|value| {
        value.get("date").and_then(Value::as_str) == Some(beijing::today_key().as_str())
    })
}

/// 今天的「已重试几次」记账（`lastResult.retryAttempts`，`{accountId: 次数}`）。
/// 负数与非法值一律当 0（手改配置不该让次数变成负的、把上限顶掉）。
fn attempts_of(previous: &Value) -> BTreeMap<String, i64> {
    previous
        .get("retryAttempts")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(id, value)| value.as_i64().map(|count| (id.clone(), count.max(0))))
                .collect()
        })
        .unwrap_or_default()
}

/// 今天该重试的账号（`None` = 没有可重试的）。
///
/// `previous` 是条目上一轮的 `lastResult`；`gap_ms` / `limit` 来自条目配置
/// （见 `checkin_schedules::Schedule` 的 `retryGapMinutes` / `retryLimit`）。
/// 只有**当天**的记账算数：跨天的旧记账一律作废（否则第二天零点会替昨天的账号
/// 再打一遍上游）。
///
/// 次数用完的账号在这里再挡一道：写入侧（[`merge`]）已经把它们摘出名单，但那
/// 条路径只覆盖「升级后跑过的轮次」；升级前留下的旧表里全是没记过次数的账号，
/// 只靠写入侧挡会让升级当天那一批继续按老行为重试一整天（2026-10 实测）。
pub(super) fn due_accounts_scoped(
    previous: Option<&Value>,
    now: i64,
    gap_ms: i64,
    limit: i64,
) -> Option<Vec<String>> {
    if gap_ms <= 0 || limit <= 0 {
        return None;
    }
    let previous = today_of(previous)?;
    let attempts = attempts_of(previous);
    let retry = previous.get("retryAccounts").and_then(Value::as_object)?;
    let due: Vec<String> = retry
        .iter()
        .filter(|(id, at)| {
            attempts.get(*id).map(|count| *count < limit).unwrap_or(true)
                && at.as_i64().is_some_and(|at| at <= now)
        })
        .map(|(id, _)| id.clone())
        .collect();
    // 空集合给 None：调用方（`tick`）本来就只认非空，给 `Some([])` 只会让它
    // 多走一趟「有目标但一个都不用签」的判定
    (!due.is_empty()).then_some(due)
}

/// 条目的重试间隔（毫秒）：`retryGapMinutes` 覆盖缺省；0 = 关掉重试。
pub(super) fn gap_ms_of(schedule: Option<&Schedule>) -> i64 {
    match schedule {
        Some(schedule) if schedule.retry_gap_minutes <= 0 => 0,
        Some(schedule) => schedule.retry_gap_minutes.saturating_mul(60_000),
        None => RETRY_GAP_MS,
    }
}

/// 条目的重试次数上限：0 = 关掉重试。
pub(super) fn limit_of(schedule: Option<&Schedule>) -> i64 {
    schedule.map(|schedule| schedule.retry_limit).unwrap_or(3)
}

fn retry_at(store: &AccountStore, row: &Value, now: i64, gap_ms: i64) -> Option<i64> {
    if row
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
        || checkin::day_completed(row)
    {
        return None;
    }
    let claim = row.get("claim")?;
    if !checkin::claim_pending(claim) {
        return None;
    }
    if let Some(at) = claim.get("retryAt") {
        return at
            .as_i64()
            .filter(|at| *at > 0)
            .map(|at| at.max(now + 30_000));
    }
    let id = row.get("id").and_then(Value::as_str)?;
    let provider = store.account_provider_of(id)?;
    let qoder = crate::server::core::providers::qoder::endpoints::Region::Global.provider_id();
    if provider == qoder {
        // Qoder 国际版的窗口由插件在 `retryAt` 里给出（10:00 那个点）；这里
        // 只是它没给时的兜底：等到当天 10:00，与 `providers::qoder::checkin`
        // 的 `daily_window_start` 同一口径。
        //
        // **只兜「窗口还没开」**：窗口过了还没拿到 `retryAt`，是插件已经判定
        // 「今天这个账号没有可领的」（见 `qoder::checkin::no_claim` 的宽限期），
        // 这时再兜「过 10 分钟再试」就会一直试到晚上 —— 2026-10 那批
        // 「未领取」刷屏正是这么来的。
        let window = beijing::now()
            .date_naive()
            .and_hms_opt(10, 0, 0)?
            .and_local_timezone(beijing::offset())
            .single()?
            .timestamp_millis();
        return (now < window).then(|| window.max(now + gap_ms));
    }
    (provider == "codearts").then_some(now + gap_ms)
}

/// 把一轮结果并进 `lastResult`：维护当天的重试名单 + 次数记账。
///
/// ── 两个字段各有分工（别合并成一个）────────────────────────
///   · `retryAccounts`：`{accountId: 下次可重试的时刻}` —— 「谁还要再签一次」；
///   · `retryAttempts`：`{accountId: 今天已重试几次}` —— 「他还剩几次机会」。
/// 光有名单没法拦住「永远重试」，光有次数没法在界面回答「还要不要再签」。
///
/// 次数只加在 [`Round::Window`] 上（见 [`Round`]）：主轮 / 补签轮虽然也传了
/// `retried`（那是「本轮真跑过的账号」），但不是「重试」。
pub(super) fn merge(
    store: &AccountStore,
    mut summary: Value,
    result: &Value,
    previous: Option<&Value>,
    retried: Option<&[String]>,
    config: RetryConfig,
) -> Value {
    let Some(fields) = summary.as_object_mut() else {
        return summary;
    };
    let now = crate::server::logging::now_ms();
    // ── 重试名单：**始终**从当天的上一轮继承 ────────────────────
    // 一条条目一天会跑多批（各家时刻不同），每一批只覆盖自己那几家。若每批都
    // 从空表重建，上一批记下的「等窗口重试」就会被冲掉（CodeArts 那类靠它重试
    // 的账号会静默失去重试机会）。所以基线永远是当天的旧表，只把本轮真跑过的
    // 那几个 id 摘掉。次数记账同理继承。
    let previous_today = today_of(previous);
    let mut retry = previous_today
        .and_then(|value| value.get("retryAccounts"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut attempts = previous_today.map(attempts_of).unwrap_or_default();
    let retry_enabled = config.gap_ms > 0 && config.limit > 0;
    if !retry_enabled { retry.clear(); }
    if let Some(retried) = retried {
        for id in retried {
            retry.remove(id);
            // 只有窗口重试轮计次数；`limit <= 0`（重试关着）时表根本不该长出来
            if config.round == Round::Window && config.limit > 0 {
                let count = attempts.entry(id.clone()).or_insert(0);
                *count = count.saturating_add(1);
            }
        }
    }
    if let (Some(previous), Some(_)) = (previous_today, retried) {
        for key in ["succeeded", "active", "failedCount"] {
            let before = previous.get(key).and_then(Value::as_u64).unwrap_or(0);
            let current = fields.get(key).and_then(Value::as_u64).unwrap_or(0);
            fields.insert(key.to_string(), Value::from(before.saturating_add(current)));
        }
        for key in ["total", "skipped"] {
            let before = previous.get(key).and_then(Value::as_u64).unwrap_or(0);
            let current = fields.get(key).and_then(Value::as_u64).unwrap_or(0);
            fields.insert(key.to_string(), Value::from(before.saturating_add(current)));
        }
        let mut failures = previous
            .get("failed")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        failures.extend(
            fields
                .get("failed")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        failures.truncate(5);
        fields.insert("failed".to_string(), Value::Array(failures));
    }
    if let Some(rows) = result.get("results").and_then(Value::as_array) {
        for row in rows {
            let Some(id) = row.get("id").and_then(Value::as_str) else {
                continue;
            };
            // 本轮真跑过的账号一律先从旧表摘掉，再由 `retry_at` 决定要不要放回 ——
            // 否则「上次在等窗口、这次已经签上」的账号会留在表里被反复重试。
            // （改造前每轮都是全量范围，`retried` 覆盖得住；现在按家分批，
            // 主轮那次 `retried` 是 None，必须靠这里兜。）
            retry.remove(id);
            if checkin::day_completed(row) {
                // 今天签上了：次数记账一并作废，否则界面会一直挂着
                // 「今日重试已用完」——它已经签完了，不是没机会了
                attempts.remove(id);
                continue;
            }
            // 次数已到上限：不再放回名单（这就是「每账号每天最多重试 N 次」的
            // 落地处）。留个行也好让界面看到它，但界面读的是 `retryExhausted`。
            if !retry_enabled || attempts.get(id).is_some_and(|count| *count >= config.limit) {
                continue;
            }
            if let Some(at) = retry_at(store, row, now, config.gap_ms) {
                retry.insert(id.to_string(), Value::from(at));
            }
        }
    }
    // 记账裁剪：只留「还在名单里」与「今天次数已用尽」两种。已签上 / 已报错的
    // 账号不需要留次数（补签轮看的是账号的 `checkinAt`，不看这张表）。
    attempts.retain(|id, count| retry.contains_key(id) || *count >= config.limit);
    fields.insert("retryAccounts".to_string(), Value::Object(retry));
    fields.insert(
        "retryAttempts".to_string(),
        serde_json::to_value(&attempts).unwrap_or(Value::Null),
    );
    summary
}

pub(super) fn next_at(result: Option<&Value>, today: &str, limit: i64) -> Option<i64> {
    let result = result.filter(|value| value.get("date").and_then(Value::as_str) == Some(today))?;
    let attempts = attempts_of(result);
    result
        .get("retryAccounts")
        .and_then(Value::as_object)?
        .iter()
        // 次数用完的账号不会再被执行，不该拿它的时刻去报「下次执行」
        .filter(|(id, _)| attempts.get(*id).map(|count| *count < limit).unwrap_or(true))
        .filter_map(|(_, at)| at.as_i64())
        .min()
}

/// **今天次数已用尽**的账号 id（界面据此说「今天不再重试、明天自动重置」）。
///
/// 必须单独报给界面，不能让它们就这么从名单里消失：`retryCount`（还要再试的
/// 账号数）会归零，用户看到的是「没事了」，而实际是「今天没戏了」。
///
/// 判据用 `>=` 而不是 `>`：写入侧在次数**到达**上限的那一刻就把它摘出了名单，
/// 「次数够了」与「不在名单里了」是同一时刻发生的。
pub(super) fn exhausted_accounts(previous: Option<&Value>, limit: i64) -> Vec<String> {
    if limit <= 0 {
        return Vec::new();
    }
    let Some(previous) = today_of(previous) else {
        return Vec::new();
    };
    attempts_of(previous)
        .into_iter()
        .filter(|(_, count)| *count >= limit)
        .map(|(id, _)| id)
        .collect()
}

/// **今天还没签上的账号 id**（补签轮的目标集合）。
///
/// 判据是账号的 `checkinAt` 是否落在今天（北京时间）—— 与账号页「今日已签到」、
/// 签到中心的 `checkedInToday` 同一份事实，所以三处永远不会给出矛盾结论。
///
/// 不看 `available` / `enabled`：那两个管的是转发，与签到无关（见
/// `billing::checkin` 模块头「签到不看 enabled」）。真正该不该签由
/// `resolve_checkin_targets` 的 `supports_checkin` 与范围过滤决定。
pub(super) fn unsigned_accounts(store: &AccountStore) -> Vec<String> {
    store
        .list_accounts()
        .get("accounts")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|account| !crate::server::core::account_store::checkin_state::checked_in_today(account))
                .filter_map(|account| account.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
