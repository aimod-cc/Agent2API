//! 提供商专属签到时刻 + 补签（第二轮）时刻。
//!
//! ── 为什么需要「专属时刻」──────────────────────────────────
//! 签到不是「谁都能在零点签」：上游的每日活动各有窗口，窗口没开时调用拿到的是
//! 「暂未下发」（中性未领取），调度器只能反复重试。实测（2026-10-29）Qoder 国际版
//! 在 00:01 触发后一路重试到 10:00 才进入窗口，签到记录里留下 7 条「未领取」；
//! 用户看到的是「设的是 00:01，实际却签了 7 次」。
//!
//! | 家 | 窗口（北京时间） | 依据 |
//! |---|---|---|
//! | Qoder（两地） | 每天 10:00 开启 | `providers::qoder::checkin` 的模块头（实测） |
//! | ZCode（两地） | 每天 19:00 开启 | 官方公告：每日 19:00 起可领当日赠送额度 |
//!
//! 时刻取「窗口 + 1 分钟」：正好卡在开启那一秒时，上游可能还没下发活动。
//!
//! ── 三档取值（改之前先读）──────────────────────────────────
//! 条目上的 `providerTimes[家]`（用户覆盖）> 本表（内置）> 条目自己的 `time`。
//! 于是存量配置**零迁移**：没覆盖的家照旧按条目时刻跑，只有本表里的家按专属时刻。
//! 用户想改就改条目上那一格，不必动代码。
//!
//! ── 补签（第二轮）──────────────────────────────────────────
//! 补签点逐家按 `max(retryTime, 主签到区间结束 + 等待间隔)` 算，且只处理当天未签到账号。
//! 默认区间 00:01–00:10、间隔 10 分钟时，普通平台 00:20 补，Qoder 10:20、ZCode 19:20。
//! 补签不会提前执行仍在等待随机主轮的账号，也不会把超出当天的时间绕回凌晨。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};

use crate::server::core::auto_checkin::{normalize_time, today_at};
use crate::server::core::beijing;
use crate::server::core::checkin_schedules::Schedule;

/// 补签不早于的缺省时刻；实际补签仍从各家区间结束后计算。
pub const DEFAULT_RETRY_TIME: &str = "00:10";
pub const DEFAULT_SPREAD_MINUTES: i64 = 9;

pub fn valid_range(start: &str, end: &str) -> bool {
    matches!((today_at_minutes(start), today_at_minutes(end)), (Some(start), Some(end)) if end >= start)
}

pub fn end_with_spread(start: &str, spread: i64) -> String {
    format_minutes((today_at_minutes(start).unwrap_or(1) + spread.max(0)).min(1439))
}

pub fn default_time_end(start: &str) -> String {
    end_with_spread(start, DEFAULT_SPREAD_MINUTES)
}

pub fn spread_minutes(schedule: &Schedule) -> i64 {
    (today_at_minutes(&schedule.time_end).unwrap_or(10)
        - today_at_minutes(&schedule.time).unwrap_or(1)).max(0)
}

pub fn effective_end(schedule: &Schedule, provider: &str) -> String {
    end_with_spread(&effective_time(schedule, provider), spread_minutes(schedule))
}

pub fn provider_window(schedule: &Schedule, provider: &str, now: DateTime<FixedOffset>) -> Option<(i64, i64)> {
    Some((today_at(&effective_time(schedule, provider), now)?.timestamp_millis(),
        today_at(&effective_end(schedule, provider), now)?.timestamp_millis()))
}

/// 提供商专属的每日窗口时刻（北京时间，窗口开启后 1 分钟）。
///
/// 顺序即界面「专属时刻」一栏的展示顺序。**只登记有实测依据的窗口** ——
/// 别按「大概那时候开」往这里加，加了就等于把某家的签到时刻悄悄挪走。
pub const PROVIDER_TIMES: [(&str, &str); 4] = [
    ("qoder", "10:01"),
    ("qoder-intl", "10:01"),
    ("zcode", "19:01"),
    ("zcode-intl", "19:01"),
];

/// 某家的内置专属时刻（没有则 None = 跟随条目时刻）。
pub fn provider_time(provider: &str) -> Option<&'static str> {
    PROVIDER_TIMES
        .iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, time)| *time)
}

/// 一家在某个条目里的**生效时刻**：条目覆盖 > 内置专属 > 条目自己的 time。
pub fn declared_time(provider: &str) -> Option<String> {
    crate::server::core::plugin_host::wasm::tasks::checkin_time(provider)
        .or_else(|| (!crate::server::core::plugin_host::wasm::has_capability(provider, "tasks.v1"))
            .then(|| provider_time(provider).map(str::to_string)).flatten())
}

pub fn effective_time(schedule: &Schedule, provider: &str) -> String {
    schedule
        .provider_times
        .get(provider)
        .filter(|time| !time.is_empty())
        .cloned()
        .or_else(|| declared_time(provider))
        .unwrap_or_else(|| schedule.time.clone())
}

