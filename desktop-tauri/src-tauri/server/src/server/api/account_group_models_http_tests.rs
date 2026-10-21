//! HTTP model-directory tests for account-group Key scopes.

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use super::account_groups_http_support::{request, request_with_key, unique};
use crate::server::core::custom_providers;

fn create_provider(name: &str, model: &str) -> String {
    let provider = custom_providers::create(&json!({
        "name": name,
        "protocol": "chat_completions",
        "baseUrl": format!("https://{name}.invalid/v1"),
    }))
    .expect("自定义 provider 应能创建");
    let id = provider["id"]
        .as_str()
        .expect("自定义 provider 应返回 id")
        .to_string();
    custom_providers::set_models(
        &id,
        json!([{ "id": model, "enabled": true, "reasoning": "" }]),
        json!([]),
    )
    .expect("自定义 provider 模型应能保存");
    id
}

fn model_ids(response: &Value) -> Vec<String> {
    response["data"]
        .as_array()
        .expect("模型目录应有 data 数组")
        .iter()
        .filter_map(|item| item["id"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn it06_group_key_models_only_include_the_group_directory() {
    let env = crate::server::core::account_groups::test_bootstrap::lock();
    let first_model = unique("it06-first-model");
    let second_model = unique("it06-second-model");
    let first = create_provider("it06-first", &first_model);
    let second = create_provider("it06-second", &second_model);
    let account = env
        .state
        .store()
        .add_custom_account(
            &first,
            &json!({ "apiKey": unique("it06-api-key"), "noAuth": false }),
            Some("it06-group-account"),
        )
        .expect("组内自定义账号应能写入")
        .account;
    let account_id = account["id"].as_str().expect("账号应返回 id").to_string();

    let runtime = tokio::runtime::Runtime::new().expect("测试 runtime 应能创建");
    runtime.block_on(async {
        let (status, created) = request(
            &env,
            Method::POST,
            "/api/account-groups",
            json!({ "name": unique("it06-group") }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let group = created["data"]["group"]["id"]
            .as_str()
            .expect("应返回组 id")
            .to_string();
        let (status, _) = request(
            &env,
            Method::PUT,
            &format!("/api/account-groups/{group}/members"),
            json!({ "members": [{ "type": "account", "id": account_id }] }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let name = unique("it06-key");
        let secret = format!("{name}-secret");
        let (status, key) = request(
            &env,
            Method::POST,
            "/api/keys",
            json!({
                "name": name,
                "key": secret,
                "routeMode": "account_group",
                "accountGroupId": group,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let key_id = key["data"]["created"]["id"]
            .as_str()
            .expect("应返回 Key id")
            .to_string();

        let (status, models) =
            request_with_key(&env, Method::GET, "/v1/models", Value::Null, &secret).await;
        assert_eq!(status, StatusCode::OK);
        let ids = model_ids(&models);
        assert!(ids.contains(&first_model));
        assert!(!ids.contains(&second_model));

        let (status, _) = request(
            &env,
            Method::DELETE,
            &format!("/api/keys/{key_id}"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = request(
            &env,
            Method::DELETE,
            &format!("/api/account-groups/{group}"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    });

    custom_providers::remove(&first, env.state.store()).expect("测试结束应清理组内 provider");
    custom_providers::remove(&second, env.state.store()).expect("测试结束应清理组外 provider");
    let _ = env.state.store().remove_account(&account_id);
}
