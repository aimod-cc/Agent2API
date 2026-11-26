//! 定时任务实例管理、立即执行/停止、自定义按钮动作与运行记录接口：`/api/task-jobs*`。

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::core::proxy_pool;
use crate::server::core::task_plugins::{cron_schedule, scheduler, store};
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};

#[derive(Deserialize)]
pub struct RunsQuery {
    #[serde(default = "default_limit")]
    limit: i64,
}
fn default_limit() -> i64 {
    30
}

pub async fn list() -> Response {
    match store::list_jobs() {
        Ok(mut jobs) => {
            for job in &mut jobs {
                if scheduler::is_job_running(&job.id) {
                    job.last_status = Some("running".to_string());
                }
            }
            let proxies: Vec<Value> = proxy_pool::list()
                .into_iter()
                .filter(|item| {
                    item.get("enabled").and_then(Value::as_bool).unwrap_or(true)
                        && item.get("resolveError").is_none_or(Value::is_null)
                })
                .map(|item| {
                    let resolved = item.get("resolved").filter(|value| value.is_object());
                    json!({
                        "id": item["id"],
                        "name": item["name"],
                        "protocol": resolved.and_then(|value| value.get("protocol")).unwrap_or(&item["protocol"]),
                        "host": resolved.and_then(|value| value.get("host")).unwrap_or(&item["host"]),
                        "port": resolved.and_then(|value| value.get("port")).unwrap_or(&item["port"]),
                    })
                })
                .collect();
            ok_json(json!({
                "jobs": jobs,
                "proxies": proxies,
            }))
        }
        Err(error) => management_error(500, error),
    }
}

pub async fn create(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let input = match parse_job_input(None, &payload) {
        Ok(i) => i,
        Err(error) => return management_error(400, error),
    };
    match store::save_job(input) {
        Ok(job) => ok_json(json!({ "job": job })),
        Err(error) => management_error(400, error),
    }
}

pub async fn update(Path(id): Path<String>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let has_plugin_id = payload
        .get("pluginId")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|v| !v.is_empty());
    if !has_plugin_id {
        let enabled = payload.get("enabled").and_then(Value::as_bool);
        let proxy_id = payload
            .get("proxyId")
            .and_then(Value::as_str)
            .map(str::to_string);
        if enabled.is_some() || proxy_id.is_some() {
            return match store::patch_job_inline(&id, enabled, proxy_id) {
                Ok(job) => ok_json(json!({ "job": job })),
                Err(error) => management_error(400, error),
            };
        }
    }
    let input = match parse_job_input(Some(id), &payload) {
        Ok(i) => i,
        Err(error) => return management_error(400, error),
    };
    match store::save_job(input) {
        Ok(job) => ok_json(json!({ "job": job })),
        Err(error) => management_error(400, error),
    }
}

pub async fn patch(Path(id): Path<String>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let enabled = payload.get("enabled").and_then(Value::as_bool);
    let proxy_id = payload
        .get("proxyId")
        .and_then(Value::as_str)
        .map(str::to_string);
    if enabled.is_none() && proxy_id.is_none() {
        return management_error(400, "没有需要更新的字段");
    }
    match store::patch_job_inline(&id, enabled, proxy_id) {
        Ok(job) => ok_json(json!({ "job": job })),
        Err(error) => management_error(400, error),
    }
}

pub async fn remove(Path(id): Path<String>) -> Response {
    let _ = scheduler::stop_job(&id);
    match store::delete_job(&id) {
        Ok(()) => ok_json(json!({ "deleted": id })),
        Err(error) => management_error(400, error),
    }
}

pub async fn duplicate(Path(id): Path<String>) -> Response {
    match store::duplicate_job(&id) {
        Ok(job) => ok_json(json!({ "job": job })),
        Err(error) => management_error(400, error),
    }
}