/// 某家在 `date` 那天是否已经跑过主轮
fn fired_on(map: &BTreeMap<String, String>, provider: &str, date: &str) -> bool {
    map.get(provider).is_some_and(|value| value == date)
}

/// 主轮是否已跑过（含**存量兼容**）。
///
/// 逐家标记（`lastFiredProviders`）为空 = 升级前的旧条目：那时整条条目只有
/// 一个 `lastFiredDate`，一旦是今天就代表「全跑过了」。这条兜底只影响升级当天
/// （第二天起逐家标记就有了），但它必须存在 —— 否则升级那一刻会把当天已经签过
/// 的家全部重签一遍。
pub(crate) fn main_fired(schedule: &Schedule, provider: &str, today: &str) -> bool {
    if schedule.account_plan.date == today {
        return !schedule.account_plan.pending_for(provider);
    }
    if !schedule.last_fired_providers.is_empty() {
        return fired_on(&schedule.last_fired_providers, provider, today);
    }
    schedule.last_fired_date.as_deref() == Some(today)
}

/// 今天的主轮该跑哪些家：时刻已到、今天还没跑过。
///
/// 这是 `tick` 的判定核心 —— 条目不再有「一天只跑一次」的整体去重，改由
/// **每家自己的日期标记**决定，于是同一条条目可以在 00:01 / 10:01 / 19:01
/// 各跑一批。
pub fn due_providers(
    schedule: &Schedule,
    now: DateTime<FixedOffset>,
    today: &str,
) -> Vec<String> {
    schedule
        .providers
        .iter()
        .filter(|provider| !main_fired(schedule, provider, today))
        .filter(|provider| time_passed(&effective_time(schedule, provider), now))
        .cloned()
        .collect()
}

/// 某家的补签时刻：`max(配置的补签时刻, 该家主签到区间结束 + 等待间隔)`。
/// 补签和活动窗口重试共用间隔；间隔为 0 时补签仍至少等 1 分钟，跨天则取消当天补签。
pub fn provider_retry_time(schedule: &Schedule, provider: &str) -> Option<String> {
    if schedule.retry_time.is_empty() {
        return None;
    }
    let base = today_at_minutes(&schedule.retry_time)?;
    let main = today_at_minutes(&effective_end(schedule, provider))?;
    // 间隔取重试间隔；它被配成 0（关掉重试）时仍给 1 分钟 —— 补签轮与「等窗口
    // 重试」是两件事，关掉后者不该顺手把补签也关掉
    let gap = schedule.retry_gap_minutes.max(1);
    let target = base.max(main + gap);
    (target < 1440).then(|| format_minutes(target))
}

/// 今天的补签轮该补哪些家：该家的补签时刻已到、今天还没补过。
///
/// 时刻由 [`provider_retry_time`] 算（已含「窗口没开不补」这层：补签点一定晚于
/// 主轮时刻）。这里不再单独判窗口 —— 那会让两处口径分叉。
pub fn retry_due_providers(
    schedule: &Schedule,
    now: DateTime<FixedOffset>,
    today: &str,
) -> Vec<String> {
    schedule
        .providers
        .iter()
        .filter(|provider| crate::server::core::plugin_host::serves_provider(provider))
        .filter(|provider| !fired_on(&schedule.last_retried_providers, provider, today))
        .filter(|provider| main_fired(schedule, provider, today))
        .filter(|provider| {
            provider_retry_time(schedule, provider)
                .is_some_and(|time| time_passed(&time, now))
        })
        .cloned()
        .collect()
}

/// 今天还没跑的家（**含还没到点的**）—— 给界面的「今天跑完了吗」用。
///
/// 与 [`due_providers`] 的差别只在「不看时刻」：那个是调度判据（到点才跑），
/// 这个是状态判据（今天是否都跑过了）。条目整体完成 = 这个返回空。
pub fn pending_providers(schedule: &Schedule, today: &str) -> Vec<String> {
    schedule
        .providers
        .iter()
        .filter(|provider| !main_fired(schedule, provider, today))
        .cloned()
        .collect()
}

/// 今天**该跑但还没跑**的家（时刻已过、标记还是空的）—— 界面「待执行」用。
///
/// 与 [`pending_providers`] 的区别很重要：早上 08:00 看一条 00:01 的条目，
/// Qoder（10:01）还没到点，它**不是**「待执行」而是「等窗口」—— 混在一起会让
/// 用户以为卡住了。
pub fn overdue_providers(
    schedule: &Schedule,
    now: DateTime<FixedOffset>,
    today: &str,
) -> Vec<String> {
    due_providers(schedule, now, today)
}

