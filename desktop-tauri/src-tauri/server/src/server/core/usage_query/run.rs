use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::{config, jobs, query_usage_for};
use crate::server::core::account_store::{sql, AccountStore};
use crate::server::core::{limiter, usage_records};
use crate::server::logging;

pub(super) fn start(store: &AccountStore, account: &Value, replace: bool) -> Option<Arc<jobs::Job>> {
    let id = account.get("id")?.as_str()?;
    let (job, created) = jobs::begin(store, id, replace)?;
    if !created { return Some(job); }
    let store = store.clone();
    let account = job.account.clone();
    let task = job.clone();
    crate::spawn_task(async move {
        let query = query_usage_for(&store, &account);
        tokio::pin!(query);
        let mut check = tokio::time::interval(Duration::from_millis(300));
        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let deadline = tokio::time::sleep_until(task.deadline);
        tokio::pin!(deadline);
        let row = loop {
            tokio::select! {
                biased;
                _ = task.cancel.cancelled() => return,
                _ = &mut deadline => break json!({ "id": task.id, "name": account["name"],
                    "usage": Value::Null, "error": "余额查询超时（已达到本轮时间上限，可重新查询）", "code": "usage_timeout" }),
                row = &mut query => break row,
                _ = check.tick() => jobs::reconcile_one(&store, &task),
            }
        };
        if row.get("code").and_then(Value::as_str) == Some("usage_timeout") {
            logging::log_event("warn", "account", "余额查询超时", Some(json!({
                "action": "usage.query", "account": task.id, "requestId": task.request_id,
                "outcome": "timeout", "timeoutSeconds": task.timeout_seconds,
            })));
        }
        commit(&store, &task, row);
    });
    Some(job)
}

fn commit(store: &AccountStore, job: &Arc<jobs::Job>, mut row: Value) {
    let guard = store.guard();
    let config_guard = config::lock();
    let Some(mut record) = store.record_by_id(&guard, &job.id) else {
        jobs::invalidate_account(&job.id);
        return;
    };
    let mut disabled = false;
    let written = jobs::finish(job, || {
        if config::of(&record) != job.config { return None; }
        let outcome = usage_records::UsageOutcome {
            usage: row.get("usage").filter(|usage| !usage.is_null()).cloned(),
            error: row.get("error").and_then(Value::as_str).map(str::to_string),
            code: row.get("code").and_then(Value::as_str).map(str::to_string),
        };
        if !usage_records::write_result(&job.id, &outcome, &job.config.reading_key, &job.request_id) {
            row["usage"] = Value::Null;
            row["error"] = json!("余额结果保存失败，请重新查询");
            row["code"] = json!("usage_storage_failed");
        } else if outcome.usage.is_some() && record.enabled() {
            let public = store.public_account(&record);
            let hit = if public.get("limiterGroups").is_some() {
                limiter::evaluate::disable_hit(&public, &limiter::sources::facts(), &limiter::counter::facts(), logging::now_ms())
            } else {
                usage_records::low_balance_disable_threshold(&public).zip(outcome.usage.as_ref()
                    .and_then(|usage| usage_records::extract_remaining(usage).0)).is_some_and(|(threshold, remaining)| remaining < threshold)
            };
            if hit {
                record.set("enabled", json!(false));
                record.set_updated_at(logging::now_ms());
                disabled = store.with_conn(&guard, |conn| sql::update_in_place(conn, &record)).is_ok();
            }
        }
        let saved = usage_records::load(&job.id);
        row["at"] = json!(saved.last_attempt_at);
        row["lastSuccessAt"] = json!(saved.last_success_at);
        row["stale"] = json!(!saved.query_config.is_empty() && saved.query_config != job.config.reading_key);
        if row.get("error").and_then(Value::as_str).is_some() { row["lastUsage"] = json!(saved.usage); }
        Some(row)
    });
    drop(config_guard);
    drop(guard);
    if written && disabled {
        store.invalidate_catpaw_sessions(&job.id, &record.provider());
        logging::log_with_level("[Usage]", &format!("账号「{}」余额限制条件已触发，已自动禁用（需手动重新启用）", record.name()), "warn");
    }
}
