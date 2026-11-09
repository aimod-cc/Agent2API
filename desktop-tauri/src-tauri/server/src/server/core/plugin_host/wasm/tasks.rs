use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use chrono::Timelike;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::server::core::{account_store::AccountStore, task_state::{self, Claim, ManualBackoff}};
use crate::server::{config, logging};
use super::super::{Host, registry};

#[derive(Clone, Deserialize, Serialize)]
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
        if task.legacy_config.as_deref().is_some_and(|key| key != "growthSchedule") {
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

fn overrides(provider: &str, id: &str) -> Value {
    config::with_raw(|raw| raw.get("pluginTaskSettings").and_then(|all| all.get(provider))
        .and_then(|items| items.get(id)).cloned()).flatten().unwrap_or(Value::Null)
}

fn current(provider: &str, task: &Definition) -> Definition {
    let mut task = task.clone();
    let mut value = overrides(provider, &task.id);
    if value.is_null() {
        if let Some(key) = &task.legacy_config {
            value = config::with_raw(|raw| raw.get(key).cloned()).flatten().unwrap_or(Value::Null);
        }
    }
    if let Some(enabled) = value["enabled"].as_bool() { task.enabled = enabled; }
    if let Some(items) = value["times"].as_array() {
        let times: Vec<_> = items.iter().filter_map(Value::as_str).filter_map(super::super::schedules::normalize_time).collect();
        if !times.is_empty() { task.times = times; task.interval_ms = None; }
    }
    task
}

pub(crate) fn snapshot() -> Value {
    let table = definitions().lock().map(|table| table.clone()).unwrap_or_default();
    let states = task_state::read_all().unwrap_or_default();
    json!(table.into_iter().filter(|(provider, _)| registry::serves(provider)).flat_map(|(provider, tasks)| {
        let states = &states;
        tasks.into_iter().filter(|task| !task.checkin).map(move |task| {
            let task = current(&provider, &task);
            let prefix = format!("pluginTask:{provider}:{}:", task.id);
            let runs: Vec<_> = states.iter().filter(|(key, _)| key.starts_with(&prefix))
                .map(|(key, state)| json!({ "account": key.strip_prefix(&prefix), "running": state.running(),
                    "lastRunAt": state.last_run_at, "nextRunAt": state.next_run_at, "error": state.last_error, "result": state.last_result })).collect();
            json!({ "provider": provider, "task": task, "runs": runs })
        }).collect::<Vec<_>>()
    }).collect::<Vec<_>>())
}

pub(crate) fn configure(provider: &str, id: &str, patch: &Value) -> Result<(), String> {
    if !super::has_capability(provider, "tasks.v1") || !registry::serves(provider) { return Err("插件任务当前不可用".into()); }
    let task = definitions().lock().map_err(|_| "任务列表不可用")?.get(provider)
        .and_then(|tasks| tasks.iter().find(|task| task.id == id && !task.checkin)).cloned().ok_or("任务不存在")?;
    if !patch.is_object() || patch.as_object().is_some_and(|map| map.keys().any(|key| !matches!(key.as_str(), "enabled" | "times")))
        || patch.get("enabled").is_some_and(|value| !value.is_boolean()) { return Err("任务设置无效".into()); }
    if let Some(times) = patch.get("times") {
        let times = times.as_array().ok_or("任务时刻必须是数组")?;
        if task.interval_ms.is_some() || times.is_empty() || times.len() > 24
            || times.iter().any(|time| time.as_str().and_then(super::super::schedules::normalize_time).is_none()) {
            return Err("任务时刻无效".into());
        }
    }
    let mut all = config::with_raw(|raw| raw.get("pluginTaskSettings").cloned()).flatten().unwrap_or_else(|| json!({}));
    if !all[provider].is_object() { all[provider] = json!({}); }
    if !all[provider][id].is_object() { all[provider][id] = json!({}); }
    for (key, value) in patch.as_object().ok_or("任务设置无效")? { all[provider][id][key] = value.clone(); }
    config::update_raw_field("pluginTaskSettings", all);
    super::super::schedules::wake();
    Ok(())
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
    if !super::has_capability(provider, "tasks.v1") || !registry::serves(provider) { return Err("插件任务当前不可用".into()); }
    let task = definitions().lock().map_err(|_| "任务列表不可用")?.get(provider)
        .and_then(|tasks| tasks.iter().find(|task| task.id == id && !task.checkin)).cloned().ok_or("任务不存在")?;
    let task = current(provider, &task);
    if !manual && !task.enabled { return Ok(json!({ "skipped": true })); }
    let mut results = Vec::new();
    for account in store.accounts_for_provider(provider) {
        let Some(account) = account["id"].as_str() else { continue };
        let Some(adapter) = registry::acquire(provider).await.map_err(|error| error.message)? else { break };
        let host = adapter.host();
        if host.options.wasm.as_ref().is_none_or(|options| !options.allows("tasks.v1")) { break; }
        if !manual && !current(provider, &task).enabled { break; }
        let now = logging::now_ms();
        let (due, interval, planned_at) = interval(&task, now);
        let key = format!("pluginTask:{provider}:{id}:{account}");
        let previous = task_state::read(&key)?;
        if !manual && (!due || planned_at > 0 && previous.last_attempt_at >= planned_at && previous.failures == 0) { continue; }
        let guard = match task_state::claim(&key, interval, manual, ManualBackoff::Respect, 1000)? {
            Claim::Acquired(guard) => guard, Claim::Deferred(_) => continue,
        };
        let result = host.call_coded("tasks.run", json!({ "provider": provider, "account": account,
            "task": id, "manual": manual, "scheduled_at": planned_at }), Duration::from_secs(600)).await;
        let (success, message) = match result {
            Ok(value) if value["success"].is_boolean() => (value["success"] == true,
                value["message"].as_str().unwrap_or("任务已结束").chars().take(1000).collect::<String>()),
            Ok(_) => (false, "插件未返回有效的任务结果".into()),
            Err(error) => (false, super::super::process::redact(&error.message)),
        };
        guard.finish(success, message.clone(), None, 0, interval)?;
        results.push(json!({ "account": account, "success": success, "message": message }));
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(json!({ "results": results }))
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
