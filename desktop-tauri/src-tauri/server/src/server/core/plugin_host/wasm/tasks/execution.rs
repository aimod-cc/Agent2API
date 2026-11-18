use std::time::Duration;

use serde_json::{json, Value};

use super::{current, definition, interval, registry, settings, AccountStore, Definition};
use crate::server::core::plugin_host::rpc::RpcAdapter;
use crate::server::core::task_state::{self, Claim, ManualBackoff};
use crate::server::logging;

pub(super) fn key(provider: &str, id: &str) -> String { format!("pluginTaskBatch:{provider}:{id}") }

pub(super) fn snapshot(state: Option<&task_state::TaskState>) -> Value {
    let Some(state) = state else { return json!({ "active": false }) };
    let active = state.running();
    let mut value = state.value.as_ref().filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({}));
    if !active && (value["active"] == true || state.last_attempt_at > state.last_run_at) {
        return json!({ "active": false, "at": state.last_attempt_at, "success": null,
            "error": "上次执行中断，结果未确认；不会自动重发任务", "interrupted": true });
    }
    value["active"] = json!(active);
    if !active && value["error"].is_null() && value["skipped"] != true {
        if let Some(error) = &state.last_error { value["error"] = json!(error); }
    }
    value
}

struct Prepared {
    provider: String,
    task: Definition,
    adapter: RpcAdapter,
    guard: task_state::RunGuard,
    accounts: Vec<Value>,
    planned_at: i64,
    manual: bool,
}

async fn prepare(store: &AccountStore, provider: &str, id: &str, manual: bool) -> Result<Option<Prepared>, String> {
    if !super::super::has_capability(provider, "tasks.v1") || !registry::serves(provider) {
        return Err("插件任务当前不可用".into());
    }
    let Some(adapter) = registry::acquire(provider).await.map_err(|error| error.message)? else {
        return Err("提供商插件尚未就绪".into());
    };
    if adapter.host().options.wasm.as_ref().is_none_or(|options| !options.allows("tasks.v1")) {
        return Err("当前插件未声明任务能力".into());
    }
    if !settings::readable() { return Err("任务配置暂不可读，请刷新状态；不会发送任务".into()); }
    let task = current(provider, &definition(provider, id).ok_or("任务不存在")?);
    let (due, _, planned_at) = interval(&task, logging::now_ms());
    if !manual && (!task.enabled || !due || planned_at > 0 && settings::legacy_fired(&task, planned_at)) {
        return Ok(None);
    }
    let accounts = store.maintenance_accounts_for_provider(provider);
    if accounts.is_empty() {
        return if manual { Err("没有可执行账号".into()) } else { Ok(None) };
    }
    if !manual {
        let now = logging::now_ms();
        let mut pending = false;
        for account in &accounts {
            let Some(account) = account["id"].as_str() else { continue };
            let state = task_state::read(&format!("pluginTask:{provider}:{id}:{account}"))?;
            if (planned_at <= 0 || state.last_attempt_at < planned_at) && state.due_at() <= now { pending = true; break; }
        }
        if !pending { return Ok(None); }
    }
    let guard = match task_state::claim(&key(provider, id), 0, true, ManualBackoff::Respect, 1000)? {
        Claim::Acquired(guard) => guard,
        Claim::Deferred(state) if manual => return Err(state.waiting_message()),
        Claim::Deferred(_) => return Ok(None),
    };
    Ok(Some(Prepared { provider: provider.into(), task, adapter, guard, accounts, planned_at, manual }))
}

pub(super) async fn run(store: &AccountStore, provider: &str, id: &str, manual: bool) -> Result<Value, String> {
    let Some(prepared) = prepare(store, provider, id, manual).await? else {
        return Ok(json!({ "skipped": true, "results": [] }));
    };
    finish(store, prepared).await
}

pub(super) async fn launch(store: AccountStore, provider: &str, id: &str) -> Result<Value, String> {
    let prepared = prepare(&store, provider, id, true).await?.ok_or("任务当前不可用")?;
    let started = json!({ "active": true, "at": logging::now_ms(), "manual": true, "results": [] });
    task_state::store_value(&key(provider, id), started.clone())?;
    crate::spawn_task(async move {
        if let Err(error) = finish(&store, prepared).await {
            logging::log_with_level("[PluginTask]", &error, "warn");
        }
    });
    Ok(json!({ "accepted": true, "provider": provider, "task": id, "operation": started }))
}

