use serde_json::{json, Value};
use std::{collections::HashSet, path::Path};

use super::{clients, discover, preview, ClientDef, Profile};

pub fn plan(def: &ClientDef, path: &Path, profile: &Profile, port: u16) -> Result<clients::Plan, String> {
    if profile.id != format!("quick-{}", def.id) || !profile.native.source_id.is_empty() || profile.kind != "gateway" {
        return Err("一键接入只能修改本客户端的专用网关配置".into());
    }
    let operation = if def.default_model { "default" } else { "apply" };
    let live = discover::read(def, path)?;
    let gateway = format!("http://127.0.0.1:{port}");
    super::quick_profiles::guard(def, &live, &gateway)?;
    let mut plan = clients::plan(def, path, profile, operation, port)?;
    let wanted = profile.models.iter().map(|model| model.id.as_str()).collect::<HashSet<_>>();
    for old in super::quick_profiles::profiles(def, &live, &gateway).iter().filter(|old| def.id != "zed" || old.protocol == profile.protocol) {
        clients::remove_models(def, &mut plan, old, &wanted)?;
    }
    for change in &plan.changes { change.check()?; }
    plan.operation = "quick-connect".into();
    Ok(plan)
}

pub fn create_preview(def: &ClientDef, path: &Path, profile: &Profile, port: u16) -> Result<Value, String> {
    preview::create_restore(def.id, &plan(def, path, profile, port)?)
}

pub fn apply(def: &ClientDef, path: &Path, profile: &Profile, token: &str, port: u16) -> Result<Value, String> {
    let plan = plan(def, path, profile, port)?;
    let changed = plan.changes.iter().any(|change| !change.unchanged());
    super::apply::apply_plan(def, path, &plan, token)?;
    Ok(json!({ "clientId": def.id, "changed": changed, "verified": true, "needsRestart": def.needs_restart }))
}
