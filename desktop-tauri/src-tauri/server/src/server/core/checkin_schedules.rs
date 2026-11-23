//! 定时签到的**多条目**配置（`autoCheckin.schedules` 数组）。
//!
//! ── 形状 ────────────────────────────────────────────────────
//! 每条条目是一次「区间签到」的完整配置：
//!   `{id, label, enabled, time, timeEnd, accountPlan, providers, providerTimes, retryTime,
//!     retryGapMinutes, retryLimit, lastFiredDate, lastFiredProviders,
//!     lastRetriedProviders, lastResult}`
//!   - `id`     稳定标识（界面按它 upsert / 删除 / 立即执行；合成条目是 "default"）
//!   - `label`  展示名（可空；空时展示退到 `time`）
//!   - `enabled` 条目自己的开关
//!   - `time` / `timeEnd` HH:MM，北京时间同一天的区间；相同表示固定时刻。
//!   - `accountPlan` 当天逐账号的随机时刻和尝试标记，重启后沿用。
//!   - `providers` 这条要签的提供商（`CHECKIN_PROVIDERS` 子集，至少一家）
//!   - `providerTimes` 逐家的区间起点覆盖（`{providerId: "HH:MM"}`；空串 = 撤销覆盖）。
//!     沿用默认区间长度，没覆盖的家跟随内置专属起点或 `time`。
//!   - `retryTime` 补签不早于的时刻（缺省 00:10；空串 = 关掉补签轮）；实际不早于区间结束加间隔。
//!   - `retryGapMinutes` 中性未领取的账号多久后重试（缺省 10 分钟；0 = 不重试）
//!   - `retryLimit` 每个账号每天最多重试几次（缺省 3；0 = 不重试）
//!   - `lastFiredDate` / `lastFiredProviders` / `lastRetriedProviders` / `lastResult`
//!     当天去重与上一轮结果：前两个逐家记账（同一家一天只跑一次主轮），
//!     `lastFiredDate` 是升级前的整体标记（只作存量兜底，见 `times::main_fired`）
//! 总开关沿用 `autoCheckin.enabled`（见 `core::auto_checkin`）：关了所有条目都不跑。
//!
//! ── 存量兼容：读时合成，不迁移文件 ─────────────────────────
//! `schedules` 键**缺失**时（老配置），用旧字段（`time` / `providers` /
//! `lastFiredDate` / `lastResult`）合成一条 `id="default"` 的条目：老配置零迁移
//! 继续工作，当天的去重状态原样保留。条目键**存在**（哪怕 `[]`）就以存储为准，
//! `[]` 表达「用户删光了」——这也是删除合成条目的落点：删除写 `[]`，合成不再发生。
//! 条目一旦存在，旧字段不再读（保留不删，作为回滚安全垫）。
//!
//! ── 为什么放 core ──────────────────────────────────────────
//! 调度点（`auto_checkin::tick` / `start`）与记账点（`fire` 的条级标记）都在
//! core；`core::auto_checkin` 直接读写 config 已是先例（见它的模块头「配置读写」），
//! 本模块沿用同一模式：`update_raw_field` 合并写回，config.json 的其它字段原样保留。

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::server::config;
use crate::server::core::auto_checkin;
use crate::server::core::auto_checkin::times;

pub mod account_plan;

/// 条目上限：再多列表难以扫读，定点签到的语义也不需要更多
pub const MAX_SCHEDULES: usize = 10;
/// 合成默认条目的 id（存量配置没有 `schedules` 时的那条）
pub const DEFAULT_SCHEDULE_ID: &str = "default";
/// 中性未领取的重试间隔上限（分钟）：再长就跨过下一个窗口了，没有意义
pub const MAX_RETRY_GAP_MINUTES: i64 = 120;
/// 每账号每天的重试次数上限：一次主轮 + 这些重试，够覆盖窗口前后的抖动
pub const MAX_RETRY_LIMIT: i64 = 20;

