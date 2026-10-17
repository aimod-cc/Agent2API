//! 定时签到的**多条目**配置（`autoCheckin.schedules` 数组）。
//!
//! ── 形状 ────────────────────────────────────────────────────
//! 每条条目是一次「定点签到」的完整配置：
//!   `{id, label, enabled, time, jitterMinutes, providers, lastFiredDate, lastResult}`
//!   - `id`     稳定标识（界面按它 upsert / 删除 / 立即执行；合成条目是 "default"）
//!   - `label`  展示名（可空；空时展示退到 `time`）
//!   - `enabled` 条目自己的开关
//!   - `time`   HH:MM，北京时间口径（见 `core::beijing`，issue #138）
//!   - `jitterMinutes` 随机前后 N 分钟（0 = 不随机）；窗口与账号铺开的语义见
//!     `core::auto_checkin` 的模块头「随机窗口」
//!   - `providers` 这条要签的提供商（`CHECKIN_PROVIDERS` 子集，至少一家）
//!   - `lastFiredDate` / `lastResult` 条目级的当天去重与上一轮结果
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

use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::auto_checkin::{self, CHECKIN_PROVIDERS};

/// 条目上限：再多列表难以扫读，定点签到的语义也不需要更多
pub const MAX_SCHEDULES: usize = 10;
/// 合成默认条目的 id（存量配置没有 `schedules` 时的那条）
pub const DEFAULT_SCHEDULE_ID: &str = "default";

/// 一条定点签到条目（读侧形状；字段缺失逐个兜底，与 auto_checkin 同一口径）
#[derive(Clone, Debug)]
pub struct Schedule {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub time: String,
    /// 随机前后 N 分钟（0 = 不随机，时刻即触发点）。窗口与账号铺开见
    /// `auto_checkin` 的「随机窗口」说明。
    pub jitter_minutes: u32,
    pub providers: Vec<String>,
    pub last_fired_date: Option<String>,
    pub last_result: Option<Value>,
}

/// 读侧的随机分钟归一：缺省 / 非数字 / 负数 / 小数一律当 0，超上限夹取。
///
/// 读侧刻意宽容（与 `time` 坏了回落 `DEFAULT_TIME` 同一原则）：配置是用户
/// 手改过也可能被旧版本写过，读不出来时退到「不随机」这个安全行为，
/// 而不是让整条条目失效。写入侧的校验更严（见 `upsert`），非法值当场拒绝。
pub fn normalize_jitter(value: Option<&Value>) -> u32 {
    let Some(number) = value.and_then(Value::as_u64) else {
        return 0;
    };
    u32::try_from(number)
        .unwrap_or(auto_checkin::MAX_JITTER_MINUTES)
        .min(auto_checkin::MAX_JITTER_MINUTES)
}

/// 写入侧的随机分钟校验：`None` 表示「这次请求没带该字段」（保持原值），
/// `Some(0)` 是**合法的显式值**（关闭随机）——两者必须能区分开。
///
/// 非法值当场拒绝而不是悄悄夹取：用户填 999 却看到保存成功、实际只按 120
/// 生效，比直接报错更难排查。错误文案与 `normalize_time` 的既有风格一致。
fn parse_jitter(value: Option<&Value>) -> Result<Option<u32>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    // `as_u64` 已经挡住了负数、小数、字符串与 null，这里只剩「超上限」要判
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|minutes| *minutes <= auto_checkin::MAX_JITTER_MINUTES)
        .map(Some)
        .ok_or_else(|| {
            format!(
                "随机分钟应为 0 - {} 的整数",
                auto_checkin::MAX_JITTER_MINUTES
            )
        })
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
    match config::current().raw().get(auto_checkin::CONFIG_KEY) {
        Some(Value::Object(map)) => map.contains_key("schedules"),
        _ => false,
    }
}

/// 存储里的条目数组（原文；键不存在给 None）
fn stored_entries() -> Option<Vec<Value>> {
    auto_checkin::raw_object()
        .get("schedules")
        .and_then(Value::as_array)
        .cloned()
}

/// 单条的归一化：id / label / enabled / time / jitter / providers 逐个兜底。
/// time 坏了回落 DEFAULT_TIME（调度起不来比时刻错更糟，与 auto_checkin 同）。
fn normalize_schedule(raw: &Value) -> Schedule {
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
    let jitter_minutes = normalize_jitter(raw.get("jitterMinutes"));
    let providers = auto_checkin::normalize_providers(raw.get("providers"));
    Schedule {
        id,
        label,
        enabled,
        time,
        jitter_minutes,
        providers,
        last_fired_date: raw
            .get("lastFiredDate")
            .and_then(Value::as_str)
            .map(str::to_string),
        last_result: raw.get("lastResult").filter(|value| value.is_object()).cloned(),
    }
}

