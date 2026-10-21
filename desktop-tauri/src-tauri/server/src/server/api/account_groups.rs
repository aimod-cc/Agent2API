//! Account-group management API.
//!
//! The service owns SQLite transactions and the management mutex.  This module
//! only translates the HTTP DTOs, builds the request-time preview, and checks
//! the cross-store Key reference barrier before deleting a group.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};

use crate::server::core::account_groups::{
    AccountGroup, AccountGroupError, AccountGroupMember, MemberType,
};
use crate::server::core::api_keys::{self, RouteMode};
use crate::server::core::{custom_providers, providers, routing};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

fn service_error(error: AccountGroupError) -> Response {
    errors::management_error(error.status_code, error.message)
}

fn body_object(body: &Bytes) -> Result<Map<String, Value>, Response> {
    let payload = parse_body(body).map_err(|error| errors::management_error(400, error.message))?;
    payload
        .as_object()
        .cloned()
        .ok_or_else(|| errors::management_error(400, "请求体必须是 JSON 对象"))
}

fn member_json(member: &AccountGroupMember) -> Value {
    json!({
        "position": member.position,
        "type": member.member_type.as_str(),
        "id": member.member_id,
    })
}

fn group_json(group: &AccountGroup, accounts: &[Value]) -> Value {
    let resolved = crate::server::core::account_groups::resolve(&group.members, accounts);
    let missing_members: Vec<Value> = resolved
        .missing_members
        .iter()
        .map(|member| {
            json!({
                "position": member.position,
                "type": member.member_type.as_str(),
                "id": member.member_id,
            })
        })
        .collect();
    let providers: Vec<Value> = resolved
        .providers
        .iter()
        .map(|id| {
            json!({
                "id": id,
                "label": crate::server::core::providers::label_of(id),
            })
        })
        .collect();

    json!({
        "id": group.id,
        "name": group.name,
        "createdAt": group.created_at,
        "updatedAt": group.updated_at,
        "members": group.members.iter().map(member_json).collect::<Vec<_>>(),
        "missingMembers": missing_members,
        "preview": {
            "accounts": resolved.accounts,
            "providers": providers,
        },
    })
}

fn groups_payload(state: &ServerState) -> Result<Value, AccountGroupError> {
    let groups = state.account_groups().list()?;
    let accounts = routing::accounts_of(&state.store().list_accounts());
    Ok(json!({
        "groups": groups
            .iter()
            .map(|group| group_json(group, &accounts))
            .collect::<Vec<_>>(),
    }))
}

fn group_payload(state: &ServerState, group: &AccountGroup) -> Value {
    let accounts = routing::accounts_of(&state.store().list_accounts());
    json!({ "group": group_json(group, &accounts) })
}

fn required_name(object: &Map<String, Value>) -> Result<&str, Response> {
    object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| errors::management_error(400, "缺少账号组名称"))
}

fn provider_exists(id: &str) -> bool {
    providers::is_known_provider_id(id) || custom_providers::is_custom_provider_id(id)
}

fn parse_members(
    state: &ServerState,
    object: &Map<String, Value>,
) -> Result<Vec<AccountGroupMember>, Response> {
    let members = object
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| errors::management_error(400, "members 必须是数组"))?;

    let accounts = routing::accounts_of(&state.store().list_accounts());
    members
        .iter()
        .map(|value| {
            let member = value
                .as_object()
                .ok_or_else(|| errors::management_error(400, "成员必须是 JSON 对象"))?;
            let member_type = member
                .get("type")
                .and_then(Value::as_str)
                .and_then(MemberType::parse)
                .ok_or_else(|| {
                    errors::management_error(400, "成员 type 必须是 account 或 provider")
                })?;
            let member_id = member
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| errors::management_error(400, "成员缺少 id"))?;
            match member_type {
                MemberType::Account => {
                    if !accounts
                        .iter()
                        .any(|account| routing::account_id(account) == Some(member_id))
                    {
                        return Err(errors::management_error(
                            400,
                            format!("账号不存在: {member_id}"),
                        ));
                    }
                }
                MemberType::Provider => {
                    if !provider_exists(member_id) {
                        return Err(errors::management_error(
                            400,
                            format!("提供商不存在: {member_id}"),
                        ));
                    }
                }
            }
            Ok(AccountGroupMember {
                position: 0,
                member_type,
                member_id: member_id.to_string(),
            })
        })
        .collect()
}

fn used_by(group_id: &str) -> Vec<Value> {
    api_keys::list()
        .into_iter()
        .filter(|entry| {
            entry.route_mode == RouteMode::AccountGroup
                && entry.account_group_id.as_deref() == Some(group_id)
        })
        .map(|entry| {
            json!({
                "id": entry.id,
                "name": entry.name,
            })
        })
        .collect()
}

fn delete_conflict(group_id: &str, used: Vec<Value>) -> Response {
    let names: Vec<String> = used
        .iter()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let suffix = if names.is_empty() {
        "已有 API Key 引用".to_string()
    } else {
        format!("已被 API Key「{}」引用", names.join("、"))
    };
    (
        axum::http::StatusCode::CONFLICT,
        axum::Json(json!({
            "success": false,
            "error": format!("账号组 {group_id} {suffix}，请先解除引用"),
            "usedBy": used,
        })),
    )
        .into_response()
}

/// GET /api/account-groups
pub async fn list_groups(State(state): State<ServerState>) -> Response {
    match state.with_management_lock(|| groups_payload(&state)) {
        Ok(payload) => ok_json(payload),
        Err(error) => service_error(error),
    }
}

/// POST /api/account-groups
pub async fn create_group(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let name = match required_name(&object) {
        Ok(name) => name,
        Err(response) => return response,
    };

    match state.with_management_lock(|| state.account_groups().create(name)) {
        Ok(group) => ok_json(group_payload(&state, &group)),
        Err(error) => service_error(error),
    }
}

/// PATCH /api/account-groups/{id}
pub async fn update_group(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let name = match required_name(&object) {
        Ok(name) => name,
        Err(response) => return response,
    };

    match state.with_management_lock(|| state.account_groups().rename(&id, name)) {
        Ok(group) => ok_json(group_payload(&state, &group)),
        Err(error) => service_error(error),
    }
}

/// PUT /api/account-groups/{id}/members
pub async fn replace_group_members(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<AccountGroup, Response> = state.with_management_lock(|| {
        let object = body_object(&body)?;
        let members = parse_members(&state, &object)?;
        state
            .account_groups()
            .replace_members(&id, members)
            .map_err(service_error)
    });
    match result {
        Ok(group) => ok_json(group_payload(&state, &group)),
        Err(response) => response,
    }
}

/// DELETE /api/account-groups/{id}
pub async fn delete_group(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    let result: Result<Value, Response> = state.with_management_lock(|| {
        let used = used_by(&id);
        if !used.is_empty() {
            return Err(delete_conflict(&id, used));
        }
        state
            .account_groups()
            .delete(&id)
            .map(|()| json!({ "deleted": true }))
            .map_err(service_error)
    });

    match result {
        Ok(payload) => ok_json(payload),
        Err(response) => response,
    }
}
