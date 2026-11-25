//! 定时任务后台调度、手动执行/停止并发守卫、交互按钮执行与编辑器试跑。

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Notify;

use super::cron_schedule::next_run_ms;
use super::sandbox::{self, SandboxInput, SandboxOutcome};
use super::store;
use super::types::TaskPlugin;
use crate::server::logging;

const TICK_INTERVAL_SECS: u64 = 5;

struct RunningHandle {
    abort: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

fn running_registry() -> &'static Mutex<HashMap<String, RunningHandle>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, RunningHandle>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

struct RunningGuard {
    job_id: String,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = running_registry().lock() {
            map.remove(&self.job_id);
        }
    }
}

fn acquire_run(job_id: &str) -> Result<(RunningGuard, Arc<AtomicBool>, Arc<Notify>), String> {
    let mut map = running_registry()
        .lock()
        .map_err(|_| "任务运行表锁不可用".to_string())?;
    if map.contains_key(job_id) {
        return Err("该定时任务正在执行中，请稍候或先停止".to_string());
    }
    let abort = Arc::new(AtomicBool::new(false));
    let notify = Arc::new(Notify::new());
    map.insert(
        job_id.to_string(),
        RunningHandle {
            abort: abort.clone(),
            notify: notify.clone(),
        },
    );
    Ok((
        RunningGuard {
            job_id: job_id.to_string(),
        },
        abort,
        notify,
    ))
}

pub fn is_job_running(job_id: &str) -> bool {
    running_registry()
        .lock()
        .map(|map| map.contains_key(job_id))
        .unwrap_or(false)
}

pub fn stop_job(job_id: &str) -> Result<(), String> {
    let handle = running_registry()
        .lock()
        .map_err(|_| "任务运行表锁不可用".to_string())?
        .get(job_id)
        .map(|h| (h.abort.clone(), h.notify.clone()));
    let Some((abort, notify)) = handle else {
        return Err("该任务当前未在执行中".to_string());
    };
    abort.store(true, Ordering::SeqCst);
    notify.notify_waiters();
    Ok(())
}

fn merge_form_values(
    plugin: &TaskPlugin,
    plain: &BTreeMap<String, Value>,
    secrets: &BTreeMap<String, String>,
    overrides: Option<&BTreeMap<String, Value>>,
) -> BTreeMap<String, Value> {
    let mut form = BTreeMap::new();
    for field in &plugin.fields {
        if let Some(def) = &field.default_value {
            form.insert(field.key.clone(), def.clone());
        }
    }
    for (k, v) in plain {
        form.insert(k.clone(), v.clone());
    }
    for (k, v) in secrets {
        form.insert(k.clone(), Value::String(v.clone()));
    }
    if let Some(extra) = overrides {
        for (k, v) in extra {
            let is_secret = plugin
                .fields
                .iter()
                .any(|f| f.key == *k && (f.secret || f.field_type == "password"));
            if is_secret {
                if let Some(s) = v.as_str() {
                    if !s.is_empty() {
                        form.insert(k.clone(), Value::String(s.to_string()));
                    }
                }
            } else {
                form.insert(k.clone(), v.clone());
            }
        }
    }
    form
}

pub async fn execute_job(job_id: &str, trigger_type: &str) -> Result<SandboxOutcome, String> {
    let record = store::get_job_record(job_id)?
        .ok_or_else(|| "定时任务不存在".to_string())?;
    let plugin = store::get_plugin(&record.view.plugin_id)?
        .ok_or_else(|| "所属定时任务插件不存在或已卸载".to_string())?;
    if !plugin.enabled {
        return Err("所属定时任务插件已停用，请先启用插件".to_string());
    }

    let (_guard, abort, notify) = acquire_run(job_id)?;
    let trigger_label = if trigger_type == "manual" { "手动" } else { "定时" };
    let run_id = store::mark_run_started(job_id, trigger_type, trigger_label)?;

    let secrets = store::decrypt_job_secrets(&record).unwrap_or_default();
    let initial_storage = store::decrypt_job_storage(&record).unwrap_or_default();
    let form_values = merge_form_values(&plugin, &record.view.values, &secrets, None);

    let outcome = sandbox::execute(SandboxInput {
        plugin,
        job_id: record.view.id.clone(),
        job_name: record.view.name.clone(),
        trigger: trigger_type.to_string(),
        action_id: None,
        form_values,
        initial_storage,
        proxy_id: record.view.proxy_id.clone(),
        timeout_seconds: record.view.timeout_seconds,
        preview_mode: false,
        abort,
        abort_notify: notify,
    })
    .await;

    let _ = store::mark_run_finished(
        job_id,
        run_id,
        trigger_type,
        outcome.success,
        outcome.duration_ms,
        &outcome.output,
        outcome.error.as_deref(),
        outcome.status_patch.as_ref(),
        outcome.storage_dirty.then_some(&outcome.new_storage),
    );

    Ok(outcome)
}

