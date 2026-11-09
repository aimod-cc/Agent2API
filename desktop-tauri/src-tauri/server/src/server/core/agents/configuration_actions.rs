use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::server::{core::account_store::AccountStore, db::Db};
use super::{apply, catalog, clients, configurations, discover, model_config, native, paths, preview, store, validate, ClientDef, Profile};

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

struct Context {
    path: PathBuf,
    profiles: Vec<Profile>,
    revision: String,
    live: Vec<Profile>,
}

fn context(def: &ClientDef, payload: &Value, port: u16) -> Result<Context, String> {
    let detection = paths::detect(def);
    if detection.path.to_string_lossy() != text(payload, "configPath") {
        return Err("客户端配置路径已变化，请重新检测后再操作".into());
    }
    let (mut profiles, revision) = store::load_with_revision(def.id)?;
    if revision != text(payload, "revision") {
        return Err("配置列表已被其他窗口修改，请重新检测后再操作".into());
    }
    for profile in &mut profiles { model_config::normalize(def, profile); }
    let mut live = discover::read(def, &detection.path)?;
    if native::revision(&live).as_deref() != Some(text(payload, "nativeRevision")) {
        return Err("客户端配置已被其他程序修改，请重新检测后再操作".into());
    }
    let gateway = format!("http://127.0.0.1:{port}");
    for profile in &mut live { profile.kind = super::kind_of(&profile.base_url, &gateway).into(); }
    Ok(Context { path: detection.path, profiles, revision, live })
}

fn entry(def: &ClientDef, context: &Context, payload: &Value) -> Result<configurations::Configuration, String> {
    let id = text(payload, "profileId");
    let live_id = text(payload, "liveId");
    configurations::list(def, &context.path, &context.profiles, &context.live, def.writable).into_iter()
        .find(|entry| entry.profile.id == id && entry.live_id.as_deref().unwrap_or("") == live_id)
        .ok_or_else(|| "配置列表已变化，请重新检测后再操作".into())
}

fn draft(def: &ClientDef, payload: &Value, old: Option<&Profile>) -> Result<Profile, String> {
    let value = payload.get("profile").ok_or("缺少配置内容")?;
    let mut profile = store::from_request(value, old)?;
    if old.is_some_and(|old| old.id != profile.id) { return Err("不能修改配置标识".into()); }
    model_config::normalize(def, &mut profile);
    Ok(profile)
}

fn validate_save(def: &ClientDef, path: &Path, profile: &Profile, live: Option<&Profile>, port: u16) -> Result<(), String> {
    let detection = paths::detect(def);
    if detection.unsupported { return Err(detection.note.unwrap_or_else(|| def.note.into())); }
    let mut definition = *def;
    if live.is_some_and(|profile| !profile.enabled) { definition.default_model = false; }
    validate::profile(&definition, profile)?;
    validate::require_gateway_url(profile, port)?;
    if paths::detect(def).path != path { return Err("客户端路径已变化，请重新检测".into()); }
    Ok(())
}

pub fn save(def: &ClientDef, payload: &Value, db: Option<&Db>, port: u16) -> Result<(), String> {
    let mut context = context(def, payload, port)?;
    let creating = text(payload, "profileId").is_empty();
    let old = if creating { None } else { Some(entry(def, &context, payload)?) };
    if old.as_ref().is_some_and(|entry| entry.live_id.is_some()) {
        return Err("此配置已存在于客户端，保存前必须预览并确认写入".into());
    }
    if old.as_ref().is_some_and(|entry| !entry.editable) { return Err("此配置暂不支持编辑，请在客户端中管理".into()); }
    let mut profile = draft(def, payload, old.as_ref().map(|entry| &entry.profile))?;
    if creating && (context.profiles.iter().any(|item| item.id == profile.id) || context.live.iter().any(|item| item.id == profile.id)) {
        return Err("配置标识已存在，请重新新增".into());
    }
    validate_save(def, &context.path, &profile, None, port)?;
    profile.updated_at = crate::server::logging::now_ms();
    if let Some(old) = old { context.profiles.retain(|item| !old.saved_ids.contains(&item.id)); }
    context.profiles.push(profile);
    if context.profiles.len() > 200 && creating { return Err("最多保存 200 套配置".into()); }
    store::save_client(db, def.id, &context.profiles, &context.revision)
}

pub fn remove_saved(def: &ClientDef, payload: &Value, db: Option<&Db>, port: u16) -> Result<(), String> {
    let mut context = context(def, payload, port)?;
    let selected = entry(def, &context, payload)?;
    if selected.live_id.is_some() { return Err("删除客户端中的配置前必须预览并确认".into()); }
    context.profiles.retain(|profile| !selected.saved_ids.contains(&profile.id));
    store::save_client(db, def.id, &context.profiles, &context.revision)
}

