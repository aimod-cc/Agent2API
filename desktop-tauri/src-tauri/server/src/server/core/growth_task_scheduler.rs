//! WorkBuddy **成长任务**的每日定点排程（国内版）。
//!
//! ── 这个模块解决什么问题 ─────────────────────────────────────
//! 成长任务原本只有手动入口（签到页的「一键完成」）：前端拿任务清单，在本地
//! 循环逐条调 `POST /api/accounts/{id}/growth/task`。用户得记得每天点一下，
//! 而其中**时段敏感**的任务（夜猫子 `black_cat`，仅 23:00-08:00 CST 可点亮）
//! 更是要求「半夜还得起来点」——这恰恰是自动化最该接手的一类。
//!
//! ── 与猫猫旅行排程（`core::growth_scheduler`）的关系 ──────────
//! 两者都属「WorkBuddy 国内版成长体系」，但**互不相干**：旅行是「派出 / 领奖」
//! 的状态机（`travel.rs`），本模块是「点亮 / 领奖」的任务链（`process_task`）。
//! 参考项目同样是两套独立排程（`travel_hours` 与成长任务各自配）。因此配置键
//! 各自独立（这里是 `growthTaskSchedule`，旅行是 `growthSchedule`），开关与
//! 时刻互不影响。
//!
//! ── 为什么不是 `core::scheduled_tasks` 里的一条 ───────────────
//! 那个模块是**等间隔重复**型（`{enabled, interval}`，间隔以分钟计）。本模块
//! 是**每天定点**型（时刻数组 + 当天去重 + 启动补跑），与 `auto_checkin` /
//! `growth_scheduler` 同一形状。硬塞进间隔型清单会逼着两边都变形 —— 那两处
//! 模块头已经为同样的理由写过说明。
//!
//! ── 默认关闭（与旅行相反，这是刻意的）────────────────────────
//! 旅行默认开启（参考项目零值即启用），因为它只是「推进一个既有的猫猫状态机」，
//! 行为温和。成长任务则**会发上报事件、领积分**，是一串写操作；对上游而言
//! 「每天自动把所有任务点亮一遍」比「一天两趟旅行」更容易被风控注意。因此这里
//! 缺省 `enabled: false`，由用户显式打开 —— 保守优先，与 `notify` 的默认关
//! 同一取舍（没配好就动是打扰）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::account_store::AccountStore;
use crate::server::core::beijing;
use crate::server::core::providers::workbuddy::growth::{self, spec::spec_of};
use crate::server::logging;

/// 配置键（config.json 里的字段名）。
pub const CONFIG_KEY: &str = "growthTaskSchedule";

/// 缺省时刻：晚上一趟即可 —— 夜猫子窗口 23:00 起，21:00 的那趟若只跑夜猫子
/// 会被时段判定挡掉（`black_cat` 非窗口跳过），所以缺省给 23:30 这个确实
/// 落在夜猫窗口内的时刻。用户可自行增删。
pub const DEFAULT_TIMES: [&str; 1] = ["23:30"];

/// 时刻条目上限：再多列表难以扫读。
pub const MAX_TIMES: usize = 6;

/// 轮询间隔：与 `auto_checkin` / `growth_scheduler` 的 30 秒同口径。
pub const TICK_MS: u64 = 30_000;

/// 账号间错峰（毫秒）：与旅行的 `ACCOUNT_GAP_MS` 同一量级。整批账号串行，
/// 逐条调用上游；错峰避免「整点齐发」被风控注意到。
const ACCOUNT_GAP_MS: u64 = 800;

/// 一条时刻的归一化结果（`HH:MM`）。
fn normalize_time(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim().to_string();
    let (hour, minute) = text.split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(format!("{hour:02}:{minute:02}"))
}

/// 读配置（缺失/损坏一律回落缺省：**默认关闭**，见模块头）。
///
/// 返回 `(enabled, times)`。`times` 恒非空（空数组回落缺省）—— 一个开着的
/// 排程没有任何时刻等于「永远不跑」，那是配置错误而不是用户意图。
pub fn settings() -> (bool, Vec<String>) {
    let raw = config::current()
        .raw()
        .get(CONFIG_KEY)
        .cloned()
        .unwrap_or(Value::Null);
    let enabled = raw
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let times: Vec<String> = raw
        .get("times")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| normalize_time(Some(item)))
                .collect()
        })
        .unwrap_or_default();
    let times = if times.is_empty() {
        DEFAULT_TIMES.iter().map(|text| text.to_string()).collect()
    } else {
        let mut sorted = times;
        sorted.sort();
        sorted.dedup();
        sorted
    };
    (enabled, times)
}

