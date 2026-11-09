//! 客户端配置管理：草稿保存不写客户端；应用与恢复必须持有准确预览的确认令牌。

use axum::{body::Bytes, extract::{Query, State}, response::Response};
use serde_json::{json, Value};
use std::collections::HashSet;

use crate::server::{
    core::agents::{self, Profile},
    errors::management_error,
    http::{ok_json, parse_body},
    ServerState,
};

fn payload(body: &Bytes) -> Result<Value, String> {
    parse_body(body).map_err(|_| "请求内容不是有效 JSON".into())
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("").trim()
}
fn definition(value: &Value) -> Result<&'static agents::ClientDef, String> {
    agents::find(text(value, "clientId")).ok_or_else(|| "未知客户端".into())
}

fn response(result: Result<Value, String>) -> Response {
    match result {
        Ok(value) => ok_json(value),
        Err(error) => management_error(
            if error.contains("已被其他")
                || error.contains("已变化")
                || error.contains("已被其他窗口")
            {
                409
            } else {
                400
            },
            error,
        ),
    }
}

pub async fn list_agents(State(state): State<ServerState>) -> Response {
    ok_json(agents::snapshot(state.port))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelQuery {
    gateway_key_id: Option<String>,
}

pub async fn list_models(State(state): State<ServerState>, Query(query): Query<ModelQuery>) -> Response {
    response(agents::catalog::models(state.store(), query.gateway_key_id.as_deref())
        .map(|models| json!({ "models": models })))
}

pub async fn save_profiles(State(state): State<ServerState>, body: Bytes) -> Response {
    response((|| {
        let payload = payload(&body)?;
        let def = definition(&payload)?;
        // 路径偏好不改变客户端自身的配置加载位置。
        if text(&payload, "action") == "path" {
            agents::paths::save_override(def, text(&payload, "configPath"))?;
            return Ok(agents::snapshot(state.port));
        }
        if text(&payload, "action") == "config-save" {
            agents::configuration_actions::save(def, &payload, state.db(), state.port)?;
            return Ok(agents::snapshot(state.port));
        }
        if text(&payload, "action") == "config-remove" {
            agents::configuration_actions::remove_saved(def, &payload, state.db(), state.port)?;
            return Ok(agents::snapshot(state.port));
        }
        let current = agents::store::load_client(def.id)?;
        let items = payload
            .get("profiles")
            .and_then(Value::as_array)
            .ok_or("profiles 必须是数组")?;
        if items.len() > 200 {
            return Err("最多保存 200 套方案".into());
        }
        let mut profiles = Vec::new();
        let mut ids = HashSet::new();
        for item in items {
            let old = current
                .iter()
                .find(|profile| profile.id == text(item, "id"));
            let mut profile = agents::store::from_request(item, old)?;
            agents::model_config::normalize(def, &mut profile);
            if !ids.insert(profile.id.clone()) || profile.id.is_empty() {
                return Err("方案 ID 不能为空或重复".into());
            }
            let unchanged = old.is_some_and(|old| same_profile(old, &profile));
            if !unchanged {
                agents::validate::profile(def, &profile)?;
                agents::validate::require_gateway_url(&profile, state.port)?;
                profile.updated_at = crate::server::logging::now_ms();
            } else if let Some(old) = old {
                profile.updated_at = old.updated_at;
            }
            profiles.push(profile);
        }
        agents::store::save_client(state.db(), def.id, &profiles, text(&payload, "revision"))?;
        Ok(agents::snapshot(state.port))
    })())
}

fn same_profile(a: &Profile, b: &Profile) -> bool {
    a.name == b.name
        && a.kind == b.kind
        && a.protocol == b.protocol
        && a.base_url == b.base_url
        && a.model == b.model
        && a.effort == b.effort
        && a.enabled == b.enabled
        && a.default == b.default
        && a.models == b.models
        && a.claude_models == b.claude_models
        && a.auth_mode == b.auth_mode
        && a.api_key == b.api_key
        && a.env_key == b.env_key
        && a.gateway_key_id == b.gateway_key_id
        && a.note == b.note
}

fn operation(def: &agents::ClientDef, payload: &Value) -> Result<String, String> {
    let operation = text(payload, "operation");
    let operation = if operation.is_empty() {
        if def.behavior == "single" {
            "default"
        } else {
            "apply"
        }
    } else {
        operation
    };
    if !["apply", "default"].contains(&operation) {
        return Err("应用操作无效".into());
    }
    if operation == "default" && !def.default_model {
        return Err("这个客户端没有本页可修改的默认模型字段".into());
    }
    Ok(operation.into())
}

fn selected_profile(
    def: &agents::ClientDef,
    payload: &Value,
    draft: bool,
) -> Result<Profile, String> {
    let profiles = agents::store::load_client(def.id)?;
    if draft {
        if let Some(value) = payload.get("profile") {
            let old = profiles
                .iter()
                .find(|profile| profile.id == text(value, "id"));
            let mut profile = agents::store::from_request(value, old)?;
            agents::model_config::normalize(def, &mut profile);
            return Ok(profile);
        }
    }
    let wanted = text(payload, "profileId");
    let mut profile = profiles.into_iter().find(|profile| profile.id == wanted)
        .ok_or_else(|| "没有找到要应用的已保存方案".to_string())?;
    agents::model_config::normalize(def, &mut profile);
    Ok(profile)
}

pub async fn preview_profile(State(state): State<ServerState>, body: Bytes) -> Response {
    response((|| {
        let payload = payload(&body)?;
        let def = definition(&payload)?;
        let path = agents::paths::detect(def).path;
        let action = text(&payload, "operation");
        if action.starts_with("config-") {
            return agents::configuration_actions::create_preview(def, &payload, state.store(), state.port);
        }
        if action == "native-edit" || action == "native-delete" {
            return agents::native::create_preview(def, &path, &payload, state.port);
        }
        if action == "restore" || action == "disconnect" {
            return agents::apply::restore_preview(def, &path, action == "disconnect");
        }
        let profile = selected_profile(def, &payload, true)?;
        agents::catalog::validate_profile(state.store(), &profile)?;
        if action == "quick-connect" {
            return agents::quick_connect::create_preview(def, &path, &profile, state.port);
        }
        agents::preview::create(def, &path, &profile, &operation(def, &payload)?, state.port)
    })())
}

pub async fn apply_profile(State(state): State<ServerState>, body: Bytes) -> Response {
    response((|| {
        let payload = payload(&body)?;
        let def = definition(&payload)?;
        let path = agents::paths::detect(def).path;
        let action = text(&payload, "operation");
        let applied = if action.starts_with("config-") {
            agents::configuration_actions::apply(def, &payload, state.store(), state.db(), state.port)?
        } else if action == "native-edit" || action == "native-delete" {
            agents::native::apply(def, &path, &payload, state.port)?
        } else {
            let profile = selected_profile(def, &payload, action == "quick-connect")?;
            agents::catalog::validate_profile(state.store(), &profile)?;
            if action == "quick-connect" {
                agents::quick_connect::apply(def, &path, &profile, text(&payload, "token"), state.port)?
            } else {
                agents::apply::apply(def, &path, &profile, &operation(def, &payload)?, text(&payload, "token"), state.port)?
            }
        };
        let mut snapshot = agents::snapshot(state.port);
        if let Some(object) = snapshot.as_object_mut() {
            object.insert("applied".into(), applied);
        }
        Ok(snapshot)
    })())
}

pub async fn restore_client(State(state): State<ServerState>, body: Bytes) -> Response {
    response((|| {
        let payload = payload(&body)?;
        let def = definition(&payload)?;
        let path = agents::paths::detect(def).path;
        let restored = agents::apply::restore(
            def,
            &path,
            text(&payload, "operation") == "disconnect",
            text(&payload, "token"),
        )?;
        let mut snapshot = agents::snapshot(state.port);
        if let Some(object) = snapshot.as_object_mut() {
            object.insert("restored".into(), restored);
        }
        Ok(snapshot)
    })())
}

pub async fn import_client(State(state): State<ServerState>, body: Bytes) -> Response {
    response((|| {
        let payload = payload(&body)?;
        let def = definition(&payload)?;
        let path = agents::paths::detect(def).path;
        let mut live = agents::discover::read(def, &path)?;
        let gateway = format!("http://127.0.0.1:{}", state.port);
        for profile in &mut live {
            profile.kind = agents::kind_of(&profile.base_url, &gateway).into();
        }
        if !payload
            .get("adopt")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(json!({ "profiles": live.iter().map(Profile::view).collect::<Vec<_>>() }));
        }
        let current = agents::store::load_client(def.id)?;
        let mut merged = current.clone();
        let mut count = 0;
        for mut profile in live {
            if merged.iter().any(|entry| {
                entry.native.source_id == profile.native.source_id && same_profile(entry, &profile)
            }) {
                continue;
            }
            if merged.iter().any(|entry| entry.id == profile.id) {
                let base_id = profile.id.clone();
                let mut suffix = 2;
                while merged
                    .iter()
                    .any(|entry| entry.id == format!("{base_id}-{suffix}"))
                {
                    suffix += 1;
                }
                profile.id = format!("{base_id}-{suffix}");
            }
            profile.updated_at = crate::server::logging::now_ms();
            merged.push(profile);
            count += 1;
        }
        if count > 0 {
            agents::store::save_client(state.db(), def.id, &merged, text(&payload, "revision"))?;
        }
        let mut snapshot = agents::snapshot(state.port);
        if let Some(object) = snapshot.as_object_mut() {
            object.insert("imported".into(), json!({"clientId":def.id,"count":count}));
        }
        Ok(snapshot)
    })())
}
