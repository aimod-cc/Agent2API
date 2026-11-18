use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::{config, diagnostics, jobs, query::query_usage_for, recovery};
use crate::server::core::account_store::{sql, AccountStore};
use crate::server::core::{limiter, usage_records};
use crate::server::logging;

pub(super) fn start(store: &AccountStore, account: &Value, replace: bool) -> Option<Arc<jobs::Job>> {
    let id = account.get("id")?.as_str()?;
    let (job, created) = jobs::begin(store, id, replace)?;
    if !created { return Some(job); }
    super::queue::submit(store.clone(), job.clone());
    Some(job)
}

pub(super) async fn execute(store: AccountStore, task: Arc<jobs::Job>) {
    let account = task.account.clone();
    crate::server::core::runtime_capacity::background_scope(async {
        let provider = account.get("provider").and_then(Value::as_str)
            .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
        let reservation = match wait_for_slot(&store, &task, provider).await {
            None => return,
            Some(Err(row)) => { commit(&store, &task, row); return; }
            Some(Ok(reservation)) => reservation,
        };
        let Some(expires) = jobs::start_execution(&task) else { return; };
        let trace = diagnostics::Trace::new(provider, &task.id, task.timeout_seconds);
        let query = diagnostics::scope(Some(trace.clone()),
            crate::server::core::plugin_host::wasm::query_queue::scope(reservation,
                super::scope_query_deadline(Some(expires),
                    super::scope_account_query(provider, &task.id, true, query_usage_for(&store, &account)))));
        tokio::pin!(query);
        let mut check = tokio::time::interval(Duration::from_millis(300));
        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let deadline = tokio::time::sleep_until(expires);
        tokio::pin!(deadline);
        let row = loop {
            tokio::select! {
                biased;
                _ = task.cancel.cancelled() => return,
                _ = &mut deadline => {
                    let mut row = trace.failure_row(&task.id, &account["name"]);
                    trace.attach(&mut row, Some("deadline"));
                    break row;
                },
                row = &mut query => break row,
                _ = check.tick() => jobs::reconcile_one(&store, &task),
            }
        };
        diagnostics::log_result(&task, &row);
        commit(&store, &task, row);
    }).await;
}

async fn wait_for_slot(store: &AccountStore, task: &Arc<jobs::Job>, provider: &str)
    -> Option<Result<Option<Arc<crate::server::core::plugin_host::wasm::query_queue::Reservation>>, Value>> {
    let reserve = crate::server::core::plugin_host::wasm::query_queue::reserve(provider, &task.id);
    tokio::pin!(reserve);
    let mut check = tokio::time::interval(Duration::from_millis(300));
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::sleep_until(tokio::time::Instant::now() + Duration::from_secs(
        crate::server::core::plugin_host::wasm::query_queue::WAIT_SECONDS));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            _ = task.cancel.cancelled() => return None,
            _ = &mut deadline => {
                let row = json!({ "id": task.id, "name": task.account["name"], "usage": Value::Null,
                    "error": "余额查询排队超时（尚未开始查询，可重新查询）", "code": "usage_queue_timeout" });
                logging::log_event("warn", "account", "余额查询排队超时", Some(json!({
                    "provider": provider, "account": task.id, "accountName": task.account["name"],
                    "requestId": task.request_id, "action": "usage.query", "outcome": "timeout",
                    "detail": row["error"], "code": row["code"],
                    "durationMs": logging::now_ms().saturating_sub(task.started_at),
                })));
                return Some(Err(row));
            }
            result = &mut reserve => return Some(result.map_err(|error| json!({
                "id": task.id, "name": task.account["name"], "usage": Value::Null,
                "error": error.message, "code": error.code,
            }))),
            _ = check.tick() => jobs::reconcile_one(store, task),
        }
    }
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
            diagnostics: row.get("diagnostics").filter(|value| value.is_object()).cloned(),
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
        recovery::settled(job, row.get("code").and_then(Value::as_str));
        Some(row)
    });
    drop(config_guard);
    drop(guard);
    if written && disabled {
        store.invalidate_catpaw_sessions(&job.id, &record.provider());
        logging::log_with_level("[Usage]", &format!("账号「{}」余额限制条件已触发，已自动禁用（需手动重新启用）", record.name()), "warn");
    }
}
