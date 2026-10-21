//! HTTP contract tests for account-group management and Key binding.

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use super::account_groups_http_support::{
    add_account, created_key_id, delete_group, delete_key, group_id, lock, request, unique,
};

#[tokio::test]
async fn it01_creates_lists_and_rejects_duplicate_group_names() {
    let env = lock();
    let name = unique("it01-group");
    let (status, created) = request(
        &env,
        Method::POST,
        "/api/account-groups",
        json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = group_id(&created);
    assert_eq!(created["data"]["group"]["name"], name);
    assert!(created["data"]["group"]["preview"]["accounts"].is_array());
    assert!(!created.to_string().contains("accessToken"));
    assert!(!created.to_string().contains("refreshToken"));

    let (status, listed) = request(&env, Method::GET, "/api/account-groups", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed["data"]["groups"]
        .as_array()
        .expect("组列表应为数组")
        .iter()
        .any(|group| group["id"] == id));

    let (status, duplicate) = request(
        &env,
        Method::POST,
        "/api/account-groups",
        json!({ "name": format!("  {}  ", name.to_uppercase()) }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(duplicate["success"], false);

    let key_value = unique("it01-key");
    let (status, key) = request(
        &env,
        Method::POST,
        "/api/keys",
        json!({ "name": key_value, "key": format!("{key_value}-secret") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let key_id = created_key_id(&key);
    let (status, keys) = request(&env, Method::GET, "/api/keys", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let listed_key = keys["data"]["keys"]
        .as_array()
        .expect("Key 列表应为数组")
        .iter()
        .find(|entry| entry["id"] == key_id)
        .expect("新 Key 应能被管理接口查询");
    assert_eq!(listed_key["routeMode"], "default_pool");
    assert_eq!(listed_key["accountGroupId"], Value::Null);

    delete_key(&env, &key_id).await;
    delete_group(&env, &id).await;
}

#[tokio::test]
async fn it02_renaming_group_keeps_key_binding_by_id() {
    let env = lock();
    let name = unique("it02-group");
    let (status, created) = request(
        &env,
        Method::POST,
        "/api/account-groups",
        json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let group = group_id(&created);
    let key_name = unique("it02-key");
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

    let renamed = unique("it02-renamed");
    let (status, response) = request(
        &env,
        Method::PATCH,
        &format!("/api/account-groups/{group}"),
        json!({ "name": renamed }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["data"]["group"]["id"], group);
    assert_eq!(response["data"]["group"]["name"], renamed);

    let (status, keys) = request(&env, Method::GET, "/api/keys", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let key = keys["data"]["keys"]
        .as_array()
        .expect("Key 列表应为数组")
        .iter()
        .find(|entry| entry["id"] == key_id)
        .expect("组 Key 应存在");
    assert_eq!(key["routeMode"], "account_group");
    assert_eq!(key["accountGroupId"], group);

    delete_key(&env, &key_id).await;
    delete_group(&env, &group).await;
}

#[tokio::test]
async fn it03_replaces_members_atomically_and_preserves_order() {
    let env = lock();
    let (status, created) = request(
        &env,
        Method::POST,
        "/api/account-groups",
        json!({ "name": unique("it03-group") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let group = group_id(&created);
    let account = add_account(&env.state.store(), &unique("it03-account"));

    let (status, _) = request(
        &env,
        Method::PUT,
        &format!("/api/account-groups/{group}/members"),
        json!({ "members": [{ "type": "provider", "id": "not-a-provider" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = request(
        &env,
        Method::PUT,
        &format!("/api/account-groups/{group}/members"),
        json!({
            "members": [
                { "type": "account", "id": account },
                { "type": "account", "id": account }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = request(
        &env,
        Method::PUT,
        &format!("/api/account-groups/{group}/members"),
        json!({ "members": [{ "type": "account", "id": "missing-account" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = request(
        &env,
        Method::PUT,
        &format!("/api/account-groups/{group}/members"),
        json!({
            "members": [
                { "type": "account", "id": account },
                { "type": "provider", "id": "workbuddy" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let failing_account = add_account(&env.state.store(), &unique("it03-failing"));
    env.state
        .store()
        .remove_account(&failing_account)
        .expect("失败替换夹具应能删除");
    let (status, _) = request(
        &env,
        Method::PUT,
        &format!("/api/account-groups/{group}/members"),
        json!({ "members": [{ "type": "account", "id": failing_account }] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, listed) = request(&env, Method::GET, "/api/account-groups", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let group = listed["data"]["groups"]
        .as_array()
        .expect("组列表应为数组")
        .iter()
        .find(|entry| entry["id"] == group)
        .expect("组应能查询");
    assert_eq!(group["members"][0]["type"], "account");
    assert_eq!(group["members"][0]["id"], account);
    assert_eq!(group["members"][1]["type"], "provider");
    assert_eq!(group["members"][1]["position"], 1);
    assert!(!group.to_string().contains("accessToken"));

    delete_group(&env, group["id"].as_str().expect("组 id 应存在")).await;
    env.state
        .store()
        .remove_account(&account)
        .expect("测试账号应能清理");
}

#[tokio::test]
async fn it08_renders_deleted_explicit_accounts_without_dropping_provider_preview() {
    let env = lock();
    let first = add_account(&env.state.store(), &unique("it08-explicit"));
    let retained = add_account(&env.state.store(), &unique("it08-provider"));
    let (status, created) = request(
        &env,
        Method::POST,
        "/api/account-groups",
        json!({ "name": unique("it08-group") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let group = group_id(&created);
    let (status, _) = request(
        &env,
        Method::PUT,
        &format!("/api/account-groups/{group}/members"),
        json!({
            "members": [
                { "type": "account", "id": first },
                { "type": "provider", "id": "workbuddy" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    env.state
        .store()
        .remove_account(&first)
        .expect("显式账号应能删除");
    let (status, listed) = request(&env, Method::GET, "/api/account-groups", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let group = listed["data"]["groups"]
        .as_array()
        .expect("组列表应为数组")
        .iter()
        .find(|entry| entry["id"] == group)
        .expect("组应仍存在");
    assert_eq!(group["members"].as_array().expect("成员应为数组").len(), 1);
    assert_eq!(group["members"][0]["type"], "provider");
    assert!(group["preview"]["accounts"]
        .as_array()
        .expect("预览账号应为数组")
        .iter()
        .any(|account| account["id"] == retained));
    assert!(group["preview"]["providers"]
        .as_array()
        .expect("预览 provider 应为数组")
        .iter()
        .any(|provider| provider["id"] == "workbuddy"));

    delete_group(&env, group["id"].as_str().expect("组 id 应存在")).await;
    env.state
        .store()
        .remove_account(&retained)
        .expect("测试账号应能清理");
}