/// 写配置（`{enabled?, times?}`，只改传进来的字段）。
pub fn configure(patch: &Value) -> Result<Value, String> {
    let mut current = config::current()
        .raw()
        .get(CONFIG_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(enabled) = patch.get("enabled").and_then(Value::as_bool) {
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
        current.insert(
            "times".to_string(),
            Value::Array(times.into_iter().map(Value::String).collect()),
        );
    }
    // 当天去重标记：改了时刻就清掉，让新时刻当天就能生效
    current.insert("lastFiredDate".to_string(), Value::Null);
    current.insert("firedTimes".to_string(), Value::Null);
    config::update_raw_field(CONFIG_KEY, Value::Object(current));
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
    json!({
        "enabled": enabled,
        "times": times,
        "firedToday": fired_today,
        "lastFiredDate": if last_fired_date.is_empty() { Value::Null } else { Value::String(last_fired_date) },
        "lastFiredTime": raw.get("lastFiredTime").cloned().unwrap_or(Value::Null),
        "lastResult": raw.get("lastResult").cloned().unwrap_or(Value::Null),
        "running": is_running(),
    })
}

// ─── 运行状态 ───────────────────────────────────────────────

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

/// 一轮执行的结果摘要（写 `lastResult` 与日志）。
fn summarize(reports: &[Value]) -> Value {
    let mut actions: Vec<Value> = Vec::new();
    let mut credit = 0i64;
    let mut energy = 0i64;
    let mut done = 0usize;
    let mut failed = 0usize;
    for (name, rows) in reports.iter().filter_map(|report| {
        let name = report.get("name")?.as_str()?.to_string();
        let rows = report.get("results")?.as_array()?.clone();
        Some((name, rows))
    }) {
        for row in &rows {
            let outcome = row.get("outcome").and_then(Value::as_str).unwrap_or("");
            credit += row.get("credit").and_then(Value::as_i64).unwrap_or(0);
            energy += row.get("energy").and_then(Value::as_i64).unwrap_or(0);
            match outcome {
                "done" => done += 1,
                "already" => done += 1,
                "failed" => failed += 1,
                // skipped（未映射 / 未解锁 / 非夜猫窗口）不算动作，不进摘要 ——
                // 那是常态，写进去只是噪音
                _ => continue,
            }
            actions.push(json!({
                "name": name,
                "code": row.get("code").and_then(Value::as_str).unwrap_or(""),
                "outcome": outcome,
                "message": row.get("message").and_then(Value::as_str).unwrap_or(""),
                "credit": row.get("credit").and_then(Value::as_i64).unwrap_or(0),
                "energy": row.get("energy").and_then(Value::as_i64).unwrap_or(0),
            }));
        }
    }
    json!({
        "at": logging::now_ms(),
        "accounts": reports.len(),
        "actions": actions,
        "credit": credit,
        "energy": energy,
        "done": done,
        "failed": failed,
    })
}

/// 对一个国内版账号跑一轮成长任务。
///
/// ── 挑选口径（与前端「一键完成」逐字对齐）──────────────────
///   1. `supported`（在映射表里）—— 未映射任务保守跳过（`process_task` 内部
///      也会再判一次，这里先筛掉省一次网络往返）；
///   2. 非 `unforgeable`（不可伪造任务跳过）；
///   3. 未 `locked`（未到上线时间）；
///   4. `accept_status != claimed`（已领的幂等跳过，`process_task` 也判）。
/// 时段敏感任务（夜猫子）的窗口判定**在 `process_task` 里**（`black_cat` 非
/// 窗口即跳过），本函数不重复判 —— 那一处的窗口口径是权威。
async fn run_for_account(store: &AccountStore, account: &Value) -> Option<Value> {
    let id = account.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())?;
    // 门控：只服务国内版个人账号（企业版没有成长体系，国际版没有 CN 任务中心）
    // —— 与 `travel::run_for_accounts` 同一口径。
    if !growth::supports_growth(account) {
        return None;
    }
    if account
        .get("enterpriseId")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        return None;
    }
    let name = account
        .get("name")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .or_else(|| account.get("nickname").and_then(Value::as_str))
        .unwrap_or(id)
        .to_string();
    let Some(growth_account) = growth::GrowthAccount::from_store(store, id) else {
        return None;
    };
    // 两个口径都拉（小程序限定任务只在带 mp 头的列表里下发），合并去重
    let mut tasks: Vec<Value> = growth::list_tasks(&growth_account, false).await.ok()?;
    if let Ok(mp_tasks) = growth::list_tasks(&growth_account, true).await {
        let known: std::collections::HashSet<String> = tasks
            .iter()
            .filter_map(|task| task.get("task_code").and_then(Value::as_str).map(str::to_string))
            .collect();
        for task in mp_tasks {
            let code = task.get("task_code").and_then(Value::as_str).unwrap_or("").to_string();
            if !code.is_empty() && !known.contains(&code) {
                tasks.push(task);
            }
        }
    }
    let targets: Vec<Value> = tasks
        .into_iter()
        .filter(|task| {
            let code = task.get("task_code").and_then(Value::as_str).unwrap_or("");
            let Some(spec) = spec_of(code) else { return false };
            if spec.unforgeable {
                return false;
            }
            if task.get("locked").and_then(Value::as_bool).unwrap_or(false) {
                return false;
            }
            let status = task.get("accept_status").and_then(Value::as_str).unwrap_or("");
            status != "claimed"
        })
        .collect();
    if targets.is_empty() {
        return None;
    }
    let mut results: Vec<Value> = Vec::new();
    for task in &targets {
        let code = task.get("task_code").and_then(Value::as_str).unwrap_or("");
        let report = growth::process_task(&growth_account, code, task).await;
        results.push(json!({
            "code": report.code,
            "outcome": report.outcome.as_str(),
            "message": report.message,
            "credit": report.credit,
            "energy": report.energy,
        }));
    }
    Some(json!({ "id": id, "name": name, "results": results }))
}

