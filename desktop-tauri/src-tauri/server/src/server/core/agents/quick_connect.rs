use serde_json::{json, Value};
use std::{collections::HashSet, path::Path};

use super::{clients, discover, preview, ClientDef, Profile};

pub fn plan(def: &ClientDef, path: &Path, profile: &Profile, port: u16) -> Result<clients::Plan, String> {
    if profile.id != format!("quick-{}", def.id) || !profile.native.source_id.is_empty() || profile.kind != "gateway" {
        return Err("一键接入只能修改本客户端的专用网关配置".into());
    }
    let operation = if def.default_model { "default" } else { "apply" };
    let mut plan = clients::plan(def, path, profile, operation, port)?;
    let target = clients::native_id(profile);
    let live = discover::read(def, path)?;
    let wanted = profile.models.iter().map(|model| model.id.as_str()).collect::<HashSet<_>>();
    // 只替换一键接入自己创建的提供商；不删除客户端的其它提供商或手工条目。
    for old in live.iter().filter(|old| old.native.source_id == target && (def.id != "zed" || old.protocol == profile.protocol)) {
        if !super::is_gateway(&old.base_url, &format!("http://127.0.0.1:{port}")) {
            return Err("一键接入条目已被改为其他服务，请从客户端实际配置中编辑，避免覆盖".into());
        }
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
