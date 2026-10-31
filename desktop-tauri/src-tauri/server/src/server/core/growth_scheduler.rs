//! 猫猫旅行排程的宿主编排：计时、用户配置、当天去重与执行记录。
//!
//! 默认开关、每日时刻和执行动作由插件的 `schedules.list` 提供。
//! 未安装、停用或不支持排程的插件不进入执行路径；插件加载与配置更新会唤醒循环。
//! `growthSchedule` 是已发布的配置键，保留它以兼容用户已有的开关与时刻。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::account_store::AccountStore;
use crate::server::core::beijing;
use crate::server::core::plugin_host::{self, schedules};
use crate::server::errors::GatewayError;
use crate::server::logging;

/// 配置键（config.json 里的字段名）。
pub const CONFIG_KEY: &str = "growthSchedule";

const PROVIDER: &str = "workbuddy";
const SCHEDULE_ID: &str = "travel";

pub use schedules::MAX_TIMES;

/// 轮询间隔：与 `auto_checkin` 的 30 秒同口径（到点后最多晚 30 秒执行）。
pub const TICK_MS: u64 = 30_000;

/// 时刻条目归一化为 `HH:MM`。
fn normalize_time(value: Option<&Value>) -> Option<String> {
    schedules::normalize_time(value?.as_str()?)
}

/// 用户明确保存的值优先；缺失字段采用已加载插件的默认值。
pub fn settings() -> (bool, Vec<String>) {
    let declared = schedules::definition(PROVIDER, SCHEDULE_ID);
    let raw = config::current()
        .raw()
        .get(CONFIG_KEY)
        .cloned()
        .unwrap_or(Value::Null);
    let enabled = raw
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            declared
                .as_ref()
                .is_some_and(|definition| definition.enabled)
        });
    let mut times: Vec<String> = raw
        .get("times")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| normalize_time(Some(item)))
                .collect()
        })
        .unwrap_or_default();
    if times.is_empty() {
        times = declared
            .map(|definition| definition.times)
            .unwrap_or_default();
    }
    times.sort();
    times.dedup();
    (enabled, times)
}

