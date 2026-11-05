//! 路由工作台：模型覆盖、实时指标、会话粘滞与持久化的决策回放。

use std::collections::{HashMap, HashSet};

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::{config, errors, ServerState};
use crate::server::core::{providers, routing, routing_scope};
use crate::server::http::{ok_json, parse_body};

use super::routing_api;

type Params = HashMap<String, String>;

fn model_of(params: &Params) -> Option<String> {
    params.get("model").map(|model| model.trim()).filter(|model| !model.is_empty()).map(str::to_string)
}

use crate::server::core::routing_scope::available_providers;

pub async fn get_workspace(State(state): State<ServerState>, Query(params): Query<Params>) -> Response {
    let model = model_of(&params);
    let state_copy = state.clone();
    match tokio::task::spawn_blocking(move || workspace_json(&state_copy, model)).await {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(500, format!("读取路由工作台失败: {error}")),
    }
}

fn workspace_json(state: &ServerState, model: Option<String>) -> Value {
    let store = state.store();
    let guard = store.guard();
    let account_state = store.load(&guard);
    let account_snapshot = store.snapshot(&account_state, crate::server::core::account_store::store_view::Visibility::UserFacing);
    let (profiles, base_settings) = config::routing_snapshot();
    let legacy_global_participation = config::routing_workspace::global_accounts::legacy_snapshot(profiles.global.as_ref(), &account_state.accounts);
    drop(guard);
    let accounts = routing::accounts_of(&account_snapshot);
    let all_providers = available_providers(&accounts);
    let eligible = model.as_deref().map(providers::catalog::forwarding_providers)
        .unwrap_or_else(|| all_providers.clone());
    let model_name = model.as_deref().unwrap_or("");
    let effective = routing_scope::materialize_profile_with(&accounts, &eligible, model_name, &profiles);
    let mut inherited = (*profiles).clone();
    inherited.models.remove(&model_name.to_ascii_lowercase());
    let inherited_profile = routing_scope::materialize_profile_with(&accounts, &eligible, model_name, &inherited);
    let mut provider_ids = eligible.clone();
    for id in &effective.providers {
        if !provider_ids.contains(id) { provider_ids.push(id.clone()); }
    }
    let provider_rows: Vec<Value> = provider_ids.iter().map(|id| json!({
        "id": id,
        "label": providers::label_of(id),
        "accountCount": accounts.iter().filter(|account| routing::provider_of(account) == id).count(),
        "custom": crate::server::core::custom_providers::is_custom_provider_id(id),
        "available": eligible.contains(id),
    })).collect();
    let mut models = providers::catalog::advertised_model_ids(&state.store());
    models.retain(|model| profiles.group_for_model(model).is_none());
    for model in profiles.models.keys() {
        if !models.iter().any(|known| known.eq_ignore_ascii_case(model)) { models.push(model.clone()); }
    }
    models.sort_by_key(|model| model.to_ascii_lowercase());
    models.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    let now = crate::server::logging::now_ms();
    let settings = profiles.settings_for(model_name, base_settings);
    let revision = config::routing_workspace::revision(&effective, settings, profiles.model_profile(model_name).is_some());
    let keys = routing::CooldownKeys::new(model_name);
    let account_states: HashMap<String, Value> = accounts.iter().filter_map(|account| {
        let id = routing::account_id(account)?;
        let reset_at = (!model_name.is_empty()).then(|| routing::rate_limit_reset_at(account, &keys, now)).unwrap_or(0);
        Some((id.to_string(), json!({ "model": keys.for_account(account), "resetAt": reset_at,
            "limitedModels": account.get("rateLimits").and_then(Value::as_object).map(|limits| limits.values().filter(|limit| limit.get("resetAt").and_then(Value::as_i64).unwrap_or(0) > now).count()).unwrap_or(0) })))
    }).collect();
    let mut metrics = state.request_stats().routing_metrics(model.as_deref());
    metrics["accountConnections"] = json!(state.upstream().connections().snapshot().into_iter().collect::<HashMap<_, _>>());
    json!({
        "model": model,
        "customized": profiles.model_profile(model_name).is_some(),
        "globalCustomized": profiles.global.is_some(),
        "globalAccountControl": true,
        "legacyGlobalParticipation": legacy_global_participation,
        "configuredModels": profiles.models.keys().collect::<Vec<_>>(),
        "models": models,
        "profile": effective,
        "inheritedProfile": inherited_profile,
        "providers": provider_rows,
        "accounts": accounts,
        "sorting": account_snapshot.get("sorting"),
        "queue": routing_api::queue_for_accounts(state, (!model_name.is_empty()).then(|| model_name.to_string()), &accounts),
        "settings": routing_api::routing_json(settings),
        "globalSettings": routing_api::routing_json(profiles.settings_for("", base_settings)),
        "revision": revision,
        "accountStates": account_states,
        "metrics": metrics,
        "gateway": { "port": state.port, "host": state.host.to_string() },
        "stickyCount": state.upstream().route_sticky_sessions(model.as_deref(), now).len(),
        "updatedAt": now,
    })
}

