//! Account-group 404/503 boundaries that are distinct from first-choice routing.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{ForwardRequest, UpstreamService};
use crate::server::api::pipeline;
use crate::server::core::account_groups::AccountGroupScope;
use crate::server::core::key_scope::KeyScope;
use crate::server::core::providers::{catalog, readiness};
use crate::server::core::upstream::usage::RequestTelemetry;

const MODEL: &str = "deepseek-v4.1-flash";

fn request(scope: AccountGroupScope) -> ForwardRequest {
    ForwardRequest {
        body: json!({ "model": MODEL, "messages": [] }),
        stream: false,
        dedupe_key: String::new(),
        client_headers: Default::default(),
        telemetry: Arc::new(RequestTelemetry::new()),
        allowed_providers: None,
        account_group_scope: Some(scope),
        pinned_account: None,
        ignore_model_gate: false,
        proxy_override: None,
    }
}

fn account(id: &str, provider: &str, enabled: bool) -> Value {
    json!({
        "id": id,
        "name": id,
        "provider": provider,
        "enabled": enabled,
        "hasCredentials": true,
        "limiters": [],
    })
}

fn real_provider() -> String {
    catalog::forwarding_providers(MODEL)
        .into_iter()
        .next()
        .expect("测试模型至少应有一个可转发 provider")
}

#[test]
fn named_model_outside_nonempty_group_is_pipeline_404() {
    let env = super::super::account_groups::test_bootstrap::lock();
    let scope = AccountGroupScope::from_canonical(
        "routing-error-outside",
        vec![account("outside-member", &real_provider(), true)],
    );
    let mut payload = json!({ "model": "c05-outside-catalog" });

    let error = pipeline::resolve_model(
        &env.state,
        &mut payload,
        Some(&KeyScope::default()),
        Some(&scope),
    )
    .expect_err("组目录外模型应被拒绝");

    assert_eq!(error.status_code, 404);
    assert_eq!(error.code.as_deref(), Some("model_not_found"));
}

#[test]
fn named_model_in_empty_group_is_pipeline_404() {
    let env = super::super::account_groups::test_bootstrap::lock();
    let scope = AccountGroupScope::from_canonical("routing-error-empty-pipeline", vec![]);
    let mut payload = json!({ "model": MODEL });

    let error = pipeline::resolve_model(
        &env.state,
        &mut payload,
        Some(&KeyScope::default()),
        Some(&scope),
    )
    .expect_err("空组点名模型应在完整管线中返回模型不存在");

    assert_eq!(error.status_code, 404);
    assert_eq!(error.code.as_deref(), Some("model_not_found"));
}

#[tokio::test]
async fn empty_group_at_router_returns_group_unavailable_503() {
    let scope = AccountGroupScope::from_canonical("routing-error-empty-router", vec![]);
    let store = crate::server::core::account_store::AccountStore::with_db(None);
    let service = UpstreamService::new(
        store.clone(),
        crate::server::core::auth::AuthService::for_store(store),
    );

    let error = match service.forward(request(scope)).await {
        Err(error) => error,
        Ok(_) => panic!("空组不应回落到 default_pool"),
    };

    assert_eq!(error.status_code, 503);
    assert_eq!(
        error.code.as_deref(),
        Some("account_group_no_available_account")
    );
}

#[tokio::test]
async fn disabled_group_candidates_return_group_unavailable_503() {
    let scope = AccountGroupScope::from_canonical(
        "routing-error-disabled",
        vec![account("disabled-member", &real_provider(), false)],
    );
    let store = crate::server::core::account_store::AccountStore::with_db(None);
    let service = UpstreamService::new(
        store.clone(),
        crate::server::core::auth::AuthService::for_store(store),
    );

    let error = match service.forward(request(scope)).await {
        Err(error) => error,
        Ok(_) => panic!("全部禁用时不应请求上游"),
    };

    assert_eq!(error.status_code, 503);
    assert_eq!(
        error.code.as_deref(),
        Some("account_group_no_available_account")
    );
}

#[test]
fn readiness_gate_keeps_the_original_group_candidates() {
    let _readiness = readiness::lock_for_tests();
    readiness::clear_for_tests();
    let provider = real_provider();
    assert!(readiness::hold(&provider, "C-05.4 fixture"));
    let scope = AccountGroupScope::from_canonical(
        "routing-error-readiness",
        vec![account("readiness-member", &provider, true)],
    );
    let keys = crate::server::core::routing::CooldownKeys::new(MODEL);

    let candidates = super::account_group_pool::accounts_in_group(&scope, &keys, None);
    readiness::clear_for_tests();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["id"], "readiness-member");
}
