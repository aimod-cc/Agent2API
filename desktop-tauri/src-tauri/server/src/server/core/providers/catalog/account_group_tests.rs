//! Model-directory and group-model resolution contract tests.

use serde_json::{json, Value};

use crate::server::api::pipeline;
use crate::server::config;
use crate::server::core::account_groups::AccountGroupScope;
use crate::server::core::custom_providers;
use crate::server::core::key_scope::KeyScope;
use crate::server::core::models::model_id;

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
        .map(model_id)
        .collect()
}

struct DefaultModelGuard {
    previous: Option<String>,
}

impl DefaultModelGuard {
    fn set(model: &str) -> Self {
        let previous = std::env::var("WORKBUDDY_DEFAULT_MODEL").ok();
        std::env::set_var("WORKBUDDY_DEFAULT_MODEL", model);
        config::reload();
        Self { previous }
    }
}

impl Drop for DefaultModelGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(model) => std::env::set_var("WORKBUDDY_DEFAULT_MODEL", model),
            None => std::env::remove_var("WORKBUDDY_DEFAULT_MODEL"),
        }
        config::reload();
    }
}

#[test]
fn group_directory_and_resolution_share_ordered_custom_provider_scope() {
    let env = super::super::super::account_groups::test_bootstrap::lock();
    let first_model = "c07-custom-first";
    let second_model = "c07-custom-second";
    let first_provider = create_provider("c07-first", first_model);
    let second_provider = create_provider("c07-second", second_model);
    let scope = AccountGroupScope::from_canonical(
        "g-c07-order",
        vec![
            json!({
                "id": "c07-second-account",
                "provider": second_provider,
                "enabled": true,
            }),
            json!({
                "id": "c07-first-account",
                "provider": first_provider,
                "enabled": true,
            }),
        ],
    );
    let directory =
        super::group_models_response(env.state.store(), &scope, Some(&KeyScope::default()));
    let expected = vec![second_model.to_string(), first_model.to_string()];
    assert_eq!(model_ids(&directory), expected);
    assert!(env
        .state
        .store()
        .accounts_for_provider(&first_provider)
        .is_empty());

    let _default_model = DefaultModelGuard::set(first_model);
    let mut unnamed_payload = json!({ "messages": [] });
    let unnamed_model = pipeline::resolve_model(
        &env.state,
        &mut unnamed_payload,
        Some(&KeyScope::default()),
        Some(&scope),
    )
    .expect("组目录非空时未点名请求应能注入模型");
    assert_eq!(unnamed_model, first_model);
    assert_eq!(unnamed_payload["model"], first_model);

    for model in &expected {
        let mut payload = json!({ "model": model });
        let resolved = pipeline::resolve_model(
            &env.state,
            &mut payload,
            Some(&KeyScope::default()),
            Some(&scope),
        )
        .expect("目录中的模型应被组解析接受");
        assert_eq!(resolved, *model);
    }
    let mut outside_payload = json!({ "model": "c07-outside-model" });
    let outside = pipeline::resolve_model(
        &env.state,
        &mut outside_payload,
        Some(&KeyScope::default()),
        Some(&scope),
    )
    .expect_err("组目录外模型不应被接受");
    assert_eq!(outside.status_code, 404);
    assert_eq!(outside.code.as_deref(), Some("model_not_found"));

    custom_providers::remove(&first_provider, env.state.store())
        .expect("测试结束应清理第一个 provider");
    custom_providers::remove(&second_provider, env.state.store())
        .expect("测试结束应清理第二个 provider");
}

#[test]
fn default_pool_catalog_with_unrestricted_key_keeps_the_same_response() {
    let env = super::super::super::account_groups::test_bootstrap::lock();
    let unrestricted = super::models_response(env.state.store(), None);
    let explicit_default_pool =
        super::models_response(env.state.store(), Some(&KeyScope::default()));
    assert_eq!(unrestricted, explicit_default_pool);
}

#[test]
fn empty_group_directory_is_empty() {
    let env = super::super::super::account_groups::test_bootstrap::lock();
    let scope = AccountGroupScope::from_canonical("g-c07-empty", vec![]);

    let directory =
        super::group_models_response(env.state.store(), &scope, Some(&KeyScope::default()));

    assert!(model_ids(&directory).is_empty());

    let mut payload = json!({});
    let model = pipeline::resolve_model(
        &env.state,
        &mut payload,
        Some(&KeyScope::default()),
        Some(&scope),
    )
    .expect("空组未点名请求不应产生解析错误");
    assert!(model.is_empty());
    assert!(payload.get("error").is_none());
    assert!(payload.get("model").is_none());
}

#[test]
fn unnamed_request_falls_back_to_first_group_model() {
    let env = super::super::super::account_groups::test_bootstrap::lock();
    let first_model = "c07-fallback-first";
    let second_model = "c07-fallback-second";
    let first_provider = create_provider("c07-fallback-first-provider", first_model);
    let second_provider = create_provider("c07-fallback-second-provider", second_model);
    let scope = AccountGroupScope::from_canonical(
        "g-c07-fallback",
        vec![
            json!({ "id": "c07-fallback-first-account", "provider": first_provider, "enabled": true }),
            json!({ "id": "c07-fallback-second-account", "provider": second_provider, "enabled": true }),
        ],
    );
    let _default_model = DefaultModelGuard::set("c07-model-outside-group");
    let mut payload = json!({ "messages": [] });

    let model = pipeline::resolve_model(
        &env.state,
        &mut payload,
        Some(&KeyScope::default()),
        Some(&scope),
    )
    .expect("默认模型不在目录时应回退到目录首条模型");

    assert_eq!(model, first_model);
    assert_eq!(payload["model"], first_model);
    custom_providers::remove(&first_provider, env.state.store())
        .expect("测试结束应清理第一个 provider");
    custom_providers::remove(&second_provider, env.state.store())
        .expect("测试结束应清理第二个 provider");
}
