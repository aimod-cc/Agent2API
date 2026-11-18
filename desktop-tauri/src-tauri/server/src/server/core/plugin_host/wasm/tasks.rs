mod category;
mod execution;
mod settings;
mod travel;

pub(crate) use category::Category;
pub(crate) use travel::{configure as configure_travel, run as run_travel, state as travel_state};

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use chrono::Timelike;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::server::core::{account_store::AccountStore, task_state};
use crate::server::logging;
use super::super::{Host, registry};

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Definition {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub times: Vec<String>,
    #[serde(default)]
    pub interval_ms: Option<i64>,
    #[serde(default = "zone")]
    pub utc_offset_minutes: i32,
    #[serde(default)]
    pub checkin: bool,
    #[serde(default)]
    pub legacy_config: Option<String>,
    #[serde(skip)]
    pub category: Category,
}
fn zone() -> i32 { 480 }

fn definitions() -> &'static Mutex<HashMap<String, Vec<Definition>>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Vec<Definition>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) async fn load(host: &Host, provider: &str) -> Result<(), String> {
    if host.options.wasm.as_ref().is_none_or(|options| !options.allows("tasks.v1")) {
        return Err("插件未声明任务能力".into());
    }
    let declared = host.call_coded("tasks.list", json!({ "provider": provider }), Duration::from_secs(10)).await
        .map_err(|error| error.message)?;
    let mut tasks: Vec<Definition> = serde_json::from_value(declared).map_err(|_| "插件任务声明格式无效")?;
    if tasks.len() > 32 { return Err("插件任务数量超过上限".into()); }
    let mut ids = std::collections::HashSet::new();
    let mut checkin = false;
    for task in &mut tasks {
        if task.checkin && (checkin || task.interval_ms.is_some() || task.times.len() != 1) {
            return Err("签到窗口只能声明一条每日时刻".into());
        }
        checkin |= task.checkin;
        if task.legacy_config.as_deref().is_some_and(|key| key != settings::LEGACY_TRAVEL
            || provider != "workbuddy" || task.id != "travel" || task.utc_offset_minutes != 480
            || task.interval_ms.is_some()) {
            return Err("任务不允许读取其他主程序配置".into());
        }
        if task.id.is_empty() || task.id.len() > 64 || !task.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
            || !ids.insert(task.id.clone()) || task.label.is_empty() || task.label.len() > 256
            || !(-720..=840).contains(&task.utc_offset_minutes) || task.times.len() > 24
            || task.interval_ms.is_some_and(|ms| !(60_000..=30 * 86_400_000).contains(&ms))
            || task.interval_ms.is_some() == !task.times.is_empty() {
            return Err("插件任务名称或排程无效".into());
        }
        for time in &mut task.times { *time = super::super::schedules::normalize_time(time).ok_or("插件任务时刻无效")?; }
        task.times.sort(); task.times.dedup();
    }
    let categories = category::load(host, provider, &tasks).await;
    for task in &mut tasks { task.category = categories.get(&task.id).copied().unwrap_or_default(); }
    definitions().lock().map_err(|_| "插件任务列表不可用")?.insert(provider.into(), tasks);
    Ok(())
}

pub(crate) fn forget(provider: &str) {
    if let Ok(mut table) = definitions().lock() { table.remove(provider); }
}

pub(crate) fn checkin_time(provider: &str) -> Option<String> {
    let table = definitions().lock().ok()?;
    let task = table.get(provider)?.iter().find(|task| task.checkin)?;
    let time = chrono::NaiveTime::parse_from_str(task.times.first()?, "%H:%M").ok()?;
    let minutes = (time.num_seconds_from_midnight() as i32 / 60 + 480 - task.utc_offset_minutes).rem_euclid(1440);
    Some(format!("{:02}:{:02}", minutes / 60, minutes % 60))
}

fn definition(provider: &str, id: &str) -> Option<Definition> {
    definitions().lock().ok()?.get(provider)?.iter().find(|task| task.id == id && !task.checkin).cloned()
}

fn current(provider: &str, task: &Definition) -> Definition { settings::current(provider, task) }

