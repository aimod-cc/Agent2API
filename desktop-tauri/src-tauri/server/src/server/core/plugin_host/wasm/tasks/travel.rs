use serde_json::{json, Value};

use super::{current, definition, execution, settings, AccountStore};
use crate::server::core::{beijing, growth_scheduler::RunOutcome, task_state};
use crate::server::errors::GatewayError;

const PROVIDER: &str = "workbuddy";
const TASK: &str = "travel";

pub(crate) fn state() -> Option<Value> {
    if !super::super::has_capability(PROVIDER, "tasks.v1") { return None; }
    let raw = settings::legacy_state();
    let declared = definition(PROVIDER, TASK);
    let task = declared.as_ref().map(|task| current(PROVIDER, task));
    let states = task_state::read_all();
    let unavailable = if !super::registry::serves(PROVIDER) || task.is_none() {
        Some("旅行任务尚未就绪，未回退到旧版执行入口")
    } else if !settings::readable() {
        Some("旅行配置暂不可读，未发送旅行请求")
    } else if states.is_err() {
        Some("任务记录暂不可读，未发送旅行请求")
    } else { None };
    let batch = states.as_ref().ok().and_then(|states| states.get(&execution::key(PROVIDER, TASK)));
    let today = beijing::today_key();
    let fired = if raw["lastFiredDate"].as_str() == Some(&today) { raw["firedTimes"].clone() } else { json!([]) };
    let recent = execution::snapshot(batch);
    let result = if recent["active"] != true && recent["at"].as_i64().unwrap_or(0) >= raw["lastResult"]["at"].as_i64().unwrap_or(0) {
        if batch.is_some() { recent } else { raw["lastResult"].clone() }
    } else { raw["lastResult"].clone() };
    Some(json!({
        "enabled": task.as_ref().map(|task| task.enabled).unwrap_or_else(|| raw["enabled"].as_bool().unwrap_or(false)),
        "times": task.map(|task| json!(task.times)).unwrap_or_else(|| raw["times"].as_array().map(|value| json!(value)).unwrap_or_else(|| json!([]))),
        "available": unavailable.is_none(), "pausedReason": unavailable, "engine": "wasm_tasks",
        "firedToday": fired.as_array().map(|values| json!(values)).unwrap_or_else(|| json!([])),
        "lastFiredDate": raw["lastFiredDate"], "lastFiredTime": raw["lastFiredTime"],
        "lastResult": result, "running": batch.is_some_and(|state| state.running())
            || states.as_ref().ok().is_some_and(|states| states.iter().any(|(key, state)| key.starts_with("pluginTask:workbuddy:travel:") && state.running())),
    }))
}

pub(crate) fn configure(patch: &Value) -> Result<Value, String> {
    super::configure(PROVIDER, TASK, patch)?;
    state().ok_or_else(|| "旅行插件已变化，请重新读取状态".to_string())
}

pub(crate) async fn run(store: &AccountStore) -> RunOutcome {
    match super::run(store, PROVIDER, TASK, true).await {
        Ok(value) if value["skipped"] == true => RunOutcome::Skipped("本轮没有发送任务，请查看最近执行结果".into()),
        Ok(value) => RunOutcome::Completed(value),
        Err(message) if message == "任务正在执行中，请稍候" => RunOutcome::Busy,
        Err(message) => RunOutcome::Failed(GatewayError::bad_request(message)),
    }
}
