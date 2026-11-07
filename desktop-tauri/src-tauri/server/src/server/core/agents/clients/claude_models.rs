use serde_json::{json, Value};

use super::{native_entry, remove, set};
use super::super::{Model, Profile};

pub fn read(value: &Value) -> Vec<Model> {
    value.pointer("/modelPicker/options").and_then(Value::as_array).into_iter().flatten().filter_map(|entry| {
        let id = entry.get("model").and_then(Value::as_str)?.trim();
        if id.is_empty() { return None; }
        let effort = value.get("modelSettings").and_then(|settings| settings.get(id)).and_then(|settings| settings.get("effortLevel"))
            .and_then(Value::as_str).unwrap_or("");
        Some(Model { id: id.into(), name: entry.get("label").and_then(Value::as_str).unwrap_or(id).into(),
            reasoning_levels: if effort.is_empty() { Vec::new() } else { vec![effort.into()] }, effort: effort.into(), ..Default::default() })
    }).collect()
}

pub fn write(value: &mut Value, profile: &Profile) -> Result<(), String> {
    if profile.models.is_empty() {
        for model in read(&profile.native.settings) { remove(value, &["modelSettings", &model.id, "effortLevel"]); }
        if profile.native.settings.get("modelPicker").is_some() { remove(value, &["modelPicker", "options"]); }
        return Ok(());
    }
    let current = value.pointer("/modelPicker/options").and_then(Value::as_array).cloned().unwrap_or_default();
    for row in &current {
        if let Some(id) = row.get("model").and_then(Value::as_str) {
            if !profile.models.iter().any(|model| model.id == id) { remove(value, &["modelSettings", id, "effortLevel"]); }
        }
    }
    let mut rows = Vec::new();
    for model in &profile.models {
        let mut row = native_entry(current.iter().find(|entry| entry.get("model").and_then(Value::as_str) == Some(&model.id)), &Value::Null)?;
        set(&mut row, &["model"], json!(model.id))?;
        set(&mut row, &["label"], json!(if model.name.is_empty() { &model.id } else { &model.name }))?;
        rows.push(row);
        if model.effort.is_empty() || model.effort == "default" {
            remove(value, &["modelSettings", &model.id, "effortLevel"]);
        } else {
            set(value, &["modelSettings", &model.id, "effortLevel"], json!(model.effort))?;
        }
    }
    set(value, &["modelPicker", "options"], json!(rows))?;
    if profile.kind == "gateway" { set(value, &["modelPicker", "replaceBuiltInOptions"], json!(true))?; }
    Ok(())
}