pub(crate) fn snapshot() -> Value {
    let table = definitions().lock().map(|table| table.clone()).unwrap_or_default();
    let records = task_state::read_all();
    let unavailable = if !settings::readable() {
        Some("任务配置暂不可读，请刷新状态；不会发送任务")
    } else if records.is_err() {
        Some("任务记录暂不可读，请刷新状态；不会发送任务")
    } else { None };
    let states = records.unwrap_or_default();
    let mut rows: Vec<_> = table.into_iter().filter(|(provider, _)| registry::serves(provider)).flat_map(|(provider, tasks)| {
        let states = &states;
        tasks.into_iter().filter(|task| !task.checkin).map(move |task| {
            let task = current(&provider, &task);
            let prefix = format!("pluginTask:{provider}:{}:", task.id);
            let mut runs: Vec<_> = states.iter().filter(|(key, _)| key.starts_with(&prefix))
                .map(|(key, state)| {
                    let running = state.running();
                    let interrupted = !running && state.last_attempt_at > state.last_run_at;
                    json!({ "account": key.strip_prefix(&prefix),
                        "name": state.value.as_ref().and_then(|value| value.get("name")), "running": running,
                        "interrupted": interrupted, "lastRunAt": state.last_run_at, "nextRunAt": state.next_run_at,
                        "error": if interrupted { Some("上次执行中断，结果未确认；不会自动重发任务") } else { state.last_error.as_deref() },
                        "result": if interrupted { None } else { state.last_result.as_deref() } })
                }).collect();
            runs.sort_by(|left, right| left["account"].as_str().cmp(&right["account"].as_str()));
            json!({ "provider": provider, "providerLabel": crate::server::core::providers::label_of(&provider),
                "category": task.category, "unavailable": unavailable,
                "operation": execution::snapshot(states.get(&execution::key(&provider, &task.id))),
                "task": task, "runs": runs })
        }).collect::<Vec<_>>()
    }).collect();
    rows.sort_by(|left, right| left["provider"].as_str().cmp(&right["provider"].as_str()));
    json!(rows)
}

pub(crate) fn checkin_snapshot() -> Value {
    let snapshot = snapshot();
    json!(snapshot.as_array().into_iter().flatten().filter(|row| {
        serde_json::from_value::<Category>(row["category"].clone()).is_ok_and(Category::belongs_to_checkin)
    }).cloned().collect::<Vec<_>>())
}

pub(crate) fn configure(provider: &str, id: &str, patch: &Value) -> Result<(), String> {
    if !super::has_capability(provider, "tasks.v1") || !registry::serves(provider) { return Err("插件任务当前不可用".into()); }
    let task = definition(provider, id).ok_or("任务不存在")?;
    settings::save(provider, &task, patch)
}

fn interval(task: &Definition, now: i64) -> (bool, i64, i64) {
    if let Some(interval) = task.interval_ms { return (true, interval, 0); }
    let offset = i64::from(task.utc_offset_minutes) * 60_000;
    let local = now + offset;
    let day = local.div_euclid(86_400_000) * 86_400_000 - offset;
    let times: Vec<i64> = task.times.iter().filter_map(|time| {
        let parsed = chrono::NaiveTime::parse_from_str(time, "%H:%M").ok()?;
        Some(day + i64::from(parsed.num_seconds_from_midnight()) * 1000)
    }).collect();
    let due = times.iter().copied().filter(|at| *at <= now).max().unwrap_or(0);
    let next = times.iter().copied().filter(|at| *at > now).min()
        .or_else(|| times.first().map(|at| at + 86_400_000)).unwrap_or(now + 86_400_000);
    (due > 0, (next - now).max(1000), due)
}

pub(crate) async fn run(store: &AccountStore, provider: &str, id: &str, manual: bool) -> Result<Value, String> {
    execution::run(store, provider, id, manual).await
}

pub(crate) async fn launch(store: AccountStore, provider: &str, id: &str) -> Result<Value, String> {
    execution::launch(store, provider, id).await
}

struct RunningProvider {
    provider: String,
    running: std::sync::Arc<Mutex<std::collections::HashSet<String>>>,
}
impl Drop for RunningProvider {
    fn drop(&mut self) { if let Ok(mut running) = self.running.lock() { running.remove(&self.provider); } }
}

pub(crate) fn spawn(store: AccountStore) {
    let running = std::sync::Arc::new(Mutex::new(std::collections::HashSet::new()));
    crate::spawn_task(async move {
        loop {
            let table = definitions().lock().map(|table| table.clone()).unwrap_or_default();
            for (provider, tasks) in table {
                if !registry::serves(&provider) || !super::has_capability(&provider, "tasks.v1") { continue; }
                if !running.lock().is_ok_and(|mut running| running.insert(provider.clone())) { continue; }
                let store = store.clone();
                let running = running.clone();
                crate::spawn_task(async move {
                    let _guard = RunningProvider { provider: provider.clone(), running };
                    for task in tasks.into_iter().filter(|task| !task.checkin) {
                        if let Err(error) = run(&store, &provider, &task.id, false).await {
                            logging::verbose("[PluginTask]", &error);
                        }
                    }
                });
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}