pub async fn put_profile(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else { return errors::management_error(400, "请求体必须是 JSON 对象") };
    if let Some(value) = object.get("migrateGlobalAccounts") {
        if object.len() != 1 { return errors::management_error(400, "转换全局账号状态不能与其他配置修改一起提交"); }
        let Some(expected) = value.as_str().filter(|value| value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())) else {
            return errors::management_error(400, "转换确认已失效，请重新读取当前配置");
        };
        if let Err(error) = config::routing_workspace::global_accounts::migrate(&state.store(), expected) {
            return errors::management_error(409, error);
        }
        return ok_json(workspace_json(&state, None));
    }
    let model = match object.get("model") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(model)) => model.clone(),
        _ => return errors::management_error(400, "model 必须是模型名或 null"),
    };
    let model = match config::normalize_routing_model(&model) {
        Ok(model) => model,
        Err(error) => return errors::management_error(400, error),
    };
    if config::routing_profiles().group_for_model(&model).is_some() {
        return errors::management_error(400, "此入口是自定义路由组，请在路由组中修改，不能覆盖为模型独立配置");
    }
    let Some(raw) = object.get("profile") else { return errors::management_error(400, "缺少 profile（恢复继承请传 null）") };
    let mut profile = if raw.is_null() { None } else {
        let profile: config::RoutingProfile = match serde_json::from_value(raw.clone()) {
            Ok(profile) => profile,
            Err(error) => return errors::management_error(400, format!("路由配置格式错误: {error}")),
        };
        if let Err(error) = config::validate_routing_profile(&profile) { return errors::management_error(400, error); }
        let accounts = routing::accounts_of(&state.store().list_accounts());
        let previous = config::routing_profiles();
        let old = previous.effective(&model);
        let known: HashSet<String> = available_providers(&accounts).into_iter()
            .chain(old.into_iter().flat_map(|profile| profile.providers.iter().chain(&profile.disabled_providers).cloned())).collect();
        if let Some(id) = profile.providers.iter().chain(&profile.disabled_providers)
            .find(|id| !known.contains(*id) && !providers::is_known_provider_id(id)) {
            return errors::management_error(400, format!("未知提供商「{id}」"));
        }
        for (provider, ids) in &profile.account_order {
            for id in ids {
                let current = accounts.iter().find(|account| routing::account_id(account) == Some(id));
                if current.is_some_and(|account| routing::provider_of(account) != provider) {
                    return errors::management_error(400, format!("账号「{id}」不属于提供商「{provider}」"));
                }
                if current.is_none() && !old.is_some_and(|profile| profile.account_order.values().any(|ids| ids.contains(id))) {
                    return errors::management_error(400, format!("账号「{id}」不存在，请刷新路由"));
                }
            }
        }
        Some(profile)
    };
    if let Some(revision) = object.get("revision") {
        let Some(revision) = revision.as_str().filter(|value| value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())) else {
            return errors::management_error(400, "配置版本无效，请刷新当前配置");
        };
        if let Err(error) = config::routing_workspace::save_workspace(&state.store(), &model, profile, revision) {
            let status = if error.starts_with("路由配置已在其他位置修改") { 409 } else { 400 };
            return errors::management_error(status, error);
        }
        return ok_json(workspace_json(&state, (!model.is_empty()).then_some(model)));
    }
    if model.is_empty() {
        if let Some(order) = object.get("orderChange") {
            let Some(kind) = order.get("kind").and_then(Value::as_str) else {
                return errors::management_error(400, "顺序修改缺少 kind 字段");
            };
            let provider = match kind {
                "provider" => None,
                "account" => match order.get("provider").and_then(Value::as_str) {
                    Some(provider) => Some(provider),
                    None => return errors::management_error(400, "账号顺序修改缺少 provider 字段"),
                },
                _ => return errors::management_error(400, "顺序修改 kind 必须为 provider 或 account"),
            };
            let Some(profile) = &profile else { return errors::management_error(400, "修改顺序必须提供路由配置") };
            let accounts = routing::accounts_of(&state.store().list_visible_accounts());
            let ids = match crate::server::core::routing_order::global_queue(&accounts, profile, provider) {
                Ok(ids) => ids,
                Err(error) => return errors::management_error(400, error),
            };
            if let Err(error) = state.store().reorder_accounts(&ids) { return super::accounts::store_error(error); }
            return ok_json(workspace_json(&state, None));
        }
        if let Some(profile) = &mut profile {
            profile.account_order.clear();
        }
    }
    if let Err(error) = config::save_routing_profile(&model, profile) {
        return errors::management_error(500, error);
    }
    let model = (!model.is_empty()).then_some(model);
    ok_json(workspace_json(&state, model))
}

