use serde_json::{json, Map, Value};

use crate::server::core::api_keys::{self, RouteMode};
use crate::server::core::key_scope::KeyScope;

use super::test_bootstrap;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        crate::server::logging::now_ms()
    )
}

#[test]
fn old_key_shapes_and_unknown_route_mode_remain_default_pool() {
    let mut legacy = Map::new();
    legacy.insert("apiKey".to_string(), json!("legacy-key-123456"));
    let entries = api_keys::entries_from(&legacy);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].route_mode, RouteMode::DefaultPool);
    assert!(entries[0].account_group_id.is_none());

    let mut array = Map::new();
    array.insert(
        api_keys::KEY_API_KEYS.to_string(),
        json!([{
            "id": "old",
            "name": "old",
            "key": "old-key-123456",
            "enabled": true,
            "allowedProviders": ["workbuddy"],
            "allowedModels": ["model-a"],
            "routeMode": "future-mode"
        }]),
    );
    let entry = api_keys::entries_from(&array).pop().unwrap();
    assert_eq!(entry.route_mode, RouteMode::DefaultPool);
    assert_eq!(entry.allowed_providers, ["workbuddy"]);
    assert_eq!(entry.allowed_models, ["model-a"]);
}

#[test]
fn account_group_route_rejects_only_incoming_provider_restrictions() {
    let env = test_bootstrap::lock();
    let service = env.state.account_groups();
    let group = service.create(&unique("Key group")).unwrap();

    let invalid_key = unique("invalid");
    let error = api_keys::add_with_route(
        "invalid group key",
        Some(&invalid_key),
        vec!["workbuddy".to_string()],
        vec![],
        RouteMode::AccountGroup,
        Some(group.id.clone()),
    )
    .expect_err("组模式新建带 provider 白名单必须失败");
    assert!(error.contains("allowedProviders"));
    assert!(!api_keys::list()
        .iter()
        .any(|entry| entry.key == invalid_key));

    let key = unique("valid");
    let entry = api_keys::add_with_route(
        "valid group key",
        Some(&key),
        vec![],
        vec!["model-a".to_string()],
        RouteMode::AccountGroup,
        Some(group.id.clone()),
    )
    .unwrap();
    let scope = KeyScope::from_entry(&entry);
    assert!(scope.is_account_group());
    assert_eq!(scope.account_group_id(), Some(group.id.as_str()));

    let residual_key = unique("residual");
    let residual = api_keys::add(
        "residual default key",
        Some(&residual_key),
        vec!["workbuddy".to_string()],
        vec![],
    )
    .unwrap();
    let switched = api_keys::update_with_route(
        &residual.id,
        None,
        None,
        None,
        None,
        Some(RouteMode::AccountGroup),
        Some(Some(group.id.clone())),
    )
    .unwrap();
    assert_eq!(switched.route_mode, RouteMode::AccountGroup);
    assert_eq!(switched.allowed_providers, ["workbuddy"]);

    let update_error = api_keys::update_with_route(
        &entry.id,
        None,
        None,
        Some(vec!["workbuddy".to_string()]),
        None,
        None,
        None,
    )
    .expect_err("组模式 PATCH 传入非空 provider 白名单必须失败");
    assert!(update_error.contains("allowedProviders"));
    let unchanged = api_keys::list()
        .into_iter()
        .find(|candidate| candidate.id == entry.id)
        .unwrap();
    assert!(unchanged.allowed_providers.is_empty());

    api_keys::remove(&entry.id).unwrap();
    api_keys::remove(&residual.id).unwrap();
    service.delete(&group.id).unwrap();
}

#[test]
fn key_scope_reads_route_mode_and_group_id_without_exposing_extra_state() {
    let value = Value::Object(
        [
            ("id", json!("k1")),
            ("name", json!("group")),
            ("key", json!("key-123456")),
            ("enabled", json!(true)),
            ("routeMode", json!("account_group")),
            ("accountGroupId", json!("g1")),
            ("allowedProviders", json!([])),
            ("allowedModels", json!([])),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect(),
    );
    let entry = api_keys::entries_from(&Map::from_iter([(
        api_keys::KEY_API_KEYS.to_string(),
        Value::Array(vec![value]),
    )]))
    .pop()
    .unwrap();
    let scope = KeyScope::from_entry(&entry);
    assert_eq!(scope.account_group_id(), Some("g1"));
    assert!(scope.is_account_group());
    assert!(!scope.restricts_providers());
    assert!(!scope.restricts_models());
}
