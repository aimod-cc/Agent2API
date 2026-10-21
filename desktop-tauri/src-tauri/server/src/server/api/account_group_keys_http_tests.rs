//! HTTP contract tests for Key route fields and group deletion protection.

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use super::account_groups_http_support::{
    create_group, created_key_id, delete_group, delete_key, group_id, key_by_id, lock, request,
    unique,
};
use crate::server::core::api_keys;

#[tokio::test]
async fn it04_persists_explicit_route_fields_and_preserves_omitted_patch_fields() {
    let env = lock();
    let (status, created) = request(
        &env,
        Method::POST,
        "/api/account-groups",
        json!({ "name": unique("it04-group") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let group = group_id(&created);

    let (status, rejected_group_key) = request(
        &env,
        Method::POST,
        "/api/keys",
        json!({
            "name": unique("it04-rejected-group-key"),
            "key": format!("{}-secret", unique("it04-rejected")),
            "allowedProviders": ["workbuddy"],
            "routeMode": "account_group",
            "accountGroupId": group,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(rejected_group_key.to_string().contains("allowedProviders"));

    let default_name = unique("it04-default-key");
    let (status, default_key) = request(
        &env,
        Method::POST,
        "/api/keys",
        json!({
            "name": default_name,
            "key": format!("{default_name}-secret"),
            "allowedProviders": ["workbuddy"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(default_key["data"]["created"]["routeMode"], "default_pool");
    assert_eq!(
        default_key["data"]["created"]["accountGroupId"],
        Value::Null
    );
    assert_eq!(
        default_key["data"]["created"]["allowedProviders"],
        json!(["workbuddy"])
    );
    let default_id = created_key_id(&default_key);

    let (status, rejected_implicit_clear) = request(
        &env,
        Method::PATCH,
        &format!("/api/keys/{default_id}"),
        json!({
            "routeMode": "account_group",
            "accountGroupId": group,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(rejected_implicit_clear.to_string().contains("清空"));

    let (status, switched_default) = request(
        &env,
        Method::PATCH,
        &format!("/api/keys/{default_id}"),
        json!({
            "allowedProviders": [],
            "routeMode": "account_group",
            "accountGroupId": group,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let switched_default_key = switched_default["data"]["keys"]
        .as_array()
        .expect("Key 列表应为数组")
        .iter()
        .find(|entry| entry["id"] == default_id)
        .expect("切换后的默认池 Key 应存在");
    assert_eq!(switched_default_key["routeMode"], "account_group");
    assert_eq!(switched_default_key["accountGroupId"], group);
    assert_eq!(switched_default_key["allowedProviders"], json!([]));

    let group_name = unique("it04-group-key");
    let (status, group_key) = request(
        &env,
        Method::POST,
        "/api/keys",
        json!({
            "name": group_name,
            "key": format!("{group_name}-secret"),
            "routeMode": "account_group",
            "accountGroupId": group,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let group_key_id = created_key_id(&group_key);

    let legacy_name = unique("it04-legacy-group-key");
    let legacy_secret = format!("{legacy_name}-secret");
    let legacy = env
        .state
        .with_management_lock(|| {
            let entry = api_keys::add(
                &legacy_name,
                Some(&legacy_secret),
                vec!["workbuddy".to_string()],
                vec![],
            )?;
            api_keys::update_with_route(
                &entry.id,
                None,
                None,
                None,
                None,
                Some(api_keys::RouteMode::AccountGroup),
                Some(Some(group.clone())),
            )
        })
        .expect("应能写入历史组 Key 夹具");

    let (status, patched_legacy) = request(
        &env,
        Method::PATCH,
        &format!("/api/keys/{}", legacy.id),
        json!({ "allowedModels": ["it04-legacy-model"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let legacy_after = patched_legacy["data"]["keys"]
        .as_array()
        .expect("Key 列表应为数组")
        .iter()
        .find(|entry| entry["id"] == legacy.id)
        .expect("历史组 Key 应存在");
    assert_eq!(legacy_after["routeMode"], "account_group");
    assert_eq!(legacy_after["accountGroupId"], group);
    assert_eq!(legacy_after["allowedProviders"], json!(["workbuddy"]));
    assert_eq!(legacy_after["allowedModels"], json!(["it04-legacy-model"]));

    let (status, patched) = request(
        &env,
        Method::PATCH,
        &format!("/api/keys/{group_key_id}"),
        json!({ "name": unique("it04-renamed") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let unchanged = patched["data"]["keys"]
        .as_array()
        .expect("Key 列表应为数组")
        .iter()
        .find(|entry| entry["id"] == group_key_id)
        .expect("组 Key 应存在");
    assert_eq!(unchanged["routeMode"], "account_group");
    assert_eq!(unchanged["accountGroupId"], group);

    let (status, switched) = request(
        &env,
        Method::PATCH,
        &format!("/api/keys/{group_key_id}"),
        json!({ "routeMode": "default_pool", "accountGroupId": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let switched_key = switched["data"]["keys"]
        .as_array()
        .expect("Key 列表应为数组")
        .iter()
        .find(|entry| entry["id"] == group_key_id)
        .expect("切换后的 Key 应存在");
    assert_eq!(switched_key["routeMode"], "default_pool");
    assert_eq!(switched_key["accountGroupId"], Value::Null);

    delete_key(&env, &group_key_id).await;
    delete_key(&env, &legacy.id).await;
    delete_key(&env, &default_id).await;
    delete_group(&env, &group).await;
}

#[tokio::test]
async fn it04_rejects_missing_group_and_reenables_rebound_legacy_key() {
    let env = lock();
    let missing_group = unique("it04-missing-group");
    let (status, missing) = request(
        &env,
        Method::POST,
        "/api/keys",
        json!({
            "name": unique("it04-missing-key"),
            "key": unique("it04-missing-secret"),
            "routeMode": "account_group",
            "accountGroupId": missing_group,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(missing["error"]
        .as_str()
        .unwrap_or_default()
        .contains("账号组不存在"));
    assert!(!missing.to_string().contains("account_group_unavailable"));

    let old_group = create_group(&env, "it04-old-group").await;
    let new_group = create_group(&env, "it04-new-group").await;
    let name = unique("it04-legacy-disabled");
    let secret = format!("{name}-secret");
    let legacy = env
        .state
        .with_management_lock(|| {
            let entry = api_keys::add_with_route(
                &name,
                Some(&secret),
                vec![],
                vec![],
                api_keys::RouteMode::AccountGroup,
                Some(old_group.clone()),
            )?;
            api_keys::update_with_route(&entry.id, None, Some(false), None, None, None, None)
        })
        .expect("应能构造停用且仍绑定旧组的 Key");
    let management_key_id = api_keys::list()
        .into_iter()
        .find(|entry| entry.key == env.management_key)
        .expect("共享管理 Key 应存在")
        .id;

    let (status, _) = request(
        &env,
        Method::PATCH,
        &format!("/api/keys/{}", legacy.id),
        json!({ "routeMode": "account_group", "accountGroupId": new_group }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rebound = key_by_id(&env, &legacy.id).await;
    assert_eq!(rebound["enabled"], false);
    assert_eq!(rebound["accountGroupId"], new_group);
    assert_eq!(rebound["key"], secret);
    assert!(!rebound.to_string().contains("account_group_unavailable"));

    let (status, _) = request(
        &env,
        Method::PATCH,
        &format!("/api/keys/{}", legacy.id),
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let enabled = key_by_id(&env, &legacy.id).await;
    assert_eq!(enabled["enabled"], true);
    assert_eq!(enabled["accountGroupId"], new_group);
    assert_eq!(enabled["key"], secret);
    assert_eq!(key_by_id(&env, &management_key_id).await["enabled"], true);

    delete_key(&env, &legacy.id).await;
    delete_group(&env, &old_group).await;
    delete_group(&env, &new_group).await;
}

#[tokio::test]
async fn it05_protects_referenced_groups_and_allows_delete_after_unbind() {
    let env = lock();
    let (status, empty) = request(
        &env,
        Method::POST,
        "/api/account-groups",
        json!({ "name": unique("it05-empty") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    delete_group(&env, &group_id(&empty)).await;

    let (status, created) = request(
        &env,
        Method::POST,
        "/api/account-groups",
        json!({ "name": unique("it05-bound") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let group = group_id(&created);
    let key_name = unique("it05-key");
    let (status, key) = request(
        &env,
        Method::POST,
        "/api/keys",
        json!({
            "name": key_name,
            "key": format!("{key_name}-secret"),
            "routeMode": "account_group",
            "accountGroupId": group,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let key_id = created_key_id(&key);

    let (status, conflict) = request(
        &env,
        Method::DELETE,
        &format!("/api/account-groups/{group}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(conflict["error"]
        .as_str()
        .unwrap_or_default()
        .contains(&key_name));
    assert_eq!(conflict["usedBy"][0]["id"], key_id);
    assert_eq!(conflict["usedBy"][0]["name"], key_name);
    assert!(conflict["usedBy"][0].get("key").is_none());
    assert!(!conflict.to_string().contains(&format!("{key_name}-secret")));

    let (status, _) = request(
        &env,
        Method::PATCH,
        &format!("/api/keys/{key_id}"),
        json!({ "routeMode": "default_pool", "accountGroupId": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    delete_group(&env, &group).await;
    delete_key(&env, &key_id).await;
}