pub async fn run_now(Path(id): Path<String>) -> Response {
    match scheduler::execute_job(&id, "manual").await {
        Ok(outcome) => {
            let job = store::get_job_record(&id).ok().flatten().map(|r| r.view);
            ok_json(json!({
                "outcome": outcome,
                "job": job,
            }))
        }
        Err(error) => management_error(400, error),
    }
}

pub async fn stop_now(Path(id): Path<String>) -> Response {
    match scheduler::stop_job(&id) {
        Ok(()) => ok_json(json!({ "stopped": true })),
        Err(error) => management_error(400, error),
    }
}

pub async fn trigger_action(Path(id): Path<String>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let action_id = payload["actionId"].as_str().unwrap_or("").trim();
    if action_id.is_empty() {
        return management_error(400, "缺少按钮动作标识");
    }
    let draft_values: Option<BTreeMap<String, Value>> = payload
        .get("values")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let draft_proxy_id = payload
        .get("proxyId")
        .and_then(Value::as_str)
        .map(str::to_string);

    match scheduler::execute_action(&id, action_id, draft_values, draft_proxy_id).await {
        Ok(outcome) => {
            let job = store::get_job_record(&id).ok().flatten().map(|r| r.view);
            ok_json(json!({
                "outcome": outcome,
                "job": job,
            }))
        }
        Err(error) => management_error(400, error),
    }
}

pub async fn list_runs(Path(id): Path<String>, Query(query): Query<RunsQuery>) -> Response {
    match store::list_runs(&id, query.limit) {
        Ok(runs) => ok_json(json!({ "runs": runs })),
        Err(error) => management_error(400, error),
    }
}

pub async fn clear_runs(Path(id): Path<String>) -> Response {
    match store::clear_runs(&id) {
        Ok(()) => ok_json(json!({ "cleared": true })),
        Err(error) => management_error(400, error),
    }
}

pub async fn validate_cron(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let expr = payload["cronExpression"].as_str().unwrap_or("");
    let tz = payload["timezone"].as_str().unwrap_or("");
    match cron_schedule::preview_next_runs(expr, tz, 3) {
        Ok(next_runs) => ok_json(json!({ "nextRuns": next_runs })),
        Err(error) => management_error(400, error),
    }
}

pub async fn cron_preview(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(v) => v,
        Err(error) => return management_error(400, error.message),
    };
    let expr = payload["cronExpression"].as_str().unwrap_or("");
    let tz = payload["timezone"].as_str().unwrap_or("");
    match cron_schedule::preview_next_runs(expr, tz, 3) {
        Ok(next_runs) => ok_json(json!({
            "valid": true,
            "nextRunAt": next_runs.first().copied(),
            "nextRuns": next_runs,
        })),
        Err(error) => ok_json(json!({
            "valid": false,
            "error": error,
        })),
    }
}

fn parse_job_input(id: Option<String>, payload: &Value) -> Result<store::JobUpsertInput, String> {
    let plugin_id = payload["pluginId"].as_str().unwrap_or("").trim().to_string();
    if plugin_id.is_empty() {
        return Err("请选择定时任务插件".to_string());
    }
    let name = payload["name"].as_str().unwrap_or("").to_string();
    let cron_expression = payload["cronExpression"].as_str().unwrap_or("").to_string();
    let timezone = payload["timezone"]
        .as_str()
        .unwrap_or("Asia/Shanghai")
        .to_string();
    let timeout_seconds = payload["timeoutSeconds"].as_i64().unwrap_or(30);
    let proxy_id = payload["proxyId"].as_str().unwrap_or("").to_string();
    let enabled = payload
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let values: BTreeMap<String, Value> = payload
        .get("values")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let clear_secrets: Vec<String> = payload
        .get("clearSecrets")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Ok(store::JobUpsertInput {
        id,
        plugin_id,
        name,
        cron_expression,
        timezone,
        timeout_seconds,
        proxy_id,
        enabled,
        values,
        clear_secrets,
    })
}