pub async fn execute_action(
    job_id: &str,
    action_id: &str,
    draft_values: Option<BTreeMap<String, Value>>,
    draft_proxy_id: Option<String>,
) -> Result<SandboxOutcome, String> {
    let record = store::get_job_record(job_id)?
        .ok_or_else(|| "定时任务不存在".to_string())?;
    let plugin = store::get_plugin(&record.view.plugin_id)?
        .ok_or_else(|| "所属定时任务插件不存在或已卸载".to_string())?;
    if !plugin.enabled {
        return Err("所属定时任务插件已停用，请先启用插件".to_string());
    }
    let action_def = plugin
        .actions
        .iter()
        .find(|a| a.id == action_id)
        .ok_or_else(|| format!("插件未声明动作：{action_id}"))?;

    let (_guard, abort, notify) = acquire_run(job_id)?;
    let trigger_label = format!("按钮 · {}", action_def.label);
    let run_id = store::mark_run_started(job_id, "action", &trigger_label)?;

    let secrets = store::decrypt_job_secrets(&record).unwrap_or_default();
    let initial_storage = store::decrypt_job_storage(&record).unwrap_or_default();
    let form_values = merge_form_values(
        &plugin,
        &record.view.values,
        &secrets,
        draft_values.as_ref(),
    );
    let proxy_id = match draft_proxy_id {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed == "direct" { String::new() } else { trimmed.to_string() }
        }
        None => record.view.proxy_id.clone(),
    };

    let outcome = sandbox::execute(SandboxInput {
        plugin,
        job_id: record.view.id.clone(),
        job_name: record.view.name.clone(),
        trigger: "action".to_string(),
        action_id: Some(action_id.to_string()),
        form_values,
        initial_storage,
        proxy_id,
        timeout_seconds: record.view.timeout_seconds,
        preview_mode: false,
        abort,
        abort_notify: notify,
    })
    .await;

    let _ = store::mark_run_finished(
        job_id,
        run_id,
        "action",
        outcome.success,
        outcome.duration_ms,
        &outcome.output,
        outcome.error.as_deref(),
        outcome.status_patch.as_ref(),
        outcome.storage_dirty.then_some(&outcome.new_storage),
    );

    Ok(outcome)
}

pub async fn preview_plugin(
    plugin: TaskPlugin,
    action_id: Option<String>,
    form_values: BTreeMap<String, Value>,
    proxy_id: String,
    storage: BTreeMap<String, String>,
) -> Result<SandboxOutcome, String> {
    let plugin = super::types::validate_and_normalize_plugin(plugin)?;
    let timeout = plugin.default_timeout.clamp(1, 60);
    let proxy_id = if proxy_id.trim() == "direct" {
        String::new()
    } else {
        proxy_id.trim().to_string()
    };
    let outcome = sandbox::execute(SandboxInput {
        plugin,
        job_id: "__preview__".to_string(),
        job_name: "编辑器试跑".to_string(),
        trigger: if action_id.is_some() { "action" } else { "manual" }.to_string(),
        action_id,
        form_values,
        initial_storage: storage,
        proxy_id,
        timeout_seconds: timeout,
        preview_mode: true,
        abort: Arc::new(AtomicBool::new(false)),
        abort_notify: Arc::new(Notify::new()),
    })
    .await;
    Ok(outcome)
}

pub fn spawn_scheduler() {
    store::recover_on_startup();
    crate::spawn_task(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let now = logging::now_ms();
            let due = match store::list_due_jobs(now) {
                Ok(list) => list,
                Err(err) => {
                    logging::verbose("[TaskPlugins]", &format!("调度读取失败：{err}"));
                    continue;
                }
            };
            for job in due {
                let next = next_run_ms(&job.cron_expression, &job.timezone, now).ok();
                let _ = store::update_next_run(&job.id, next);
                if is_job_running(&job.id) {
                    continue;
                }
                let job_id = job.id.clone();
                crate::spawn_task(async move {
                    if let Err(err) = execute_job(&job_id, "schedule").await {
                        logging::verbose("[TaskPlugins]", &format!("任务 {job_id} 调度失败：{err}"));
                    }
                });
            }
        }
    });
}
