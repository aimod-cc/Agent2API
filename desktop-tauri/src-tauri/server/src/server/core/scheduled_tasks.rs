//! 间隔型定时任务注册表与调度循环。
//!
//! ── 两类任务（区别是**谁来执行**，不是可配性）─────────────────
//!   - `Runner::Backend`：凭证自动维护、模型目录刷新、软件版本检查。
//!     后端循环执行，因此有「上次执行 / 下次执行 / 立即执行」这些运行状态。
//!   - `Runner::Frontend`：日志页 / 请求日志页 / 报表页的自动刷新。定时器天然长在
//!     页面上（只在页面可见时该走），后端只存开关与间隔，界面自己读。
//!
//! ── 本模块只做「何时做」，不做事 ─────────────────────────────
//! 每条任务的动作在各自模块里（`credential_maintenance` /
//! `providers::catalog_refresh` / `update::check`），本文件负责把它们按配置的
//! 开关与间隔串起来 —— 与 `auto_checkin` 同一分工。
//!
//! ── 余额查询为什么不在这个清单里 ─────────────────────────────
//! 它已退役成**每账号各自配置**的自动查询（间隔与「余额不足处理」都在账号
//! 设置里配），调度与判定在 `core::usage_query` / `core::usage_records`：
//! 「每账号独立间隔」与这里的「全局一条任务」不是同一个形状，硬塞进
//! `{enabled, interval}` 会逼着两边都变形（与自动签到不在此清单的理由同理）。
//!
//! ── 排期为什么不在这个模块里 ─────────────────────────────────
//! 「上次尝试 / 上次成功 / 下次执行 / 失败冷却 / 在途占位」全部委托给
//! `core::task_state`（落库、跨重启、跨实例）。本模块的循环因此每次 tick 都
//! 从**持久化状态**重算「到点了吗」，而不是在内存里记 `nextRunAt` —— 改造前
//! 的形态正是后者，于是每次启动都把首轮排到「现在」，重启几次就把上游打几次。
//!
//! ── 调度方式：轮询判定，不是「睡满间隔」────────────────────────
//! 与 `auto_checkin` 同一取舍：循环每 `TICK_MS` 醒一次，每次重新判定。两个后果
//! 都是想要的：
//!   - **改完设置下一轮就生效**（最多慢一个 tick）：睡满间隔的写法在把间隔从
//!     60 分钟改到 1 分钟时，最坏要等 60 分钟才醒来 —— 界面显示「已生效」而实际
//!     没有，正是最难排查的那种不一致；
//!   - **机器休眠 / 锁屏 / 改时钟后自愈**：单调时钟在休眠期间不推进，
//!     醒来的第一次 tick 比一下墙上时钟就把错过的时点补上（只补一次，
//!     不补跑期间的每一轮）。
//!
//! ── 自动签到为什么不在这个清单里 ─────────────────────────────
//! 它是**每天定点**型（可指定 00:01 这类时刻），与本模块的「等间隔重复」不是
//! 同一个形状：时刻（JSON 字符串）、当天去重（`lastFiredDate`）、启动补签都是
//! 它独有的语义。硬塞进 `{enabled, interval}` 会逼着两边都变形。
//! 界面上它仍在同一页展示，只是读改走 `/api/auto-checkin`。

use std::time::Duration;

use serde_json::{json, Value};

use crate::server::config;
use crate::server::config::IntervalTaskPatch;
use crate::server::core::account_store::AccountStore;
use crate::server::core::task_state::{self, TaskState};
use crate::server::core::update::UpdateManager;
use crate::server::logging;

/// 调度循环的判定间隔。
///
/// 10 秒：backend 任务的间隔下限是 1 分钟（见 `INTERVAL_MIN_MINUTES`），
/// 10 秒的判定粒度意味着「到点后最多晚 10 秒执行」——用户感知不到，
/// 而空转代价只是每 10 秒一次读库（排期在库里）。不做得更密：没有收益。
pub const TICK_MS: u64 = 10_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Runner {
    Backend,
    Frontend,
}

pub struct TaskDef {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub unit: &'static str,
    pub runner: Runner,
    pub min: i64,
    pub max: i64,
    pub default_interval: i64,
}