/// 一条定点签到条目（读侧形状；字段缺失逐个兜底，与 auto_checkin 同一口径）
#[derive(Clone, Debug)]
pub struct Schedule {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub time: String,
    pub time_end: String,
    pub providers: Vec<String>,
    /// 逐家的时刻覆盖（空串 = 撤销覆盖，回落到内置 / 条目时刻）
    pub provider_times: BTreeMap<String, String>,
    /// 补签轮时刻（空串 = 关掉补签轮）
    pub retry_time: String,
    /// 中性未领取的重试间隔（分钟；0 = 不重试）
    pub retry_gap_minutes: i64,
    /// 每账号每天的重试次数上限（0 = 不重试）
    pub retry_limit: i64,
    pub last_fired_date: Option<String>,
    /// 逐家的主轮完成日期（`{providerId: "YYYY-MM-DD"}`）
    pub last_fired_providers: BTreeMap<String, String>,
    /// 逐家的补签轮完成日期
    pub last_retried_providers: BTreeMap<String, String>,
    pub account_plan: account_plan::AccountPlan,
    pub last_result: Option<Value>,
}

/// 展示名：用户起的名，没起就退到时刻（时刻是唯一必填项，一定能展示）
pub fn display_label(schedule: &Schedule) -> &str {
    if schedule.label.is_empty() {
        &schedule.time
    } else {
        &schedule.label
    }
}

// ─── 读取 ───────────────────────────────────────────────────

/// 条目键是否存在（哪怕 `[]`）：存在即以存储为准，不再从旧字段合成
fn stored_key_exists() -> bool {
    config::with_raw(|raw| {
        matches!(
            raw.get(auto_checkin::CONFIG_KEY),
            Some(Value::Object(map)) if map.contains_key("schedules")
        )
    })
    .unwrap_or(false)
}

/// 存储里的条目数组（原文；键不存在给 None）
fn stored_entries() -> Option<Vec<Value>> {
    config::with_raw(|raw| {
        raw.get(auto_checkin::CONFIG_KEY)
            .and_then(|value| value.get("schedules"))
            .and_then(Value::as_array)
            .cloned()
    })
    .flatten()
}

/// 单条的归一化：id / label / enabled / time / providers 逐个兜底。
/// time 坏了回落 DEFAULT_TIME（调度起不来比时刻错更糟，与 auto_checkin 同）。
fn normalize_schedule(raw: &Value) -> Schedule {
    let known = auto_checkin::checkin_providers();
    normalize_schedule_with(raw, &known)
}

fn normalize_schedule_with(raw: &Value, known: &[String]) -> Schedule {
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or(DEFAULT_SCHEDULE_ID)
        .to_string();
    let label = raw
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let enabled = raw.get("enabled").and_then(Value::as_bool) == Some(true);
    let time = raw
        .get("time")
        .filter(|value| !value.is_null())
        .and_then(|value| auto_checkin::normalize_time(Some(value)).ok())
        .unwrap_or_else(auto_checkin::default_time);
    let time_end = raw.get("timeEnd")
        .and_then(|value| auto_checkin::normalize_time(Some(value)).ok())
        .filter(|end| times::valid_range(&time, end))
        .unwrap_or_else(|| times::default_time_end(&time));
    let providers = auto_checkin::normalize_providers_with(raw.get("providers"), known);
    Schedule {
        id,
        label,
        enabled,
        time,
        time_end,
        providers,
        provider_times: times::normalize_overrides_with(raw.get("providerTimes"), known),
        retry_time: times::normalize_retry_time(raw.get("retryTime")),
        retry_gap_minutes: retry_number(raw, "retryGapMinutes", 10, MAX_RETRY_GAP_MINUTES),
        retry_limit: retry_number(raw, "retryLimit", 3, MAX_RETRY_LIMIT),
        last_fired_date: raw
            .get("lastFiredDate")
            .and_then(Value::as_str)
            .map(str::to_string),
        last_fired_providers: date_map(raw.get("lastFiredProviders")),
        last_retried_providers: date_map(raw.get("lastRetriedProviders")),
        account_plan: account_plan::AccountPlan::from_value(raw.get("accountPlan")),
        last_result: raw.get("lastResult").filter(|value| value.is_object()).cloned(),
    }
}

/// 重试相关数值：缺失 / 非法 → 缺省；负值夹到 0（0 = 关掉重试）。
fn retry_number(raw: &Value, key: &str, default: i64, max: i64) -> i64 {
    raw.get(key)
        .and_then(Value::as_i64)
        .unwrap_or(default)
        .clamp(0, max)
}