/// 跑一轮成长任务巡检（全部国内版账号，串行 + 错峰）。
///
/// `plan_time` 是**本次对应的计划时刻**（`HH:MM`）：去重标记必须按它落，
/// 不能按实际执行时刻 —— 到点后最多晚一个 tick（30 秒）才跑，拿实际时刻
/// 去比对计划时刻永远不相等，结果是每个 tick 都重跑一遍（与 `growth_scheduler`
/// 同一处 bug 教训）。
/// `reason` 只进日志（区分「到点触发」与「手动立即执行」）。
pub async fn run_once(store: &AccountStore, reason: &str, plan_time: Option<&str>) -> Option<Value> {
    let guard = acquire()?;
    logging::log("[GrowthTask]", &format!("⏰ 成长任务巡检开始（{reason}）"));

    let accounts: Vec<Value> = store
        .list_accounts()
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut reports: Vec<Value> = Vec::new();
    let mut first = true;
    for account in &accounts {
        if let Some(report) = run_for_account(store, account).await {
            if !first {
                tokio::time::sleep(Duration::from_millis(ACCOUNT_GAP_MS)).await;
            }
            first = false;
            reports.push(report);
        }
    }
    let summary = summarize(&reports);

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
    if raw.get("lastFiredDate").and_then(Value::as_str) != Some(today.as_str()) {
        fired.clear();
    }
    // 去重标记按**计划时刻**落（见函数头）；手动执行没有计划时刻，不写标记
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
    next.insert("lastResult".to_string(), summary.clone());
    config::update_raw_field(CONFIG_KEY, Value::Object(next));

    let done = summary["done"].as_u64().unwrap_or(0);
    let credit = summary["credit"].as_i64().unwrap_or(0);
    logging::log(
        "[GrowthTask]",
        &format!("成长任务巡检完成：{done} 项、+{credit} 积分"),
    );
    drop(guard);
    Some(summary)
}