pub const TASK_CREDENTIAL_MAINTENANCE: &str = config::KEY_CREDENTIAL_MAINTENANCE;
pub const TASK_MODEL_REFRESH: &str = config::KEY_MODEL_REFRESH;
pub const TASK_UPDATE_CHECK: &str = config::KEY_UPDATE_CHECK;
pub const TASK_LOGS_AUTO_REFRESH: &str = config::KEY_LOGS_AUTO_REFRESH;
pub const TASK_REQUESTS_AUTO_REFRESH: &str = config::KEY_REQUESTS_AUTO_REFRESH;
pub const TASK_REPORT_AUTO_REFRESH: &str = config::KEY_REPORT_AUTO_REFRESH;

pub const TASKS: [TaskDef; 6] = [
    TaskDef {
        id: TASK_CREDENTIAL_MAINTENANCE,
        label: "凭证维护",
        description: "定期遍历账号，只刷新已过期或临期的凭证。重启后沿用上次排期，失败账号在冷却后重试；转发需要的按需续期不受此开关影响。",
        unit: "minutes",
        runner: Runner::Backend,
        min: config::INTERVAL_MIN_MINUTES,
        max: config::INTERVAL_MAX_MINUTES,
        default_interval: config::DEFAULT_CREDENTIAL_MAINTENANCE_MINUTES,
    },
    TaskDef {
        id: TASK_MODEL_REFRESH,
        label: "模型目录更新",
        description: "定期拉取最新模型清单，重启沿用排期与缓存。客户端读取模型列表的后台刷新也遵守本任务开关、间隔和失败冷却；手动获取模型可提前刷新。同一轮顺带刷新 models.dev 模型元数据（能力位与思考档位，可在设置页单独关闭）。",
        unit: "minutes",
        runner: Runner::Backend,
        min: config::INTERVAL_MIN_MINUTES,
        max: config::INTERVAL_MAX_MINUTES,
        default_interval: config::DEFAULT_MODEL_REFRESH_MINUTES,
    },
    TaskDef {
        id: TASK_UPDATE_CHECK,
        label: "软件版本检查",
        description: "定期向 GitHub 查询最新发布版本，默认每 20 分钟一次。检查结果、排期和限流冷却跨重启保留；手动检查也会合并重复请求并遵守冷却。开关与间隔在「软件更新」面板的「更新设置」弹窗里配置（定时任务页不再渲染这一条）。",
        unit: "minutes",
        runner: Runner::Backend,
        min: config::INTERVAL_MIN_MINUTES,
        max: config::INTERVAL_MAX_MINUTES,
        default_interval: config::DEFAULT_UPDATE_CHECK_MINUTES,
    },
    TaskDef {
        id: TASK_LOGS_AUTO_REFRESH,
        label: "日志页自动刷新",
        description: "停留在「日志」页时按此间隔重新拉取系统事件；页面不可见时不请求。",
        unit: "seconds",
        runner: Runner::Frontend,
        min: config::INTERVAL_MIN_SECONDS,
        max: config::INTERVAL_MAX_SECONDS,
        default_interval: config::DEFAULT_LOGS_AUTO_REFRESH_SECONDS,
    },
    TaskDef {
        id: TASK_REQUESTS_AUTO_REFRESH,
        label: "请求日志页自动刷新",
        description: "停留在「请求日志」页时按此间隔重新拉取请求日志；页面不可见时不请求。",
        unit: "seconds",
        runner: Runner::Frontend,
        min: config::INTERVAL_MIN_SECONDS,
        max: config::INTERVAL_MAX_SECONDS,
        default_interval: config::DEFAULT_REQUESTS_AUTO_REFRESH_SECONDS,
    },
    TaskDef {
        id: TASK_REPORT_AUTO_REFRESH,
        label: "报表自动刷新",
        description: "停留在「报表」页时按此间隔重新拉取统计数据；页面不可见时不请求。",
        unit: "seconds",
        runner: Runner::Frontend,
        min: config::INTERVAL_MIN_SECONDS,
        max: config::INTERVAL_MAX_SECONDS,
        default_interval: config::DEFAULT_REPORT_AUTO_REFRESH_SECONDS,
    },
];

pub fn find(id: &str) -> Option<&'static TaskDef> {
    TASKS.iter().find(|task| task.id == id)
}

fn settings_of(settings: config::ScheduledSettings, id: &str) -> config::IntervalTask {
    match id {
        TASK_CREDENTIAL_MAINTENANCE => settings.credential_maintenance,
        TASK_MODEL_REFRESH => settings.model_refresh,
        TASK_UPDATE_CHECK => settings.update_check,
        TASK_LOGS_AUTO_REFRESH => settings.logs_auto_refresh,
        TASK_REQUESTS_AUTO_REFRESH => settings.requests_auto_refresh,
        TASK_REPORT_AUTO_REFRESH => settings.report_auto_refresh,
        _ => config::IntervalTask { enabled: false, interval: 0 },
    }
}

