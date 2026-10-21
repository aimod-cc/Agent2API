//! Shared mechanical helpers for account-group HTTP contract tests.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use tower::ServiceExt;

pub(crate) use super::super::core::account_groups::test_bootstrap::{lock, TestEnv};
pub(crate) use super::super::core::account_store::AccountStore;
use super::super::http;

pub(crate) fn unique(label: &str) -> String {
    let ticks = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应可用")
        .as_nanos();
    format!("agent2api-{label}-{}-{ticks}", std::process::id())
}

pub(crate) async fn request(
    env: &TestEnv,
    method: Method,
    path: &str,
    payload: Value,
) -> (StatusCode, Value) {
    request_with_key(env, method, path, payload, &env.management_key).await
}

pub(crate) async fn request_with_key(
    env: &TestEnv,
    method: Method,
    path: &str,
    payload: Value,
    key: &str,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(payload.to_string()))
        .expect("测试请求应能构造");
    let response = Router::from(http::router(env.state.clone()))
        .oneshot(request)
        .await
        .expect("router 错误类型应为 Infallible");
    let status = response.status();
    let body = to_bytes(response.into_body(), http::MAX_BODY_SIZE)
        .await
        .expect("测试响应体应能读取");
    let body = serde_json::from_slice(&body).expect("管理 API 应返回 JSON");
    (status, body)
}

pub(crate) fn group_id(body: &Value) -> String {
    body["data"]["group"]["id"]
        .as_str()
        .expect("响应应包含账号组 id")
        .to_string()
}

pub(crate) fn created_key_id(body: &Value) -> String {
    body["data"]["created"]["id"]
        .as_str()
        .expect("响应应包含 Key id")
        .to_string()
}

pub(crate) fn add_account(store: &AccountStore, label: &str) -> String {
    store
        .add_account(
            &serde_json::json!({
                "auth": { "accessToken": format!("token-{label}") },
                "account": { "uid": format!("uid-{label}") },
            }),
            Some(label),
            Some("workbuddy"),
        )
        .expect("测试账号应能写入")
        .get("id")
        .and_then(Value::as_str)
        .expect("测试账号应包含 id")
        .to_string()
}

pub(crate) async fn create_group(env: &TestEnv, label: &str) -> String {
    let (status, body) = request(
        env,
        Method::POST,
        "/api/account-groups",
        serde_json::json!({ "name": unique(label) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    group_id(&body)
}

pub(crate) async fn create_default_key(env: &TestEnv, label: &str) -> String {
    let name = unique(label);
    let (status, body) = request(
        env,
        Method::POST,
        "/api/keys",
        serde_json::json!({
            "name": name,
            "key": format!("{name}-secret"),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    created_key_id(&body)
}

pub(crate) async fn key_by_id(env: &TestEnv, id: &str) -> Value {
    let (status, body) = request(env, Method::GET, "/api/keys", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    body["data"]["keys"]
        .as_array()
        .expect("Key 列表应为数组")
        .iter()
        .find(|entry| entry["id"] == id)
        .cloned()
        .expect("Key 应存在")
}

pub(crate) async fn delete_key(env: &TestEnv, id: &str) {
    let (status, _) = request(env, Method::DELETE, &format!("/api/keys/{id}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
}

pub(crate) async fn delete_group(env: &TestEnv, id: &str) {
    let (status, _) = request(
        env,
        Method::DELETE,
        &format!("/api/account-groups/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