/// `{providerId: "YYYY-MM-DD"}` 的归一化（只收非空字符串值）
fn date_map(value: Option<&Value>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Some(object) = value.and_then(Value::as_object) else {
        return map;
    };
    for (key, raw) in object {
        if let Some(text) = raw.as_str().filter(|text| !text.is_empty()) {
            map.insert(key.clone(), text.to_string());
        }
    }
    map
}

/// 存量兼容：用旧字段合成的那条默认条目（`schedules` 键缺失时）
fn legacy_schedule() -> Schedule {
    legacy_schedule_of(&auto_checkin::raw_object())
}

fn legacy_schedule_of(raw: &Map<String, Value>) -> Schedule {
    let known = auto_checkin::checkin_providers();
    legacy_schedule_of_with(raw, &known)
}

fn legacy_schedule_of_with(raw: &Map<String, Value>, known: &[String]) -> Schedule {
    normalize_schedule_with(&json!({
        "id": DEFAULT_SCHEDULE_ID,
        "enabled": raw.get("enabled").cloned().unwrap_or(Value::Null),
        "time": raw.get("time").cloned().unwrap_or(Value::Null),
        "timeEnd": raw.get("timeEnd").cloned().unwrap_or(Value::Null),
        "accountPlan": raw.get("accountPlan").cloned().unwrap_or(Value::Null),
        "providers": raw.get("providers").cloned().unwrap_or(Value::Null),
        "providerTimes": raw.get("providerTimes").cloned().unwrap_or(Value::Null),
        "retryTime": raw.get("retryTime").cloned().unwrap_or(Value::Null),
        "retryGapMinutes": raw.get("retryGapMinutes").cloned().unwrap_or(Value::Null),
        "retryLimit": raw.get("retryLimit").cloned().unwrap_or(Value::Null),
        "lastFiredDate": raw.get("lastFiredDate").cloned().unwrap_or(Value::Null),
        "lastFiredProviders": raw.get("lastFiredProviders").cloned().unwrap_or(Value::Null),
        "lastRetriedProviders": raw.get("lastRetriedProviders").cloned().unwrap_or(Value::Null),
        "lastResult": raw.get("lastResult").cloned().unwrap_or(Value::Null),
    }), known)
}

/// 全部条目（调度循环与快照的组装原料）。
/// `schedules` 键缺失 → 旧字段合成的默认条目；存在（含 `[]`）→ 存储为准。
pub fn list() -> Vec<Schedule> {
    let known = auto_checkin::checkin_providers();
    let stored = config::with_raw(|raw| {
        let Some(Value::Object(obj)) = raw.get(auto_checkin::CONFIG_KEY) else {
            return (None, None);
        };
        match obj.get("schedules").and_then(Value::as_array) {
            Some(items) => (Some(items.clone()), None),
            None => (None, Some(obj.clone())),
        }
    });
    match stored {
        Some((Some(items), _)) if !items.is_empty() => items
            .iter()
            .map(|raw| normalize_schedule_with(raw, &known))
            .collect(),
        Some((Some(_), _)) => Vec::new(),
        Some((None, Some(obj))) => vec![legacy_schedule_of_with(&obj, &known)],
        _ => vec![legacy_schedule_of_with(&Map::new(), &known)],
    }
}

// ─── 写回 ───────────────────────────────────────────────────

/// 条目数组写回 `autoCheckin.schedules`（其余字段原样保留）
fn write_entries(entries: Vec<Value>) {
    auto_checkin::write_state(json!({ "schedules": entries }));
}

/// 条目 → 存储原文（保留 lastFiredDate / lastResult，条目级的去重状态跨重启）
fn to_value(schedule: &Schedule) -> Value {
    json!({
        "id": schedule.id,
        "label": schedule.label,
        "enabled": schedule.enabled,
        "time": schedule.time,
        "timeEnd": schedule.time_end,
        "accountPlan": schedule.account_plan,
        "providers": schedule.providers,
        "providerTimes": schedule.provider_times,
        "retryTime": schedule.retry_time,
        "retryGapMinutes": schedule.retry_gap_minutes,
        "retryLimit": schedule.retry_limit,
        "lastFiredDate": schedule.last_fired_date,
        "lastFiredProviders": schedule.last_fired_providers,
        "lastRetriedProviders": schedule.last_retried_providers,
        "lastResult": schedule.last_result,
    })
}

