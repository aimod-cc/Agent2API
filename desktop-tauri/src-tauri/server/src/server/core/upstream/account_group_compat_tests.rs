//! Compatibility tests for account-group routing scope propagation.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::server::config::RouteStrategy;
use crate::server::core::api_keys::{self, RouteMode};
use crate::server::core::key_scope::KeyScope;
use crate::server::core::providers::readiness;
use crate::server::core::routing::{self, CooldownKeys, PlanMeta};

fn account(id: &str, priority: i64) -> Value {
    json!({
        "id": id,
        "name": id,
        "provider": "workbuddy",
        "priority": priority,
        "addedAt": priority,
        "enabled": true,
        "hasCredentials": true,
    })
}

struct ReadinessCleanup;

impl Drop for ReadinessCleanup {
    fn drop(&mut self) {
        readiness::clear_for_tests();
    }
}

#[test]
fn legacy_key_and_default_pool_scope_keep_existing_semantics() {
    let mut raw = serde_json::Map::new();
    raw.insert(
        api_keys::KEY_API_KEYS.to_string(),
        json!([{
            "id": "compat-key",
            "name": "compat-key",
            "key": "compat-secret",
            "enabled": true,
            "allowedProviders": ["workbuddy"],
            "allowedModels": ["deepseek-v4.1-flash"],
        }]),
    );
    let entries = api_keys::entries_from(&raw);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].route_mode, RouteMode::DefaultPool);
    let scope = KeyScope::from_entry(&entries[0]);
    assert!(scope.allows_provider("WORKBUDDY"));
    assert!(scope.allows_model("DEEPSEEK-V4.1-FLASH"));
    assert!(!scope.allows_provider("catpaw"));
    assert!(!scope.allows_model("other-model"));
}

#[test]
fn default_pool_order_still_uses_global_priority() {
    let accounts = vec![account("priority-two", 2), account("priority-one", 1)];
    let keys = CooldownKeys::new("deepseek-v4.1-flash");
    let empty = HashMap::new();
    let open = HashSet::new();
    let limiter_blocked = HashSet::new();
    let meta = PlanMeta {
        strategy: RouteStrategy::Order,
        sunk: &empty,
        resting: &empty,
        open: &open,
        limiter_blocked: &limiter_blocked,
        cursor: None,
        provider_cursors: None,
        sticky_account: None,
    };
    let plan = routing::plan_route(&accounts, &keys, &HashMap::new(), &[], 1_000, &meta);
    let ordered: Vec<_> = plan.ordered.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(ordered, ["priority-one", "priority-two"]);
}

#[test]
fn default_pool_readiness_returns_original_candidates_when_all_are_blocked() {
    let _readiness = readiness::lock_for_tests();
    let _cleanup = ReadinessCleanup;
    readiness::clear_for_tests();

    let candidates = vec![account("default-first", 1), account("default-second", 2)];
    readiness::hold("workbuddy", "测试用例临时挡住默认池");
    let filtered = super::rotate::readiness_candidates_or_original_pool(&candidates);

    assert_eq!(filtered, candidates);
    assert!(filtered.iter().all(|entry| {
        entry.get("provider").and_then(Value::as_str) == Some("workbuddy")
    }));
}

#[test]
fn unrestricted_default_pool_model_directory_is_unchanged_by_explicit_scope() {
    let env = super::super::account_groups::test_bootstrap::lock();
    let unrestricted =
        crate::server::core::providers::catalog::models_response(env.state.store(), None);
    let explicit_default_pool = crate::server::core::providers::catalog::models_response(
        env.state.store(),
        Some(&KeyScope::default()),
    );
    assert_eq!(unrestricted, explicit_default_pool);
}