pub async fn get_sticky(State(state): State<ServerState>, Query(params): Query<Params>) -> Response {
    let model = model_of(&params);
    let now = crate::server::logging::now_ms();
    let accounts = routing::accounts_of(&state.store().list_accounts());
    let mut entries = state.upstream().route_sticky_sessions(model.as_deref(), now);
    for entry in &mut entries {
        let id = entry.get("accountId").and_then(Value::as_str).unwrap_or("").to_string();
        let account = accounts.iter().find(|account| routing::account_id(account) == Some(&id));
        if let Some(object) = entry.as_object_mut() {
            object.insert("accountName".to_string(), Value::String(account.map(|account| routing::account_name(account, &id)).unwrap_or(id)));
            let provider = account.map(routing::provider_of).unwrap_or("");
            object.insert("provider".to_string(), Value::String(provider.to_string()));
            object.insert("providerLabel".to_string(), Value::String(providers::label_of(provider)));
        }
    }
    ok_json(json!({ "entries": entries, "now": now, "sticky": config::routing_settings_for_model(model.as_deref().unwrap_or("")).sticky.as_str() }))
}

pub async fn clear_sticky(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else { return errors::management_error(400, "请求体必须是 JSON 对象") };
    let id = match object.get("id") {
        None => None,
        Some(Value::String(id)) if id.len() == 64 && id.chars().all(|ch| ch.is_ascii_hexdigit()) => Some(id.as_str()),
        _ => return errors::management_error(400, "id 必须是会话列表返回的完整标识"),
    };
    let model = match object.get("model") {
        None => None,
        Some(Value::String(model)) if !model.trim().is_empty() => Some(model.trim()),
        _ => return errors::management_error(400, "model 必须是非空模型名"),
    };
    if id.is_none() && model.is_none() && payload.get("all") != Some(&Value::Bool(true)) {
        return errors::management_error(400, "解除全部会话必须显式传 all: true");
    }
    let cleared = state.upstream().clear_route_sticky(id, model);
    ok_json(json!({ "cleared": cleared, "success": true }))
}

pub async fn get_replays(State(state): State<ServerState>, Query(params): Query<Params>) -> Response {
    let model = model_of(&params);
    let offset = params.get("offset").and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
    let limit = params.get("limit").and_then(|value| value.parse::<usize>().ok()).unwrap_or(30);
    let stats = state.request_stats();
    let status = params.get("status").cloned().unwrap_or_default();
    match tokio::task::spawn_blocking(move || stats.routing_replays(model.as_deref(), offset, limit, &status)).await {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(500, format!("读取路由回放失败: {error}")),
    }
}