fn interval_ms(task: &TaskDef) -> i64 {
    let interval = settings_of(config::scheduled_settings(), task.id).interval.max(1);
    interval * if task.unit == "seconds" { 1000 } else { 60_000 }
}

fn state_key(id: &str) -> String {
    if id == TASK_UPDATE_CHECK {
        crate::server::core::update::global_check_key()
    } else {
        id.to_string()
    }
}

fn task_json(task: &TaskDef, state: Result<TaskState, String>) -> Value {
    let settings = settings_of(config::scheduled_settings(), task.id);
    let backend = task.runner == Runner::Backend;
    let error = state.as_ref().err().cloned();
    let state = state.unwrap_or_default();
    let timestamp = |at: i64| if at > 0 { json!(at) } else { Value::Null };
    json!({
        "id": task.id,
        "label": task.label,
        "description": task.description,
        "unit": task.unit,
        "runner": if backend { "backend" } else { "frontend" },
        "enabled": settings.enabled,
        "interval": settings.interval,
        "min": task.min,
        "max": task.max,
        "defaultInterval": task.default_interval,
        "running": backend && state.running(),
        "lastRunAt": timestamp(state.last_run_at),
        "lastAttemptAt": timestamp(state.last_attempt_at),
        "lastSuccessAt": timestamp(state.last_success_at),
        "lastResult": error.or_else(|| state.last_result.clone()),
        "lastError": state.last_error,
        "retryAt": timestamp(state.retry_at),
        "nextRunAt": if backend && settings.enabled { timestamp(state.due_at().max(logging::now_ms())) } else { Value::Null },
        "canRun": backend,
    })
}

pub fn list() -> Value {
    // 一次读出全部状态：本接口被「定时任务」页每 20 秒轮询一次，
    // 逐条任务各读一次库（四次读 + 四次解析）没有意义。
    let states = task_state::read_all();
    json!({
        "tasks": TASKS
            .iter()
            .map(|task| {
                let state = match &states {
                    Ok(states) if task.runner == Runner::Backend => {
                        Ok(states.get(&state_key(task.id)).cloned().unwrap_or_default())
                    }
                    Ok(_) => Ok(TaskState::default()),
                    Err(error) => Err(error.clone()),
                };
                task_json(task, state)
            })
            .collect::<Vec<_>>(),
    })
}

pub fn task_by_id(id: &str) -> Value {
    find(id)
        .map(|task| {
            let state = if task.runner == Runner::Backend {
                task_state::read(&state_key(task.id))
            } else {
                Ok(TaskState::default())
            };
            task_json(task, state)
        })
        .unwrap_or(Value::Null)
}

pub fn configure(id: &str, patch: IntervalTaskPatch) -> Result<Value, String> {
    let task = find(id).ok_or_else(|| format!("未知的定时任务: {id}"))?;
    if patch.enabled.is_none() && patch.interval.is_none() {
        return Err("没有需要更新的字段".to_string());
    }
    if let Some(interval) = patch.interval {
        if !(task.min..=task.max).contains(&interval) {
            let unit = if task.unit == "seconds" { "秒" } else { "分钟" };
            return Err(format!("「{}」的间隔必须是 {}–{} {}", task.label, task.min, task.max, unit));
        }
    }
    let before = settings_of(config::scheduled_settings(), id);
    let plugins_before = (id == TASK_UPDATE_CHECK).then(crate::server::core::plugin_host::settings::current);
    let saved = config::set_scheduled_task(id, patch, task.min, task.max);
    let after = settings_of(config::scheduled_settings(), id);
    if let Some(before) = plugins_before {
        crate::server::core::plugin_host::settings::notify_changed(before);
    }
    if task.runner == Runner::Backend {
        if !before.enabled && after.enabled {
            // 刚开启：保留改造前「开启即跑一次」的语义（下一个 tick 就执行），
            // 而不是按上次尝试时间算出的旧排期干等 —— 那正是用户拨开关时最
            // 意外的一种反应。失败冷却仍然优先（`due_at` 取两者的较大值）。
            task_state::schedule_now(&state_key(id))?;
        } else if before.interval != after.interval {
            // 改间隔：按**新**间隔从上次尝试起重新计时（不额外提前一次 ——
            // 要马上跑有「立即执行」按钮）
            task_state::reschedule(&state_key(id), interval_ms(task))?;
        }
        if id == TASK_MODEL_REFRESH && before.interval != after.interval {
            crate::server::core::providers::catalog_refresh::reschedule(interval_ms(task))?;
        }
    }
    if before != after {
        logging::log("[Tasks]", &format!("定时任务「{}」已{}，间隔 {} {}", task.label,
            if after.enabled { "开启" } else { "关闭" }, after.interval,
            if task.unit == "seconds" { "秒" } else { "分钟" }));
    }
    let mut result = task_by_id(id);
    result["saved"] = json!(saved);
    Ok(result)
}