/// 当天该跑的时刻里还有没跑过的 → Some(那一条)，否则 None。
fn due_time(now: chrono::DateTime<chrono::FixedOffset>) -> Option<String> {
    let (enabled, times) = settings();
    if !enabled {
        return None;
    }
    let today = beijing::today_key();
    let raw = config::current()
        .raw()
        .get(CONFIG_KEY)
        .cloned()
        .unwrap_or(Value::Null);
    let fired: Vec<String> = if raw.get("lastFiredDate").and_then(Value::as_str) == Some(today.as_str()) {
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
    times
        .into_iter()
        .find(|time| time.as_str() <= now_text.as_str() && !fired.iter().any(|item| item == time))
}

/// 调度循环（由 `ServerState::bootstrap` 起一次）。
pub fn spawn(store: AccountStore) {
    crate::spawn_task(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
            let now = beijing::now();
            let Some(plan_time) = due_time(now) else { continue };
            let _ = run_once(&store, &format!("到点触发（{plan_time}）"), Some(&plan_time)).await;
        }
    });
}

/// 手动立即执行（供 API 调用）。返回 None = 已有一轮在跑。
///
/// 手动执行**不**写计划时刻的去重标记（见 `run_once`）。
pub async fn run_now(store: &AccountStore) -> Option<Value> {
    run_once(store, "手动执行", None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_time_accepts_and_rejects() {
        assert_eq!(normalize_time(Some(&json!("23:30"))), Some("23:30".to_string()));
        assert_eq!(normalize_time(Some(&json!("7:0"))), Some("07:00".to_string()));
        assert_eq!(normalize_time(Some(&json!(" 08:05 "))), Some("08:05".to_string()));
        assert_eq!(normalize_time(Some(&json!("24:00"))), None);
        assert_eq!(normalize_time(Some(&json!("12:60"))), None);
        assert_eq!(normalize_time(Some(&json!(2330))), None);
        assert_eq!(normalize_time(None), None);
    }

    /// 缺省时刻必须落在夜猫窗口（23:00-08:00）内 —— 否则开箱即用时那一趟
    /// 会被 `black_cat` 的时段判定整个挡掉，用户看到「跑了但一项没完成」。
    #[test]
    fn default_time_is_inside_night_window() {
        let hour: u32 = DEFAULT_TIMES[0]
            .split(':')
            .next()
            .and_then(|text| text.parse().ok())
            .expect("缺省时刻必须是 HH:MM");
        assert!(hour >= 23 || hour < 8, "缺省时刻 {hour} 点不在夜猫窗口内");
    }

    /// 去重标记必须按**计划时刻**比对（与 `growth_scheduler` 同一个真实 bug）：
    /// 按实际执行时刻落标记会永远对不上，于是每个 tick 都重跑一遍。
    #[test]
    fn due_time_matches_plan_time_not_actual_time() {
        let times = ["07:00".to_string(), "23:30".to_string()];
        let fired = vec!["07:00".to_string()];
        let now_text = "08:00".to_string();
        let due = times
            .iter()
            .find(|time| time.as_str() <= now_text.as_str() && !fired.iter().any(|item| *item == **time));
        // 07:00 已跑过、23:30 未到 → 无事可做
        assert_eq!(due, None);
        // 若标记落的是实际执行时刻（07:01），会误判成「没跑过」→ 重复执行
        let wrong = vec!["07:01".to_string()];
        let due_wrong = times
            .iter()
            .find(|time| time.as_str() <= now_text.as_str() && !wrong.iter().any(|item| *item == **time));
        assert_eq!(due_wrong.map(|time| time.as_str()), Some("07:00"));
    }

    /// 多个时刻同时过期时逐轮补跑（一轮一条，下个 tick 再补下一条）。
    #[test]
    fn due_time_picks_earliest_unfired_past_slot() {
        let times = ["07:00".to_string(), "12:00".to_string(), "23:30".to_string()];
        let now_text = "23:59".to_string();
        let fired: Vec<String> = Vec::new();
        let due = times
            .iter()
            .find(|time| time.as_str() <= now_text.as_str() && !fired.iter().any(|item| *item == **time));
        assert_eq!(due.map(|time| time.as_str()), Some("07:00"));
        let fired = vec!["07:00".to_string()];
        let due = times
            .iter()
            .find(|time| time.as_str() <= now_text.as_str() && !fired.iter().any(|item| *item == **time));
        assert_eq!(due.map(|time| time.as_str()), Some("12:00"));
    }
}