// ─── CRUD（界面的三条写入口）───────────────────────────────

/// 新增 / 更新一条条目（按 id upsert）。校验：时刻必填且合法、范围至少一家、
/// 条数上限。返回落盘后的那条（含后端归一化的值，界面以后端返回为准）。
///
/// `enabled` 缺省不是「开」而是「保持原样」：兼容入口（`auto_checkin::configure`
/// 只改时刻/范围）不该顺手把停用的条目点亮；新建条目没有原样可保，缺省为开。
pub fn upsert(payload: &Value) -> Result<Schedule, String> {
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("schedule-{}", crate::server::logging::now_ms()));
    let time = auto_checkin::normalize_time(payload.get("time"))
        .map_err(|error| error.message)?;
    // 范围：只认注册过的提供商（去重、按注册顺序）；一条不勾到点也无账号可签
    let list_value = payload.get("providers");
    let picked: Vec<String> = auto_checkin::checkin_providers()
        .into_iter()
        .filter(|id| {
            list_value
                .and_then(Value::as_array)
                .map(|items| items.iter().any(|item| item.as_str() == Some(id.as_str())))
                .unwrap_or(false)
        })
        .collect();
    if picked.is_empty() {
        return Err("至少勾选一家签到提供商".to_string());
    }
    let label = payload
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let entries = stored_entries().unwrap_or_default();
    let existing = entries.iter().position(|raw| {
        raw.get("id").and_then(Value::as_str) == Some(id.as_str())
    });
    if existing.is_none() && entries.len() >= MAX_SCHEDULES {
        return Err(format!("定时条目最多 {MAX_SCHEDULES} 条"));
    }
    let existing_entry = existing.map(|index| normalize_schedule(&entries[index]))
        .or_else(|| (!stored_key_exists() && id == DEFAULT_SCHEDULE_ID).then(legacy_schedule));
    let enabled = match payload.get("enabled").and_then(Value::as_bool) {
        Some(flag) => flag,
        // 缺省：更新保持原开关；新建默认开 —— 但第一次固化**合成条目**时，
        // 「原开关」在旧字段里（合成的 default 读的就是它），一并带过来
        None => match existing_entry.as_ref().map(|old| old.enabled) {
            Some(flag) => flag,
            None if !stored_key_exists() && id == DEFAULT_SCHEDULE_ID => {
                legacy_schedule().enabled
            }
            None => true,
        },
    };
    // 更新已有条目时保留它的去重状态；新建条目从「今天还没跑」开始
    let (last_fired_date, last_result) = existing_entry
        .as_ref()
        .map(|old| (old.last_fired_date.clone(), old.last_result.clone()))
        .unwrap_or((None, None));
    // 存量兼容：第一次编辑合成的默认条目时，把旧字段的去重状态带过来
    let (last_fired_date, last_result) = if last_fired_date.is_none() && !stored_key_exists() && id == DEFAULT_SCHEDULE_ID {
        let legacy = legacy_schedule();
        (legacy.last_fired_date, legacy.last_result)
    } else {
        (last_fired_date, last_result)
    };
    // 逐家去重标记同样要跨「编辑条目」保留：改时刻不该把今天已经签过的家重签一遍
    let (last_fired_providers, last_retried_providers) = existing_entry
        .as_ref()
        .map(|old| (old.last_fired_providers.clone(), old.last_retried_providers.clone()))
        .unwrap_or_default();
    // ── 逐家时刻 / 补签的写入口径 ──────────────────────────────
    // 三个字段都**只认 payload 里真的给了的键**：老界面（与 `configure` 那条兼容
    // 入口）只传 time/providers，不该把它们顺手重置成缺省值。
    let provider_times = match payload.get("providerTimes") {
        Some(value) => times::normalize_overrides(Some(value)),
        None => existing_entry
            .as_ref()
            .map(|old| old.provider_times.clone())
            .unwrap_or_default(),
    };
    let retry_time = match payload.get("retryTime") {
        Some(value) => times::normalize_retry_time(Some(value)),
        None => existing_entry
            .as_ref()
            .map(|old| old.retry_time.clone())
            .unwrap_or_else(|| times::DEFAULT_RETRY_TIME.to_string()),
    };
    let retry_gap_minutes = payload
        .get("retryGapMinutes")
        .and_then(Value::as_i64)
        .map(|value| value.clamp(0, MAX_RETRY_GAP_MINUTES))
        .or_else(|| existing_entry.as_ref().map(|old| old.retry_gap_minutes))
        .unwrap_or(10);
    let retry_limit = payload
        .get("retryLimit")
        .and_then(Value::as_i64)
        .map(|value| value.clamp(0, MAX_RETRY_LIMIT))
        .or_else(|| existing_entry.as_ref().map(|old| old.retry_limit))
        .unwrap_or(3);
    let time_end = match payload.get("timeEnd") {
        Some(value) => auto_checkin::normalize_time(Some(value)).map_err(|error| error.message)?,
        None => existing_entry.as_ref().map(|old| {
            times::end_with_spread(&time, times::spread_minutes(old))
        }).unwrap_or_else(|| times::default_time_end(&time)),
    };
    if !times::valid_range(&time, &time_end) {
        return Err("签到区间结束时间不能早于开始时间；暂不支持跨天区间".to_string());
    }
    let schedule = Schedule {
        id,
        label,
        enabled,
        time,
        time_end,
        providers: picked,
        provider_times,
        retry_time,
        retry_gap_minutes,
        retry_limit,
        last_fired_date,
        last_fired_providers,
        last_retried_providers,
        account_plan: existing_entry.as_ref().map(|old| old.account_plan.clone()).unwrap_or_default(),
        last_result,
    };
    let mut outcome = Err("定时条目保存失败".to_string());
    auto_checkin::update_state(|state| {
        let mut entries = state.get("schedules").and_then(Value::as_array).cloned().unwrap_or_default();
        let index = entries.iter().position(|raw| raw.get("id").and_then(Value::as_str) == Some(schedule.id.as_str()));
        if index.is_none() && entries.len() >= MAX_SCHEDULES {
            outcome = Err(format!("定时条目最多 {MAX_SCHEDULES} 条"));
            return;
        }
        let current = index.map(|index| normalize_schedule(&entries[index]))
            .or_else(|| (!state.contains_key("schedules") && schedule.id == DEFAULT_SCHEDULE_ID).then(|| legacy_schedule_of(state)));
        let mut schedule = schedule;
        if let Some(current) = current {
            if payload.get("enabled").is_none() { schedule.enabled = current.enabled; }
            if payload.get("providerTimes").is_none() { schedule.provider_times = current.provider_times; }
            if payload.get("retryTime").is_none() { schedule.retry_time = current.retry_time; }
            if payload.get("retryGapMinutes").is_none() { schedule.retry_gap_minutes = current.retry_gap_minutes; }
            if payload.get("retryLimit").is_none() { schedule.retry_limit = current.retry_limit; }
            schedule.last_fired_date = current.last_fired_date;
            schedule.last_fired_providers = current.last_fired_providers;
            schedule.last_retried_providers = current.last_retried_providers;
            schedule.account_plan = current.account_plan;
            schedule.last_result = current.last_result;
        }
        match index {
            Some(index) => entries[index] = to_value(&schedule),
            None => entries.push(to_value(&schedule)),
        }
        state.insert("schedules".to_string(), Value::Array(entries));
        outcome = Ok(schedule);
    });
    outcome
}

