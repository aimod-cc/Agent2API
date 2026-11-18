use serde_json::{json, Map, Value};

use super::Definition;
use crate::server::{config, logging};

const KEY: &str = "pluginTaskSettings";
pub(super) const LEGACY_TRAVEL: &str = "growthSchedule";

pub(super) fn readable() -> bool { config::with_raw(|_| ()).is_some() }

pub(super) fn current(provider: &str, task: &Definition) -> Definition {
    config::with_raw(|raw| from_raw(raw, provider, task)).unwrap_or_else(|| {
        let mut unavailable = task.clone();
        unavailable.enabled = false;
        unavailable
    })
}

fn from_raw(raw: &Map<String, Value>, provider: &str, task: &Definition) -> Definition {
    let mut current = task.clone();
    if let Some(key) = &task.legacy_config { apply(&mut current, raw.get(key)); }
    let override_value = raw.get(KEY).and_then(|all| all.get(provider)).and_then(|all| all.get(&task.id));
    apply(&mut current, override_value);
    // 两个已发布入口的开关冲突时优先停用；再次保存会原子同步两处。
    if let Some(key) = &task.legacy_config {
        if raw.get(key).and_then(|value| value.get("enabled")) == Some(&Value::Bool(false)) {
            current.enabled = false;
        }
    }
    current
}

fn apply(task: &mut Definition, value: Option<&Value>) {
    let Some(value) = value else { return };
    if let Some(enabled) = value["enabled"].as_bool() { task.enabled = enabled; }
    if let Some(items) = value["times"].as_array() {
        let mut times: Vec<_> = items.iter().filter_map(Value::as_str)
            .filter_map(super::super::super::schedules::normalize_time).collect();
        times.sort(); times.dedup();
        if !times.is_empty() { task.times = times; task.interval_ms = None; }
    }
}

pub(super) fn save(provider: &str, task: &Definition, patch: &Value) -> Result<(), String> {
    let fields = patch.as_object().filter(|fields| !fields.is_empty()).ok_or("任务设置无效")?;
    if fields.keys().any(|key| !matches!(key.as_str(), "enabled" | "times"))
        || fields.get("enabled").is_some_and(|value| !value.is_boolean()) {
        return Err("任务设置无效".into());
    }
    let mut patch = patch.clone();
    if let Some(value) = fields.get("times") {
        let max = if task.legacy_config.is_some() { super::super::super::schedules::MAX_TIMES } else { 24 };
        let items = value.as_array().filter(|items| !items.is_empty() && items.len() <= max).ok_or("任务时刻无效")?;
        if task.interval_ms.is_some() { return Err("间隔任务不接受每日时刻".into()); }
        let mut times: Vec<_> = items.iter().map(|value| value.as_str()
            .and_then(super::super::super::schedules::normalize_time).ok_or("任务时刻无效"))
            .collect::<Result<_, _>>()?;
        times.sort(); times.dedup();
        patch["times"] = json!(times);
    }
    config::try_update_raw_fields(|raw| {
        let patch = if task.legacy_config.is_some() {
            let mut effective = from_raw(raw, provider, task);
            apply(&mut effective, Some(&patch));
            json!({ "enabled": effective.enabled, "times": effective.times })
        } else { patch };
        let all = raw.entry(KEY).or_insert_with(|| json!({}));
        if !all.is_object() { return Err("任务配置格式无效，未覆盖已有配置".to_string()); }
        let root = all.as_object_mut().ok_or("任务配置格式无效")?;
        let settings = root.entry(provider).or_insert_with(|| json!({}));
        let settings = settings.as_object_mut().ok_or("提供商任务配置格式无效")?;
        let settings = settings.entry(&task.id).or_insert_with(|| json!({}));
        merge(settings, &patch)?;
        if let Some(key) = &task.legacy_config {
            let legacy = raw.entry(key).or_insert_with(|| json!({}));
            merge(legacy, &patch)?;
        }
        Ok(())
    }, || "任务设置未能保存，未改变原有排程".to_string())?;
    super::super::super::schedules::wake();
    Ok(())
}

fn merge(value: &mut Value, patch: &Value) -> Result<(), String> {
    let fields = value.as_object_mut().ok_or("已有任务配置格式无效，未覆盖")?;
    for (key, value) in patch.as_object().ok_or("任务设置无效")? { fields.insert(key.clone(), value.clone()); }
    Ok(())
}

pub(super) fn legacy_fired(task: &Definition, planned_at: i64) -> bool {
    let Some(key) = &task.legacy_config else { return false };
    let local = chrono::FixedOffset::east_opt(task.utc_offset_minutes * 60).and_then(|offset| {
        chrono::DateTime::from_timestamp_millis(planned_at).map(|at| at.with_timezone(&offset))
    });
    let Some(local) = local else { return true };
    let date = local.format("%Y-%m-%d").to_string();
    let time = local.format("%H:%M").to_string();
    config::with_raw(|raw| raw.get(key).is_some_and(|value| value["lastFiredDate"] == date
        && value["firedTimes"].as_array().is_some_and(|times| times.iter().any(|value| value == &time))))
        .unwrap_or(true)
}

pub(super) fn legacy_state() -> Value {
    config::with_raw(|raw| raw.get(LEGACY_TRAVEL).cloned()).flatten().unwrap_or(Value::Null)
}

pub(super) fn remember_legacy(task: &Definition, planned_at: Option<i64>, result: &Value) {
    let Some(key) = &task.legacy_config else { return };
    let now = crate::server::core::beijing::now();
    let planned = planned_at.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| at.with_timezone(now.offset()));
    let date = planned.as_ref().unwrap_or(&now).format("%Y-%m-%d").to_string();
    let planned_time = planned.map(|at| at.format("%H:%M").to_string());
    let saved = config::try_update_raw_field(key, |value| {
        if value.is_null() { *value = json!({}); }
        let fields = value.as_object_mut().ok_or("旅行记录格式无效")?;
        let mut fired = if fields.get("lastFiredDate") == Some(&json!(date)) {
            fields.get("firedTimes").and_then(Value::as_array).cloned().unwrap_or_default()
        } else { Vec::new() };
        if let Some(time) = planned_time {
            if !fired.iter().any(|value| value == &time) { fired.push(json!(time)); }
        }
        fields.insert("lastFiredDate".into(), json!(date));
        fields.insert("lastFiredTime".into(), json!(now.format("%H:%M").to_string()));
        fields.insert("firedTimes".into(), json!(fired));
        fields.insert("lastResult".into(), result.clone());
        Ok(())
    }, || "旅行记录保存失败");
    if saved.is_err() { logging::log_with_level("[PluginTask]", "旅行结果已记录，但旧版摘要同步失败；不会重发任务", "warn"); }
}
