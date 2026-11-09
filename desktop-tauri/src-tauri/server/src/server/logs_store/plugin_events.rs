use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::LogEntry;

pub(super) fn prepare(message: &str, data: &mut Map<String, Value>) {
    if data.get("action").and_then(Value::as_str) == Some("plugin.startup") {
        return;
    }
    if let Some((id, loaded)) = message.strip_prefix("插件包「").and_then(|text| text.split_once("」v")) {
        if let Some(count) = loaded.split_once(" 已加载，").and_then(|(_, text)| text.strip_suffix(" 家提供商可用"))
            .and_then(|text| text.parse::<usize>().ok()) {
            data.insert("action".into(), json!("plugin.startup"));
            data.insert("pluginId".into(), json!(id));
            data.insert("phase".into(), json!("ready"));
            data.insert("providerCount".into(), json!(count));
            data.insert("summary".into(), json!("插件已就绪"));
            data.insert("routine".into(), json!(true));
            return;
        }
    }
    let Some((id, result)) = message.split_once('：') else { return };
    let id = id.trim();
    if id.is_empty() || id.chars().any(char::is_whitespace) { return; }
    let (phase, rest) = if let Some(rest) = result.strip_prefix("加载完成（") {
        ("ready", rest)
    } else if let Some(rest) = result.strip_prefix("加载未完成（") {
        ("unknown", rest)
    } else {
        return;
    };
    let Some((duration, timing)) = rest.split_once(" 毫秒，阶段耗时：") else { return };
    let Ok(duration) = duration.parse::<i64>() else { return };
    let summary = if phase == "ready" { "插件已就绪" } else { "插件未能就绪" };
    data.insert("action".into(), json!("plugin.startup"));
    data.insert("pluginId".into(), json!(id));
    data.insert("phase".into(), json!(phase));
    data.insert("durationMs".into(), json!(duration.max(0)));
    data.insert("summary".into(), json!(summary));
    data.insert("routine".into(), json!(phase == "ready"));
    if let Some((steps, detail)) = timing.split_once('）') {
        if let Ok(Value::Array(steps)) = serde_json::from_str::<Value>(steps) {
            data.insert("steps".into(), Value::Array(steps));
        }
        let detail = detail.trim_start_matches('：').trim();
        if !detail.is_empty() { data.insert("detail".into(), json!(detail)); }
    }
}

pub(super) fn enrich(entry: &mut LogEntry, names: &BTreeMap<String, String>) {
    let Some(data) = entry.data.as_mut().and_then(Value::as_object_mut) else { return };
    enrich_name(data, names);
    if let Some(plugins) = data.get_mut("plugins").and_then(Value::as_array_mut) {
        for plugin in plugins.iter_mut().filter_map(Value::as_object_mut) { enrich_name(plugin, names); }
    }
}

fn enrich_name(data: &mut Map<String, Value>, names: &BTreeMap<String, String>) {
    let Some(name) = data.get("pluginId").and_then(Value::as_str).and_then(|id| names.get(id)) else { return };
    if data.get("pluginName").and_then(Value::as_str) == Some(name) { return; }
    if let Some(recorded) = data.get("pluginName").cloned() {
        data.entry("recordedPluginName").or_insert(recorded);
    }
    data.insert("pluginName".into(), json!(name));
}

pub(super) fn group_key(entry: &LogEntry) -> Option<String> {
    let data = entry.data.as_ref()?;
    if data.get("action").and_then(Value::as_str) != Some("plugin.startup")
        || !matches!(entry.level.as_str(), "warn" | "error") {
        return None;
    }
    // 只合并同一插件、同一结果和同一原因，不把共享环境中的不同插件混成一条。
    let reason = data.get("detail").and_then(Value::as_str).filter(|detail| !detail.is_empty())
        .unwrap_or(&entry.message);
    serde_json::to_string(&(
        &entry.level, &entry.category, data.get("pluginId")?.as_str()?,
        data.get("phase"), reason, data.get("bootId"),
    )).ok()
}
