use serde_json::Value;
use super::{ClientDef, Model, Profile};

pub fn levels(client: &str, declared: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for level in declared {
        let level = if client == "dsh" {
            match level.as_str() { "none" | "disabled" => "off", "mid" => "medium", _ => level }
        } else if client == "codex" && matches!(level.as_str(), "off" | "disabled") { "none" } else { level };
        let supported = match client {
            "claude-code" => matches!(level, "low" | "medium" | "high" | "xhigh"),
            "codex" => matches!(level, "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"),
            "zed" => matches!(level, "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"),
            "dsh" => matches!(level, "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"),
            _ => !level.trim().is_empty(),
        };
        if supported && !result.iter().any(|item| item == level) { result.push(level.to_string()); }
    }
    result
}

pub fn normalize(def: &ClientDef, profile: &mut Profile) {
    if matches!(def.id, "codex" | "claude-code") && profile.models.is_empty() && !profile.model.is_empty()
        && !(def.id == "codex" && profile.base_url.is_empty() && profile.auth_mode == "preserve") {
        profile.models.push(Model { id: profile.model.clone(), name: profile.model.clone(), ..Default::default() });
    }
    for model in &mut profile.models {
        model.reasoning_levels = levels(def.id, &model.reasoning_levels);
        if def.id == "opencode" && profile.protocol == "anthropic" {
            model.reasoning_levels = model.reasoning_levels.iter().map(|level| {
                if matches!(level.as_str(), "off" | "disabled") { "none".to_string() } else { level.clone() }
            }).filter(|level| matches!(level.as_str(), "none" | "low" | "medium" | "high" | "xhigh" | "max")).collect();
            if matches!(model.effort.as_str(), "off" | "disabled") { model.effort = "none".into(); }
        }
        if def.id == "dsh" && model.supports_reasoning == Some(false) { model.reasoning_levels.clear(); }
        if model.supports_reasoning == Some(false) { model.effort.clear(); }
    }
}

pub fn declared_fields(def: &ClientDef, path: &std::path::Path) -> Value {
    let mut fields = vec!["contextWindow", "maxOutputTokens", "supportsImages"];
    match def.id {
        "claude-code" => { fields.clear(); }
        "codex" => { fields.retain(|field| *field != "maxOutputTokens"); fields.push("supportsTools"); }
        "zcode" | "opencode" => { fields.extend(["supportsTools", "supportsVideo"]); if def.id == "opencode" { fields.push("supportsReasoning"); } }
        "kimi-code" => { fields.extend(["supportsReasoning", "supportsVideo"]); if super::paths::kimi_legacy(path) { fields.retain(|field| *field != "maxOutputTokens"); } else { fields.push("supportsTools"); } }
        "workbuddy" | "cline" => fields.extend(["supportsTools", "supportsReasoning"]),
        "zed" => fields.push("supportsTools"),
        "crush" | "dsh" => fields.push("supportsReasoning"),
        _ => fields.clear(),
    }
    serde_json::json!(fields)
}