/// 存量兼容：用旧字段合成的那条默认条目（`schedules` 键缺失时）
fn legacy_schedule() -> Schedule {
    let raw = auto_checkin::raw_object();
    normalize_schedule(&json!({
        "id": DEFAULT_SCHEDULE_ID,
        "enabled": raw.get("enabled").cloned().unwrap_or(Value::Null),
        "time": raw.get("time").cloned().unwrap_or(Value::Null),
        "providers": raw.get("providers").cloned().unwrap_or(Value::Null),
        "lastFiredDate": raw.get("lastFiredDate").cloned().unwrap_or(Value::Null),
        "lastResult": raw.get("lastResult").cloned().unwrap_or(Value::Null),
    }))
}

/// 全部条目（调度循环与快照的组装原料）。
/// `schedules` 键缺失 → 旧字段合成的默认条目；存在（含 `[]`）→ 存储为准。
pub fn list() -> Vec<Schedule> {
    match stored_entries() {
        Some(items) if !items.is_empty() => items.iter().map(normalize_schedule).collect(),
        Some(_) => Vec::new(),
        None => vec![legacy_schedule()],
    }
}

// ─── 写回 ───────────────────────────────────────────────────

/// 条目数组写回 `autoCheckin.schedules`（其余字段原样保留）
fn write_entries(entries: Vec<Value>) {
    let mut merged = auto_checkin::raw_object();
    merged.insert("schedules".to_string(), Value::Array(entries));
    config::update_raw_field(auto_checkin::CONFIG_KEY, Value::Object(merged));
}

