use serde_json::{json, Value};
use super::{remove, set};
use super::super::Model;

pub fn modern(entry: &Value) -> bool {
    matches!(entry.get("type").and_then(Value::as_str), Some("completions" | "responses" | "messages"))
        || entry.get("models").and_then(Value::as_array).is_some_and(|models| models.iter().any(|model| model.get("capabilities").is_some() || model.get("pricing").is_some()))
}

pub fn read(entry: &Value) -> Model {
    let reasoning = entry.pointer("/reasoning/thinking").and_then(Value::as_str);
    let levels = entry.pointer("/reasoning/effort_levels").and_then(Value::as_array)
        .map(|values| values.iter().filter_map(|value| value.get("value").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_else(|| entry.get("reasoning_levels").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect());
    Model {
        id: super::string(entry, "id"), name: super::string(entry, "name"),
        context_window: entry.get("context_window").and_then(Value::as_u64),
        max_output_tokens: entry.get("default_max_tokens").and_then(Value::as_u64),
        supports_images: entry.pointer("/capabilities/vision").or_else(|| entry.get("supports_attachments")).or_else(|| entry.get("supports_images")).and_then(Value::as_bool),
        supports_reasoning: reasoning.map(|value| value != "never").or_else(|| entry.get("can_reason").and_then(Value::as_bool)),
        reasoning_levels: levels,
        effort: entry.pointer("/reasoning/default_effort_level").or_else(|| entry.get("default_reasoning_effort")).and_then(Value::as_str).unwrap_or("").into(),
        ..Default::default()
    }
}

pub fn write(entry: &mut Value, model: &Model, modern: bool) -> Result<(), String> {
    if modern {
        if let Some(value) = model.supports_images { set(entry, &["capabilities", "vision"], json!(value))?; }
        if let Some(value) = model.supports_reasoning {
            let thinking = if !value { "never" } else if entry.pointer("/reasoning/thinking").and_then(Value::as_str) == Some("always") { "always" } else { "toggleable" };
            set(entry, &["reasoning", "thinking"], json!(thinking))?;
        }
        if !model.reasoning_levels.is_empty() {
            let levels = model.reasoning_levels.iter().map(|value| json!({ "value": value, "display": value })).collect::<Vec<_>>();
            set(entry, &["reasoning", "effort_levels"], json!(levels))?;
        }
        if model.effort.is_empty() || model.effort == "default" { remove(entry, &["reasoning", "default_effort_level"]); }
        else { set(entry, &["reasoning", "default_effort_level"], json!(model.effort))?; }
    } else {
        if let Some(value) = model.supports_images { set(entry, &["supports_attachments"], json!(value))?; }
        if let Some(value) = model.supports_reasoning { set(entry, &["can_reason"], json!(value))?; }
        if !model.reasoning_levels.is_empty() { set(entry, &["reasoning_levels"], json!(model.reasoning_levels))?; }
        if model.effort.is_empty() || model.effort == "default" { remove(entry, &["default_reasoning_effort"]); }
        else { set(entry, &["default_reasoning_effort"], json!(model.effort))?; }
    }
    if model.supports_reasoning == Some(false) {
        for path in [vec!["reasoning_levels"], vec!["default_reasoning_effort"], vec!["reasoning", "effort_levels"], vec!["reasoning", "default_effort_level"]] { remove(entry, &path); }
    }
    Ok(())
}