/// 条目里下一次还会发生的时刻（毫秒）—— 界面「下次执行」用。
///
/// 主轮逐家算：今天还没跑且时刻未到 → 今天；已跑 / 时刻已过 → 明天。
/// 补签轮单独算（见 [`retry_next_at`]）。两者取最早。
pub fn next_run_at(
    schedule: &Schedule,
    now: DateTime<FixedOffset>,
    today: &str,
) -> Option<i64> {
    let mut earliest: Option<i64> = if schedule.account_plan.date == today {
        schedule.account_plan.next_at().map(|at| at.max(now.timestamp_millis()))
    } else { None };
    for provider in &schedule.providers {
        if !crate::server::core::plugin_host::serves_provider(provider) { continue; }
        let fired = main_fired(schedule, provider, today);
        if !fired && schedule.account_plan.date == today { continue; }
        let Some(at) = next_occurrence(&effective_time(schedule, provider), fired, now) else {
            continue;
        };
        earliest = Some(earliest.map_or(at, |current: i64| current.min(at)));
    }
    if let Some(retry) = retry_next_at(schedule, now, today) {
        earliest = Some(earliest.map_or(retry, |current: i64| current.min(retry)));
    }
    earliest
}

/// 补签轮的下一次时刻（毫秒）；补签关掉（`retryTime` 为空）给 None。
///
/// 「还有得补」（某家今天没补过且窗口已过）→ 今天；否则顺延到明天。
pub fn retry_next_at(
    schedule: &Schedule,
    now: DateTime<FixedOffset>,
    today: &str,
) -> Option<i64> {
    if schedule.retry_time.is_empty() {
        return None;
    }
    // 逐家算补签点，取最近的那个「今天还没补过」的；都已补过就顺延到明天
    let mut earliest: Option<i64> = None;
    for provider in &schedule.providers {
        if !crate::server::core::plugin_host::serves_provider(provider) { continue; }
        let Some(time) = provider_retry_time(schedule, provider) else {
            continue;
        };
        let Some(target) = today_at(&time, now) else {
            continue;
        };
        let pending = !fired_on(&schedule.last_retried_providers, provider, today);
        if pending && !main_fired(schedule, provider, today) && target <= now { continue; }
        let next = if pending {
            // 已到点还没补（等下一个 tick）→ 就是现在
            if target > now { target.timestamp_millis() } else { now.timestamp_millis() }
        } else {
            (target + chrono::Duration::days(1)).timestamp_millis()
        };
        earliest = Some(earliest.map_or(next, |current: i64| current.min(next)));
    }
    earliest
}

/// `HH:MM` → 当天零点起的分钟数（非法返回 None）
fn today_at_minutes(time_text: &str) -> Option<i64> {
    let normalized = normalize_time(Some(&serde_json::Value::String(time_text.to_string()))).ok()?;
    let (hour, minute) = normalized.split_once(':')?;
    Some(hour.parse::<i64>().ok()? * 60 + minute.parse::<i64>().ok()?)
}

/// 分钟数 → `HH:MM`（跨天时按 24 小时取模）
fn format_minutes(total: i64) -> String {
    let wrapped = total.rem_euclid(24 * 60);
    format!("{:02}:{:02}", wrapped / 60, wrapped % 60)
}

/// 今天的 HH:MM 是否已经过去（非法时刻按「没过去」处理 —— 宁可晚签不早撞）
fn time_passed(time_text: &str, now: DateTime<FixedOffset>) -> bool {
    today_at(time_text, now).is_some_and(|target| now >= target)
}

/// 某个时刻的下一次出现：`fired`（今天已经跑过）或时刻已过 → 明天。
fn next_occurrence(
    time_text: &str,
    fired: bool,
    now: DateTime<FixedOffset>,
) -> Option<i64> {
    let target = today_at(time_text, now)?;
    let next = if fired || target <= now {
        target + chrono::Duration::days(1)
    } else {
        target
    };
    Some(next.timestamp_millis())
}

/// 归一化条目里的 `providerTimes`（`{providerId: "HH:MM"}`）。
///
/// 只认已知的签到提供商（`checkin_providers()`，含插件家）；空串表示「这一家
/// 撤掉覆盖、回落到内置 / 条目时刻」，因此**保留**为空串而不是丢弃 —— 丢弃会
/// 让「撤掉覆盖」变成「没改」，用户改不回去。
pub fn normalize_overrides(value: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        return BTreeMap::new();
    };
    let known = crate::server::core::auto_checkin::checkin_providers();
    let mut map = BTreeMap::new();
    for (provider, raw) in object {
        if !known.iter().any(|id| id == provider) {
            continue;
        }
        let text = raw.as_str().map(str::trim).unwrap_or("");
        if text.is_empty() {
            map.insert(provider.clone(), String::new());
            continue;
        }
        if let Ok(normalized) = normalize_time(Some(raw)) {
            map.insert(provider.clone(), normalized);
        }
    }
    map
}

/// 归一化 `retryTime`：缺失 → 缺省；空串 → 关闭（返回空串）；非法 → 缺省。
pub fn normalize_retry_time(value: Option<&serde_json::Value>) -> String {
    match value {
        None => DEFAULT_RETRY_TIME.to_string(),
        Some(serde_json::Value::String(text)) if text.trim().is_empty() => String::new(),
        Some(raw) => normalize_time(Some(raw)).unwrap_or_else(|_| DEFAULT_RETRY_TIME.to_string()),
    }
}

/// 北京时间 → 本模块要的偏移（与 `auto_checkin` 同一口径，仅便于调用方）
pub fn now() -> DateTime<FixedOffset> {
    beijing::now()
}
