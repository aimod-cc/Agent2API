use serde_json::{json, Value};
use std::path::Path;

use super::{clients, discover, files, paths, preview, store, ClientDef, Profile};

fn live_profile(def: &ClientDef, path: &Path, id: &str, port: u16) -> Result<Profile, String> {
    let mut profile = discover::read(def, path)?.into_iter()
        .find(|profile| profile.id == id)
        .ok_or("客户端配置已变化，请重新检测后再编辑")?;
    profile.kind = super::kind_of(&profile.base_url, &format!("http://127.0.0.1:{port}")).into();
    Ok(profile)
}

fn read_revision(def: &ClientDef, path: &Path) -> Result<String, String> {
    let values = discover::read(def, path)?;
    revision(&values).ok_or_else(|| "读取原生配置版本失败".into())
}

pub fn revision(profiles: &[Profile]) -> Option<String> {
    let mut values = profiles.to_vec();
    for profile in &mut values { profile.kind.clear(); }
    let bytes = serde_json::to_vec(&values).ok()?;
    Some(store::digest(&bytes))
}

pub fn editable(def: &ClientDef, profile: &Profile) -> bool {
    def.writable && def.protocols.contains(&profile.protocol.as_str())
        && !(def.id == "cline" && profile.native.source_id != "openai-compatible")
        && !(def.models && profile.native.entry.is_null() && !matches!(def.id, "claude-code" | "codex"))
}

pub fn request_plan(def: &ClientDef, path: &Path, payload: &Value, port: u16) -> Result<clients::Plan, String> {
    if !def.writable || paths::detect(def).unsupported {
        return Err(def.note.into());
    }
    let action = payload.get("operation").and_then(Value::as_str).unwrap_or("");
    let id = payload.get("profileId").and_then(Value::as_str).unwrap_or("");
    let expected = payload.get("nativeRevision").and_then(Value::as_str).unwrap_or("");
    if expected.is_empty() || expected != read_revision(def, path)? {
        return Err("客户端配置已被其他程序修改，请重新检测后再操作".into());
    }
    let old = live_profile(def, path, id, port)?;
    if action == "native-delete" {
        return clients::delete_plan(def, path, &old);
    }
    if action != "native-edit" || !editable(def, &old) {
        return Err("这个客户端条目的原生协议暂不支持编辑".into());
    }
    let value = payload.get("profile").ok_or("缺少要编辑的配置")?;
    let mut profile = store::from_request(value, Some(&old))?;
    super::model_config::normalize(def, &mut profile);
    if profile.id != old.id {
        return Err("不能修改客户端配置的标识".into());
    }
    profile.updated_at = old.updated_at;
    edit_plan(def, path, &old, &profile, port)
}

pub(super) fn edit_plan(def: &ClientDef, path: &Path, old: &Profile, profile: &Profile, port: u16) -> Result<clients::Plan, String> {
    if def.id == "zed" && profile.protocol != old.protocol {
        let duplicates = old.native.entry.get("available_models").and_then(Value::as_array).is_some_and(|models| models.iter().any(|model|
            (model.pointer("/capabilities/chat_completions").and_then(Value::as_bool) == Some(false)) == (profile.protocol == "responses")
                && model.get("name").and_then(Value::as_str).is_some_and(|id| profile.models.iter().any(|model| model.id == id))));
        if duplicates { return Err("目标协议已有同名模型，不能覆盖另一个协议的配置".into()); }
    }
    let mut definition = *def;
    if !old.enabled { definition.default_model = false; }
    let operation = if old.enabled && definition.default_model { "default" } else { "apply" };
    let mut plan = clients::plan(&definition, path, profile, operation, port)?;
    let model_ids: std::collections::HashSet<_> = profile.models.iter().map(|model| model.id.as_str()).collect();
    clients::remove_models(def, &mut plan, old, &model_ids)?;
    for change in &mut plan.changes {
        if change.path != path || def.id == "dsh" { continue; }
        let original = files::text(change.before.as_deref())?;
        let after = files::text(change.after.as_deref())?;
        let mut value = files::parse(def.format, &after)?;
        clients::preserve_selection(def, &mut value, old)?;
        change.after = Some(files::render(def.format, &original, &value)?);
    }
    plan.operation = "native-edit".into();
    plan.warnings.push("直接修改所选客户端配置；保留其它条目、未知字段与认证，写入前自动备份。".into());
    for change in &plan.changes { change.check()?; }
    Ok(plan)
}

pub fn create_preview(def: &ClientDef, path: &Path, payload: &Value, port: u16) -> Result<Value, String> {
    preview::create_restore(def.id, &request_plan(def, path, payload, port)?)
}

pub fn apply(def: &ClientDef, path: &Path, payload: &Value, port: u16) -> Result<Value, String> {
    let token = payload.get("token").and_then(Value::as_str).unwrap_or("");
    if token.is_empty() { return Err("写入前必须生成预览并确认".into()); }
    let plan = request_plan(def, path, payload, port)?;
    let changed = plan.changes.iter().any(|change| !change.unchanged());
    super::apply::apply_plan(def, path, &plan, token)?;
    Ok(json!({ "clientId": def.id, "changed": changed, "verified": true, "needsRestart": def.needs_restart }))
}