fn plan(def: &ClientDef, context: &Context, selected: &configurations::Configuration, payload: &Value, accounts: &AccountStore, port: u16) -> Result<clients::Plan, String> {
    if !def.writable || paths::detect(def).unsupported { return Err(def.note.into()); }
    let action = text(payload, "operation");
    let live = selected.live_id.as_deref().and_then(|id| context.live.iter().find(|profile| profile.id == id));
    let mut plan = match action {
        "config-delete" => {
            let live = live.ok_or("配置已不在客户端中，请重新检测")?;
            let mut plan = clients::delete_plan(def, &context.path, live)?;
            plan.profile.id = selected.profile.id.clone();
            plan
        }
        "config-edit" => {
            let live = live.ok_or("配置已不在客户端中，请重新检测")?;
            if !native::editable(def, live) { return Err("此配置暂不支持编辑，请在客户端中管理".into()); }
            let profile = draft(def, payload, Some(&selected.profile))?;
            if profile.protocol != live.protocol && def.id == "zed" && context.profiles.iter().any(|saved| !selected.saved_ids.contains(&saved.id)
                && saved.native.source_id == live.native.source_id && saved.protocol == profile.protocol) {
                return Err("目标协议已有独立配置，请编辑该配置，避免合并覆盖".into());
            }
            catalog::validate_profile(accounts, &profile)?;
            validate_save(def, &context.path, &profile, Some(live), port)?;
            native::edit_plan(def, &context.path, live, &profile, port)?
        }
        "config-use" | "config-disable" => {
            if !selected.editable { return Err("此配置暂不支持切换，请在客户端中管理".into()); }
            let mut profile = selected.profile.clone();
            if action == "config-disable" && !def.toggle { return Err("此客户端不支持停用配置".into()); }
            profile.enabled = action != "config-disable";
            catalog::validate_profile(accounts, &profile)?;
            if live.is_none() && profile.native.source_id.is_empty() && !matches!(def.id, "claude-code" | "gemini-cli" | "cline" | "workbuddy") {
                if let Some(existing) = context.live.iter().find(|current| current.native.source_id == clients::native_id(&profile)) {
                    if profile.protocol != existing.protocol || profile.base_url.trim_end_matches('/') != existing.base_url.trim_end_matches('/') {
                        return Err("目标配置标识已被客户端占用，请重新检测后编辑已有配置".into());
                    }
                }
            }
            if let Some(live) = live.filter(|_| def.toggle) {
                native::edit_plan(def, &context.path, live, &profile, port)?
            } else {
                let operation = if def.default_model { "default" } else { "apply" };
                let mut planned = clients::plan(def, &context.path, &profile, operation, port)?;
                if live.is_none() {
                    let wanted = profile.models.iter().map(|model| model.id.as_str()).collect();
                    for old in &context.live {
                        let replaces = matches!(def.id, "claude-code" | "gemini-cli")
                            || (def.id == "cline" && old.native.source_id == "openai-compatible")
                            || (!profile.native.source_id.is_empty() && profile.native.source_id == old.native.source_id && profile.protocol == old.protocol);
                        if replaces { clients::remove_models(def, &mut planned, old, &wanted)?; }
                    }
                }
                planned
            }
        }
        _ => return Err("配置操作无效".into()),
    };
    plan.operation = action.into();
    Ok(plan)
}

pub fn create_preview(def: &ClientDef, payload: &Value, accounts: &AccountStore, port: u16) -> Result<Value, String> {
    let context = context(def, payload, port)?;
    let selected = entry(def, &context, payload)?;
    preview::create_restore(def.id, &plan(def, &context, &selected, payload, accounts, port)?)
}

pub fn apply(def: &ClientDef, payload: &Value, accounts: &AccountStore, db: Option<&Db>, port: u16) -> Result<Value, String> {
    let mut context = context(def, payload, port)?;
    let selected = entry(def, &context, payload)?;
    let plan = plan(def, &context, &selected, payload, accounts, port)?;
    let deleting = text(payload, "operation") == "config-delete";
    configurations::remember(def, &context.path, &mut context.profiles, &context.live, selected.live_id.as_deref(), &plan.profile.id);
    context.profiles.retain(|profile| !selected.saved_ids.contains(&profile.id));
    if !deleting {
        let mut profile = plan.profile.clone();
        if context.profiles.iter().any(|item| item.id == profile.id) {
            return Err("配置标识发生冲突，请重新检测后再操作".into());
        }
        profile.updated_at = crate::server::logging::now_ms();
        context.profiles.push(profile);
    }
    if native::revision(&discover::read(def, &context.path)?).as_deref() != Some(text(payload, "nativeRevision")) {
        return Err("客户端配置已变化，请重新检测后再操作".into());
    }
    let changed = plan.changes.iter().any(|change| !change.unchanged());
    apply::apply_plan_with(def, &context.path, &plan, text(payload, "token"), |write| {
        store::save_client_with(db, def.id, &context.profiles, &context.revision, write)
    })?;
    Ok(json!({ "clientId": def.id, "changed": changed, "verified": true, "needsRestart": def.needs_restart }))
}