async fn finish(store: &AccountStore, prepared: Prepared) -> Result<Value, String> {
    let result = execute(store, &prepared).await;
    let mut summary = match &result {
        Ok(value) => value.clone(),
        Err(error) => json!({ "active": false, "at": logging::now_ms(), "error": error, "success": false }),
    };
    summary["manual"] = json!(prepared.manual);
    summary["skipped"] = json!(result.is_ok() && summary["accounts"].as_u64() == Some(0));
    let success = summary["success"].as_bool() == Some(true);
    let message = summary["error"].as_str().map(str::to_string)
        .unwrap_or_else(|| if summary["accounts"].as_u64() == Some(0) {
            "本轮没有发送任务，原有排程保持不变".to_string()
        } else { "本轮任务已结束，逐账号结果已记录".to_string() });
    if !prepared.manual && summary["accounts"].as_u64().is_some_and(|count| count > 0) {
        settings::remember_legacy(&prepared.task, (prepared.planned_at > 0).then_some(prepared.planned_at), &summary);
    } else if prepared.manual {
        settings::remember_legacy(&prepared.task, None, &summary);
    }
    prepared.guard.finish(success, message, Some(summary.clone()), 0, 60_000)?;
    result.map(|_| summary)
}

async fn execute(store: &AccountStore, prepared: &Prepared) -> Result<Value, String> {
    let started_at = logging::now_ms();
    let provider = &prepared.provider;
    let id = &prepared.task.id;
    let mut results = Vec::new();
    let mut skipped = 0;
    for account in &prepared.accounts {
        let Some(account_id) = account["id"].as_str() else { continue };
        if store.public_account_by_id(account_id).is_none_or(|account| account["provider"].as_str() != Some(provider.as_str())) {
            skipped += 1;
            continue;
        }
        if !settings::readable() { return Err("任务配置暂不可读，请刷新状态；不会继续发送任务".into()); }
        let task = current(provider, &prepared.task);
        if !prepared.manual && (!task.enabled || task.times != prepared.task.times || task.interval_ms != prepared.task.interval_ms) { break; }
        let now = logging::now_ms();
        let (due, next, current_plan) = interval(&task, now);
        if !prepared.manual && current_plan != prepared.planned_at { break; }
        let planned_at = prepared.planned_at;
        let account_key = format!("pluginTask:{provider}:{id}:{account_id}");
        let previous = task_state::read(&account_key)?;
        // 已尝试的每日时段不会因失败、重启或切换界面再发送一次。
        if !prepared.manual && (!due || planned_at > 0 && (previous.last_attempt_at >= planned_at || settings::legacy_fired(&task, planned_at))) {
            skipped += 1;
            continue;
        }
        let guard = match task_state::claim(&account_key, next, prepared.manual, ManualBackoff::Respect, 1000)? {
            Claim::Acquired(guard) => guard,
            Claim::Deferred(_) => { skipped += 1; continue; }
        };
        if !prepared.manual && current(provider, &prepared.task) != task { drop(guard); break; }
        let result = prepared.adapter.host().call_coded("tasks.run", json!({ "provider": provider, "account": account_id,
            "task": id, "manual": prepared.manual, "scheduled_at": planned_at }), Duration::from_secs(600)).await;
        let (success, message, credit) = match result {
            Ok(value) if value["success"].is_boolean() => (value["success"] == true,
                value["message"].as_str().unwrap_or("任务已结束").chars().take(1000).collect::<String>(),
                value["report"]["credit"].as_f64().or_else(|| value["credit"].as_f64())),
            Ok(_) => (false, "插件未返回有效的任务结果".into(), None),
            Err(error) => (false, super::super::super::process::redact(&error.message), None),
        };
        let name = account["displayName"].as_str().filter(|name| !name.is_empty())
            .or_else(|| account["name"].as_str()).unwrap_or(account_id);
        let row = json!({ "account": account_id, "accountId": account_id, "name": name,
            "success": success, "message": message, "credit": credit, "at": logging::now_ms() });
        guard.finish(success, message, Some(row.clone()), 0, next)?;
        results.push(row);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let failed = results.iter().filter(|row| row["success"] == false).count();
    let credit = (!results.is_empty()).then(|| results.iter().try_fold(0.0_f64, |sum, row| Some(sum + row["credit"].as_f64()?))).flatten();
    let error = results.iter().find(|row| row["success"] == false).map(|row| row["message"].clone());
    Ok(json!({ "active": false, "startedAt": started_at, "at": logging::now_ms(),
        "accounts": results.len(), "failed": failed, "skipped": skipped, "credit": credit, "error": error,
        "success": (!results.is_empty()).then_some(failed == 0), "actions": results, "results": results }))
}