/// 条目 → 存储原文（保留 lastFiredDate / lastResult，条目级的去重状态跨重启）。
///
/// ⚠️ 新增字段必须同时加到 `normalize_schedule` 与本函数：`patch_entry` 是
/// 「读原文 → normalize → 改 → to_value 全量写回」，漏一处就会让 `mark_fired`
/// 每次把该字段静默清掉。
fn to_value(schedule: &Schedule) -> Value {
    json!({
        "id": schedule.id,
        "label": schedule.label,
        "enabled": schedule.enabled,
        "time": schedule.time,
        "jitterMinutes": schedule.jitter_minutes,
        "providers": schedule.providers,
        "lastFiredDate": schedule.last_fired_date,
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
    let jitter_minutes = parse_jitter(payload.get("jitterMinutes"))?;
    // 范围：只认注册过的提供商（去重、按注册顺序）；一条不勾到点也无账号可签
    let list_value = payload.get("providers");
    let picked: Vec<String> = CHECKIN_PROVIDERS
        .iter()
        .filter(|id| {
            list_value
                .and_then(Value::as_array)
                .map(|items| items.iter().any(|item| item.as_str() == Some(*id)))
                .unwrap_or(false)
        })
        .map(|id| id.to_string())
        .collect();
    if picked.is_empty() {
        return Err("至少勾选一家签到提供商".to_string());
    }
    let label = payload
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut entries = stored_entries().unwrap_or_default();
    let existing = entries.iter().position(|raw| {
        raw.get("id").and_then(Value::as_str) == Some(id.as_str())
    });
    if existing.is_none() && entries.len() >= MAX_SCHEDULES {
        return Err(format!("定时条目最多 {MAX_SCHEDULES} 条"));
    }
    let existing_entry = existing.map(|index| normalize_schedule(&entries[index]));
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
    // 随机分钟同样「缺省保持原值、新建默认 0」：界面的条目开关按钮是整表单
    // 重发，漏带这个字段时不能把它清零（见前端 `toggleScheduleEnabled`）。
    let jitter_minutes = match jitter_minutes {
        Some(minutes) => minutes,
        None => existing_entry
            .as_ref()
            .map(|old| old.jitter_minutes)
            .unwrap_or(0),
    };
    // 更新已有条目时保留它的去重状态；新建条目从「今天还没跑」开始
    let (last_fired_date, last_result) = existing_entry
        .map(|old| (old.last_fired_date, old.last_result))
        .unwrap_or((None, None));
    // 存量兼容：第一次编辑合成的默认条目时，把旧字段的去重状态带过来
    let (last_fired_date, last_result) = if last_fired_date.is_none() && !stored_key_exists() && id == DEFAULT_SCHEDULE_ID {
        let legacy = legacy_schedule();
        (legacy.last_fired_date, legacy.last_result)
    } else {
        (last_fired_date, last_result)
    };
    let schedule = Schedule {
        id,
        label,
        enabled,
        time,
        jitter_minutes,
        providers: picked,
        last_fired_date,
        last_result,
    };
    match existing {
        Some(index) => entries[index] = to_value(&schedule),
        None => entries.push(to_value(&schedule)),
    }
    write_entries(entries);
    Ok(schedule)
}

/// 删除一条条目。删除合成条目（或最后一条）写 `[]`：条目键自此存在，
/// 「从旧字段合成」不再发生——删光了就是删光了，不该再长回来。
pub fn remove(id: &str) -> Result<(), String> {
    let mut entries = stored_entries().unwrap_or_default();
    let before = entries.len();
    entries.retain(|raw| raw.get("id").and_then(Value::as_str) != Some(id));
    if entries.len() == before {
        // 存量兼容：删除的可能是旧字段合成的默认条目（不在存储里）
        if !stored_key_exists() && id == DEFAULT_SCHEDULE_ID {
            write_entries(Vec::new());
            return Ok(());
        }
        return Err(format!("定时条目不存在：{id}"));
    }
    write_entries(entries);
    Ok(())
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
    if !stored_key_exists() {
        if id != DEFAULT_SCHEDULE_ID {
            return;
        }
        let mut schedule = legacy_schedule();
        apply(&mut schedule);
        auto_checkin::write_state(json!({
            "lastFiredDate": schedule.last_fired_date,
            "lastResult": schedule.last_result,
        }));
        return;
    }
    let mut entries = stored_entries().unwrap_or_default();
    let Some(index) = entries
        .iter()
        .position(|raw| raw.get("id").and_then(Value::as_str) == Some(id))
    else {
        return;
    };
    let mut schedule = normalize_schedule(&entries[index]);
    apply(&mut schedule);
    entries[index] = to_value(&schedule);
    write_entries(entries);
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
    CHECKIN_PROVIDERS
        .iter()
        .filter(|id| picked.iter().any(|item| item == *id))
        .map(|id| id.to_string())
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

#[cfg(test)]
mod tests {
    //! 随机分钟字段的归一与往返。
    //!
    //! 重点钉住 `normalize_schedule` ↔ `to_value` 的对称性：`patch_entry` 是
    //! 「读原文 → normalize → 改 → to_value 全量写回」，漏了任何一侧，
    //! `mark_fired` 都会把 `jitterMinutes` 静默清掉（用户只在第二天发现随机
    //! 没了，极难排查）。这些函数不碰全局 config，可并行跑。
    use super::*;
    use serde_json::json;

    fn roundtrip(raw: Value) -> Value {
        to_value(&normalize_schedule(&raw))
    }

    #[test]
    fn jitter_roundtrips_through_storage() {
        let stored = roundtrip(json!({
            "id": "s1",
            "enabled": true,
            "time": "07:20",
            "jitterMinutes": 45,
            "providers": ["trae"],
        }));
        assert_eq!(stored.get("jitterMinutes").and_then(Value::as_u64), Some(45));
    }

    #[test]
    fn missing_jitter_reads_as_zero() {
        let schedule = normalize_schedule(&json!({
            "id": "s1",
            "time": "07:20",
            "providers": ["trae"],
        }));
        assert_eq!(schedule.jitter_minutes, 0);
    }

    #[test]
    fn invalid_jitter_reads_as_zero_or_clamps() {
        // 读侧宽容：手改配置写坏了不能整条条目失效
        assert_eq!(normalize_jitter(Some(&json!(null))), 0);
        assert_eq!(normalize_jitter(Some(&json!("abc"))), 0);
        assert_eq!(normalize_jitter(Some(&json!(-5))), 0);
        assert_eq!(normalize_jitter(None), 0);
        // 超上限夹取（不报错）
        assert_eq!(
            normalize_jitter(Some(&json!(9999))),
            auto_checkin::MAX_JITTER_MINUTES
        );
    }

    #[test]
    fn write_side_rejects_invalid_jitter() {
        // 写入侧严格：用户填 999 却按 120 生效比直接报错更难排查。
        // None = 「没带这个字段」→ 保持原值，与 Some(0) 必须区分开。
        assert_eq!(parse_jitter(None).unwrap(), None);
        assert_eq!(parse_jitter(Some(&json!(0))).unwrap(), Some(0));
        assert_eq!(parse_jitter(Some(&json!(120))).unwrap(), Some(120));
        assert!(parse_jitter(Some(&json!(121))).is_err());
        assert!(parse_jitter(Some(&json!(-1))).is_err());
        assert!(parse_jitter(Some(&json!(1.5))).is_err());
        assert!(parse_jitter(Some(&json!("30"))).is_err());
    }
}