/// 删除一条条目。删除合成条目（或最后一条）写 `[]`：条目键自此存在，
/// 「从旧字段合成」不再发生——删光了就是删光了，不该再长回来。
pub fn remove(id: &str) -> Result<(), String> {
    let mut outcome = Err(format!("定时条目不存在：{id}"));
    auto_checkin::update_state(|state| {
        let mut entries = state.get("schedules").and_then(Value::as_array).cloned().unwrap_or_default();
        let before = entries.len();
        entries.retain(|raw| raw.get("id").and_then(Value::as_str) != Some(id));
        if entries.len() < before || (!state.contains_key("schedules") && id == DEFAULT_SCHEDULE_ID) {
            state.insert("schedules".to_string(), Value::Array(entries));
            outcome = Ok(());
        }
    });
    outcome
}

/// 条目级的当天去重标记：`fire` 在执行前落（先落再执行，进程被杀也不会反复补签）。
/// `date` 给 None = 清掉标记（Qoder 国际版的活动窗口未开、今天保持未落账等重试）。
///
/// 存量兼容：`schedules` 键不存在时写的是**旧字段**（`autoCheckin.lastFiredDate`）
/// ——旧模式不引入结构变更，与改造前的落点逐字一致。
pub fn mark_fired(id: &str, date: Option<&str>) {
    patch_entry(id, |schedule| {
        schedule.last_fired_date = date.map(str::to_string)
    });
}

