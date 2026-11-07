use std::collections::HashSet;
use serde_json::Value;
use super::{ClientDef, Profile};

pub fn profiles(def: &ClientDef, live: &[Profile], gateway: &str) -> Vec<Profile> {
    if def.id == "workbuddy" {
        return live.iter().filter(|profile| super::is_gateway(&profile.base_url, gateway)).filter_map(|profile| {
            let owned: HashSet<_> = profile.native.entry.as_array()?.iter()
                .filter(|entry| entry.get("vendor").and_then(Value::as_str) == Some("Agent2API"))
                .filter_map(|entry| entry.get("id").and_then(Value::as_str)).collect();
            let mut result = profile.clone();
            result.models.retain(|model| owned.contains(model.id.as_str()));
            (!result.models.is_empty()).then_some(result)
        }).collect();
    }
    let target = format!("agent2api-quick-{}", def.id);
    live.iter().filter(|profile| super::is_gateway(&profile.base_url, gateway)
        && (profile.native.source_id == target || matches!(def.id, "claude-code" | "cline")))
        .cloned().collect()
}

pub fn guard(def: &ClientDef, live: &[Profile], gateway: &str) -> Result<(), String> {
    let target = format!("agent2api-quick-{}", def.id);
    if live.iter().any(|profile| profile.native.source_id == target && !super::is_gateway(&profile.base_url, gateway)) {
        return Err("一键接入条目已被改为其他服务，请从客户端实际配置中编辑，避免覆盖".into());
    }
    Ok(())
}