pub async fn run_now(store: &AccountStore, update: &UpdateManager, id: &str) -> Result<String, String> {
    let task = find(id).ok_or_else(|| format!("未知的定时任务: {id}"))?;
    if task.runner != Runner::Backend {
        return Err(format!("「{}」由界面自己刷新，无法在后端立即执行", task.label));
    }
    run_backend(store, update, task, true).await
}

async fn run_backend(
    store: &AccountStore,
    update: &UpdateManager,
    task: &TaskDef,
    manual: bool,
) -> Result<String, String> {
    // 更新管理器同时承接设置页按钮，排期必须属于它而不是外围定时器。
    if task.id == TASK_UPDATE_CHECK {
        let current = crate::server::core::update::CURRENT_VERSION;
        let info = if manual { update.check(current).await } else { update.check_scheduled(current).await }
            .map_err(|error| error.message)?;
        return Ok(if info.get("hasUpdate").and_then(Value::as_bool) == Some(true) {
            let summary = format!("发现新版本 {}（当前 {current}）", info["latestVersion"].as_str().unwrap_or(""));
            logging::log("[Update]", &summary);
            summary
        } else { "已完成版本检查".to_string() });
    }
    if matches!(task.id, TASK_CREDENTIAL_MAINTENANCE | TASK_MODEL_REFRESH) {
        let detail = crate::server::core::maintenance::execute(
            store,
            crate::server::core::maintenance::Request::full(task.id, manual),
        ).await?;
        return Ok(crate::server::core::maintenance::summary(&detail));
    }
    Err("未知后端任务".to_string())
}

/// 调度循环：把后端任务按各自配置的间隔重复执行（由 `ServerState::bootstrap`
/// 起一次，进程内只有这一个循环）。
///
/// 循环体只做三件事：读配置 → 看谁到点了 → 跑它。所有排期状态都在库里
/// （`core::task_state`），因此**重启不重置节奏**：上次 10:00 查过更新
/// （间隔 20 分钟），10:05 / 10:10 反复重启都不会再查，仍等到 10:20；
/// 停机期间错过的那一轮则在启动后补跑一次（只补一次，不补跑每一轮）。
///
/// 每 tick 读一次**全部**任务的状态（而不是逐条各读一次）：一次读库换四条
/// 任务的判定，空转成本与改造前的「两次读锁」同一量级。数据库不可用时整轮
/// 跳过 —— 那时发不出任何可信的判定，宁可这一轮什么都不做。
pub fn spawn(store: AccountStore, update: UpdateManager) {
    crate::spawn_task(async move {
        // 首轮先按持久化状态恢复排期：从未跑过的任务排到「现在」（于是全新安装
        // 启动即跑一次，与改造前一致），跑过的沿用「上次尝试 + 间隔」——
        // 已经到期的自然落到过去，这一轮就补上。
        for task in TASKS.iter().filter(|task| task.runner == Runner::Backend) {
            if let Err(error) = task_state::reschedule(&state_key(task.id), interval_ms(task)) {
                logging::log("[Tasks]", &format!("恢复「{}」排期失败：{error}", task.label));
            }
        }
        loop {
            let Ok(states) = task_state::read_all() else {
                tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
                continue;
            };
            for task in TASKS.iter().filter(|task| task.runner == Runner::Backend) {
                if !settings_of(config::scheduled_settings(), task.id).enabled {
                    continue;
                }
                // 状态损坏恢复后可能缺少条目，按首次运行交给 claim 重建排期。
                let state = states.get(&state_key(task.id));
                // 在跑（长任务）或还没到点（含失败冷却 / 占位租约）都跳过
                if state.is_some_and(|state| state.running() || logging::now_ms() < state.due_at()) {
                    continue;
                }
                // 到点：跑一次（`run_backend` 内部按当前间隔重新排期）。
                // 被跳过（另一进程抢到、或在最短间隔内）不算错误，只留 verbose。
                if let Err(error) = run_backend(&store, &update, task, false).await {
                    logging::verbose("[Tasks]", &format!("{}：{error}", task.label));
                }
            }
            tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
        }
    });
}