/// 逐家记下「这家今天的主轮跑过了」。
///
/// 与 [`mark_fired`] 并存：那个是升级前的整体标记（现在只作存量兜底与旧读方
/// 兼容），逐家标记才是真正的去重依据 —— 同一家一天只跑一次主轮，不同家各按
/// 各自的时刻跑（见 `auto_checkin::times`）。
pub fn mark_fired_providers(id: &str, providers: &[String], date: &str) {
    if providers.is_empty() {
        return;
    }
    patch_entry(id, |schedule| {
        for provider in providers {
            schedule
                .last_fired_providers
                .insert(provider.clone(), date.to_string());
        }
        schedule.last_fired_date = Some(date.to_string());
    });
}

/// 逐家记下「这家今天的补签轮跑过了」。
pub fn mark_retried_providers(id: &str, providers: &[String], date: &str) {
    if providers.is_empty() {
        return;
    }
    patch_entry(id, |schedule| {
        for provider in providers {
            schedule
                .last_retried_providers
                .insert(provider.clone(), date.to_string());
        }
    });
}

/// 条目级的上一轮结果：成功 / 失败分支各落一次（与 auto_checkin 的 lastResult
/// 同构）；`None` = 清掉。
pub fn mark_result(id: &str, result: Option<Value>) {
    patch_entry(id, |schedule| {
        schedule.last_result = result.filter(|value| value.is_object())
    });
}

/// 改一条条目的去重状态（只动 `apply` 碰的字段）。
/// 条目键不存在（老配置）且删的是默认条目时落到旧字段，不引入结构变更。
fn patch_entry<F: FnOnce(&mut Schedule)>(id: &str, apply: F) {
    auto_checkin::update_state(|state| {
        if !state.contains_key("schedules") {
            if id != DEFAULT_SCHEDULE_ID { return; }
            let mut schedule = legacy_schedule_of(state);
            apply(&mut schedule);
            let value = to_value(&schedule);
            for key in ["accountPlan", "lastFiredDate", "lastFiredProviders", "lastRetriedProviders", "lastResult"] {
                state.insert(key.to_string(), value[key].clone());
            }
            return;
        }
        let mut entries = state.get("schedules").and_then(Value::as_array).cloned().unwrap_or_default();
        let Some(index) = entries.iter().position(|raw| raw.get("id").and_then(Value::as_str) == Some(id)) else { return; };
        let mut schedule = normalize_schedule(&entries[index]);
        apply(&mut schedule);
        entries[index] = to_value(&schedule);
        state.insert("schedules".to_string(), Value::Array(entries));
    });
}

/* ─── 一次性补齐：新加入的家默认在范围内 ───────────────────
 *
 * 背景与「为什么不能每次读都补」写在 `auto_checkin::PROVIDERS_VERSION` 上；
 * 这里只补充两条落地细节：
 *   · 补齐按 `CHECKIN_PROVIDERS` 的注册顺序归一（与 upsert 同一口径），
 *     免得补进来的那几家排在清单末尾、界面上勾选项顺序与别处不一致；
 *   · 老配置（还没固化过条目）**不去固化**，改顶层的 `providers` 字段 ——
 *     「读时合成、不迁移文件」是 `checkin_schedules` 的既有性质，本项不动它。 */

/// 配置里记的「签到范围已补到第几版」（缺失 = 1）
fn providers_version() -> i64 {
    auto_checkin::raw_object()
        .get("providersVersion")
        .and_then(Value::as_i64)
        .unwrap_or(1)
}