/// 写配置（`{enabled?, times?}`，只改传进来的字段）。
///
/// 与 `auto_checkin` 的 `write_state` 同构：合并写回，config.json 的其它字段
/// 原样保留。
pub fn configure(patch: &Value) -> Result<Value, String> {
    let (_, previous_times) = settings();
    let mut current = config::current()
        .raw()
        .get(CONFIG_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(value) = patch.get("enabled") {
        let Some(enabled) = value.as_bool() else {
            return Err("enabled 必须是布尔值".to_string());
        };
        current.insert("enabled".to_string(), Value::Bool(enabled));
    }
    if let Some(items) = patch.get("times") {
        let Some(items) = items.as_array() else {
            return Err("times 必须是时刻数组".to_string());
        };
        if items.len() > MAX_TIMES {
            return Err(format!("时刻最多 {MAX_TIMES} 条"));
        }
        let mut times: Vec<String> = Vec::new();
        for item in items {
            let Some(time) = normalize_time(Some(item)) else {
                return Err(format!(
                    "非法时刻「{}」（应为 HH:MM，00:00 - 23:59）",
                    item.as_str().unwrap_or("?")
                ));
            };
            if !times.contains(&time) {
                times.push(time);
            }
        }
        if times.is_empty() {
            return Err("至少保留一个时刻".to_string());
        }
        times.sort();
        if times != previous_times {
            current.insert("lastFiredDate".to_string(), Value::Null);
            current.insert("lastFiredTime".to_string(), Value::Null);
            current.remove("firedTimes");
        }
        current.insert(
            "times".to_string(),
            Value::Array(times.into_iter().map(Value::String).collect()),
        );
    }
    config::update_raw_field(CONFIG_KEY, Value::Object(current));
    schedules::wake();
    Ok(state())
}

/// 对外状态（设置页 / 签到页读它）。
pub fn state() -> Value {
    let (enabled, times) = settings();
    let raw = config::current()
        .raw()
        .get(CONFIG_KEY)
        .cloned()
        .unwrap_or(Value::Null);
    let today = beijing::today_key();
    let last_fired_date = raw
        .get("lastFiredDate")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    // 今天已经跑过的**计划时刻**（界面据此把已执行的条目标灰）。
    // 只认 `firedTimes`：那是按计划时刻落的标记（见 `run_once` 的说明），
    // 不回落到 `lastFiredTime`（实际执行时刻，与计划时刻不是同一个坐标系）。
    let fired_today: Vec<String> = if last_fired_date == today {
        raw.get("firedTimes")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let pause_reason = schedules::pause_reason(PROVIDER, SCHEDULE_ID);
    json!({
        "enabled": enabled,
        "available": pause_reason.is_none(),
        "pausedReason": pause_reason,
        "times": times,
        "firedToday": fired_today,
        "lastFiredDate": if last_fired_date.is_empty() { Value::Null } else { Value::String(last_fired_date) },
        "lastFiredTime": raw.get("lastFiredTime").cloned().unwrap_or(Value::Null),
        "lastResult": raw.get("lastResult").cloned().unwrap_or(Value::Null),
        "running": is_running(),
    })
}

// ─── 运行状态 ───────────────────────────────────────────────

/// 是否有一轮正在执行（跨进程内全局，与 auto_checkin 的 running 同构）。
fn running_flag() -> &'static AtomicBool {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    &RUNNING
}

pub fn is_running() -> bool {
    running_flag().load(Ordering::SeqCst)
}

/// 抢「执行中」标记的守卫：Drop 时复位（对应 Go 的 `defer`）。
struct RunGuard;

impl Drop for RunGuard {
    fn drop(&mut self) {
        running_flag().store(false, Ordering::SeqCst);
    }
}

fn acquire() -> Option<RunGuard> {
    if running_flag().swap(true, Ordering::SeqCst) {
        return None;
    }
    Some(RunGuard)
}

// ─── 执行 ───────────────────────────────────────────────────

/// 执行中、能力暂不可用和真正失败不能合并为同一个空结果。
pub enum RunOutcome {
    Completed(Value),
    Skipped(String),
    Busy,
    Failed(GatewayError),
}

/// 跑一轮巡检；去重按计划时刻落，手动执行不占用定时轮次。
pub async fn run_once(store: &AccountStore, reason: &str, plan_time: Option<&str>) -> RunOutcome {
    if let Some(message) = schedules::pause_reason(PROVIDER, SCHEDULE_ID) {
        return RunOutcome::Skipped(message);
    }
    let Some(definition) = schedules::definition(PROVIDER, SCHEDULE_ID) else {
        return RunOutcome::Skipped("当前提供商插件未申报这项排程，请更新提供商插件".to_string());
    };
    let Some(adapter) = plugin_host::lookup(PROVIDER) else {
        return RunOutcome::Skipped("未安装或未启用提供商插件".to_string());
    };
    let Some(_guard) = acquire() else {
        return RunOutcome::Busy;
    };
    logging::log("[Travel]", &format!("⏰ 猫猫旅行巡检开始（{reason}）"));
    let summary = match adapter
        .admin_action(store, "", &definition.action, &Value::Null)
        .await
    {
        Ok(summary) => summary,
        Err(error) if error.code.as_deref() == Some("plugin_updating") => {
            return RunOutcome::Skipped("提供商插件正在重新加载，本轮已跳过".to_string());
        }
        Err(error) if error.status_code == 501 => {
            let message = "当前提供商插件未实现这项排程，请更新提供商插件".to_string();
            schedules::suspend(PROVIDER, SCHEDULE_ID, message.clone());
            return RunOutcome::Skipped(message);
        }
        Err(error) => {
            // 每天定点的动作不应因失败变成高频重试；下个计划时刻或手动执行可再试。
            record_run(
                json!({
                    "at": logging::now_ms(),
                    "outcome": "failed",
                    "error": error.message,
                    "accounts": 0,
                    "actions": [],
                    "credit": 0,
                    "failed": 1,
                }),
                plan_time,
            );
            logging::log_with_level(
                "[Travel]",
                &format!("猫猫旅行巡检失败：{}", error.message),
                "error",
            );
            return RunOutcome::Failed(error);
        }
    };
    record_run(summary.clone(), plan_time);
    let action_count = summary
        .get("actions")
        .and_then(Value::as_array)
        .map(|items| items.len())
        .unwrap_or(0);
    let credit = summary.get("credit").and_then(Value::as_i64).unwrap_or(0);
    logging::log(
        "[Travel]",
        &format!("猫猫旅行巡检完成：{action_count} 个动作（+{credit}c）"),
    );
    RunOutcome::Completed(summary)
}

fn record_run(summary: Value, plan_time: Option<&str>) {
    // 落 lastResult + 当天的「已执行时刻」
    let today = beijing::today_key();
    let now_time = beijing::now().format("%H:%M").to_string();
    let raw = config::current()
        .raw()
        .get(CONFIG_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut fired: Vec<String> = raw
        .get("firedTimes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    // 跨天则重置（firedTimes 只在同一天内有效）
    if raw.get("lastFiredDate").and_then(Value::as_str) != Some(today.as_str()) {
        fired.clear();
    }
    // 去重标记按**计划时刻**落（见函数头）；手动执行没有计划时刻，
    // 不写标记 —— 手动跑过不等于当天的定时轮次已经跑过。
    if let Some(plan_time) = plan_time.filter(|text| !text.is_empty()) {
        if !fired.iter().any(|item| item == plan_time) {
            fired.push(plan_time.to_string());
        }
    }
    let mut next = raw;
    next.insert("lastFiredDate".to_string(), Value::String(today));
    next.insert("lastFiredTime".to_string(), Value::String(now_time));
    next.insert(
        "firedTimes".to_string(),
        Value::Array(fired.into_iter().map(Value::String).collect()),
    );
    next.insert("lastResult".to_string(), summary);
    config::update_raw_field(CONFIG_KEY, Value::Object(next));
}

/// 当天该跑的时刻里还有没跑过的 → Some(那一条)，否则 None。
fn due_time(now: chrono::DateTime<chrono::FixedOffset>) -> Option<String> {
    let (enabled, times) = settings();
    if !enabled || schedules::pause_reason(PROVIDER, SCHEDULE_ID).is_some() {
        return None;
    }
    let today = beijing::today_key();
    let raw = config::current()
        .raw()
        .get(CONFIG_KEY)
        .cloned()
        .unwrap_or(Value::Null);
    let fired: Vec<String> =
        if raw.get("lastFiredDate").and_then(Value::as_str) == Some(today.as_str()) {
            raw.get("firedTimes")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
    let now_text = now.format("%H:%M").to_string();
    // 取「已到点且今天没跑过」里最早的一条：多个时刻同时过期时逐轮补跑
    //（一轮只跑一次，下个 tick 再补下一条 —— 与 auto_checkin 的补签同语义）
    times
        .into_iter()
        .find(|time| time.as_str() <= now_text.as_str() && !fired.iter().any(|item| item == time))
}

/// 调度循环（由 `ServerState::bootstrap` 起一次）。
///
/// 每 `TICK_MS` 醒一次重新判定：改完设置下一轮就生效、机器休眠后自愈 ——
/// 与 `auto_checkin` / `scheduled_tasks` 同一取舍（详见那两个模块头）。
pub fn spawn(store: AccountStore) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::spawn_task(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(TICK_MS)) => {},
                _ = schedules::changed() => {},
            }
            let now = beijing::now();
            let Some(plan_time) = due_time(now) else {
                continue;
            };
            let _ = run_once(
                &store,
                &format!("到点触发（{plan_time}）"),
                Some(&plan_time),
            )
            .await;
        }
    });
}

// ─── 全局句柄（供 API 手动触发）─────────────────────────────

/// 手动执行不占用当天的定时轮次。
pub async fn run_now(store: &AccountStore) -> RunOutcome {
    run_once(store, "手动执行", None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_time_accepts_and_rejects() {
        assert_eq!(
            normalize_time(Some(&json!("09:00"))),
            Some("09:00".to_string())
        );
        assert_eq!(
            normalize_time(Some(&json!("9:5"))),
            Some("09:05".to_string())
        );
        assert_eq!(
            normalize_time(Some(&json!(" 21:30 "))),
            Some("21:30".to_string())
        );
        // 越界 / 非字符串 / 缺字段
        assert_eq!(normalize_time(Some(&json!("24:00"))), None);
        assert_eq!(normalize_time(Some(&json!("12:60"))), None);
        assert_eq!(normalize_time(Some(&json!(930))), None);
        assert_eq!(normalize_time(None), None);
    }

    /// 去重标记必须按**计划时刻**比对。
    ///
    /// 这条钉住的是一个真实 bug：`run_once` 曾按实际执行时刻（`now_time`）
    /// 落标记，而 `due_time` 拿计划时刻去查 —— 到点后最多晚 30 秒执行，
    /// 两者永不相等，于是每个 tick 都重跑一遍。
    #[test]
    fn due_time_matches_plan_time_not_actual_time() {
        let times = ["09:00".to_string(), "21:00".to_string()];
        let fired = vec!["09:00".to_string()];
        let now_text = "09:00".to_string();
        // 09:00 已跑过 → 21:00 还没到 → 无事可做
        let due = times.iter().find(|time| {
            time.as_str() <= now_text.as_str() && !fired.iter().any(|item| *item == **time)
        });
        assert_eq!(due, None);
        // 若把 fired 换成实际执行时刻（09:01），会误判成「没跑过」→ 重复执行
        let wrong_fired = vec!["09:01".to_string()];
        let due_wrong = times.iter().find(|time| {
            time.as_str() <= now_text.as_str() && !wrong_fired.iter().any(|item| *item == **time)
        });
        assert_eq!(due_wrong.map(|time| time.as_str()), Some("09:00"));
    }

    #[test]
    fn due_time_picks_earliest_unfired_past_slot() {
        let times = [
            "09:00".to_string(),
            "13:00".to_string(),
            "21:00".to_string(),
        ];
        // 现在 22:00、今天一个都没跑 → 先补最早的 09:00（一轮一条）
        let now_text = "22:00".to_string();
        let fired: Vec<String> = Vec::new();
        let due = times.iter().find(|time| {
            time.as_str() <= now_text.as_str() && !fired.iter().any(|item| *item == **time)
        });
        assert_eq!(due.map(|time| time.as_str()), Some("09:00"));
        // 09:00 跑过之后 → 13:00
        let fired = vec!["09:00".to_string()];
        let due = times.iter().find(|time| {
            time.as_str() <= now_text.as_str() && !fired.iter().any(|item| *item == **time)
        });
        assert_eq!(due.map(|time| time.as_str()), Some("13:00"));
    }
}