/// 清单按注册顺序归一（只留认识的家、去重）
fn ordered_providers(picked: &[String]) -> Vec<String> {
    auto_checkin::checkin_providers()
        .into_iter()
        .filter(|id| picked.iter().any(|item| item == id))
        .collect()
}

/// 把「加入版本晚于配置里这一版」的家补进每一条条目的范围，然后记下版本。
/// 返回是否真的改动了清单（调用方据此打一行日志）。幂等：版本到位后直接返回。
pub fn extend_providers_once() -> bool {
    let version = providers_version();
    if version >= auto_checkin::PROVIDERS_VERSION {
        return false;
    }
    let added = auto_checkin::providers_added_after(version);
    let mut changed = false;
    if added.is_empty() {
        // 版本被回退过（装过更新的版本又装回来）：只记账，不动清单
        record_providers_version();
        return false;
    }
    match stored_entries() {
        Some(entries) => {
            let mut next = entries;
            for raw in next.iter_mut() {
                let mut schedule = normalize_schedule(raw);
                let mut providers = schedule.providers.clone();
                for id in &added {
                    if !providers.iter().any(|item| item == id) {
                        providers.push(id.clone());
                    }
                }
                if providers.len() != schedule.providers.len() {
                    schedule.providers = ordered_providers(&providers);
                    *raw = to_value(&schedule);
                    changed = true;
                }
            }
            if changed {
                write_entries(next);
            }
        }
        None => {
            let legacy = ordered_providers(
                &auto_checkin::normalize_providers(auto_checkin::raw_object().get("providers")),
            );
            let mut providers = legacy.clone();
            for id in &added {
                if !providers.iter().any(|item| item == id) {
                    providers.push(id.clone());
                }
            }
            if providers.len() != legacy.len() {
                auto_checkin::write_state(json!({ "providers": ordered_providers(&providers) }));
                changed = true;
            }
        }
    }
    record_providers_version();
    changed
}

/// 记下「签到范围已补到当前版本」（不动其余字段）
fn record_providers_version() {
    auto_checkin::write_state(json!({ "providersVersion": auto_checkin::PROVIDERS_VERSION }));
}

/// 插件首次声明每日签到时仅补一次；用户之后取消的范围不能被重新勾选。
/// 对已上线且明确声明不支持每日签到的插件，清理其历史留存标记以免下架后继续占位。
pub fn extend_plugin_providers_once() -> bool {
    let available: Vec<String> = auto_checkin::checkin_providers()
        .into_iter()
        .filter(|id| !auto_checkin::CHECKIN_PROVIDERS.contains(&id.as_str()))
        .collect();
    let known: Vec<String> = auto_checkin::raw_object()
        .get("pluginProvidersSeen")
        .and_then(Value::as_array)
        .into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    let retained: Vec<String> = known
        .iter()
        .filter(|id| {
            !crate::server::core::plugin_host::serves_provider(id)
                || auto_checkin::plugin_supports_daily_checkin(id)
        })
        .cloned()
        .collect();
    let pruned = retained.len() != known.len();
    let added: Vec<String> = available.into_iter().filter(|id| !retained.contains(id)).collect();
    if added.is_empty() {
        if pruned {
            auto_checkin::write_state(json!({ "pluginProvidersSeen": retained }));
        }
        return false;
    }
    let mut changed = false;
    match stored_entries() {
        Some(mut entries) => {
            for raw in &mut entries {
                let mut schedule = normalize_schedule(raw);
                let mut row_changed = false;
                for id in &added {
                    if !schedule.providers.contains(id) {
                        schedule.providers.push(id.clone());
                        row_changed = true;
                    }
                }
                if row_changed {
                    if let Some(object) = raw.as_object_mut() {
                        object.insert("providers".into(), json!(schedule.providers));
                    }
                    changed = true;
                }
            }
            if changed { write_entries(entries); }
        }
        None => {
            let mut providers = legacy_schedule().providers;
            for id in &added {
                if !providers.contains(id) { providers.push(id.clone()); changed = true; }
            }
            if changed { auto_checkin::write_state(json!({ "providers": providers })); }
        }
    }
    let mut seen = retained;
    seen.extend(added);
    auto_checkin::write_state(json!({ "pluginProvidersSeen": seen }));
    changed
}
