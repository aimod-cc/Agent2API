//! Contract tests for request-local account-group routing.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::connections::{ConnectionGuard, Connections};
use super::{account_group_pool, UpstreamService};
use crate::server::config::RouteStrategy;
use crate::server::core::account_groups::AccountGroupScope;
use crate::server::core::account_store::AccountStore;
use crate::server::core::auth::AuthService;
use crate::server::core::providers::{catalog, readiness};
use crate::server::core::routing;

fn account(id: &str, provider: &str, priority: i64) -> Value {
    json!({
        "id": id,
        "name": id,
        "provider": provider,
        "priority": priority,
        "addedAt": priority,
        "enabled": true,
        "hasCredentials": true,
    })
}

fn test_service() -> UpstreamService {
    let store = AccountStore::with_db(None);
    UpstreamService::new(store.clone(), AuthService::for_store(store))
}

fn ids(accounts: &[Value]) -> Vec<&str> {
    accounts
        .iter()
        .filter_map(|account| account.get("id").and_then(Value::as_str))
        .collect()
}

fn plan_meta<'a>(
    strategy: RouteStrategy,
    cursor: Option<usize>,
    provider_cursors: Option<&'a HashMap<String, usize>>,
    sticky_account: Option<&'a str>,
    sunk: &'a HashMap<String, i64>,
    resting: &'a HashMap<String, i64>,
    open: &'a HashSet<String>,
    limiter_blocked: &'a HashSet<String>,
) -> routing::PlanMeta<'a> {
    routing::PlanMeta {
        strategy,
        sunk,
        resting,
        open,
        limiter_blocked,
        cursor,
        provider_cursors,
        sticky_account,
    }
}

#[test]
fn group_pool_intersects_forwarding_providers_in_canonical_order() {
    let _readiness = readiness::lock_for_tests();
    readiness::clear_for_tests();

    let model = "deepseek-v4.1-flash";
    let provider = catalog::forwarding_providers(model)
        .first()
        .expect("测试模型至少应有一个可转发 provider")
        .clone();
    let scope = AccountGroupScope::from_canonical(
        "group-main",
        vec![
            account("member-2", &provider, 20),
            account("outside", "provider-outside-group", 1),
            account("member-1", &provider, 10),
        ],
    );
    let keys = routing::CooldownKeys::new(model);

    let candidates = account_group_pool::accounts_in_group(&scope, &keys, None);

    assert_eq!(ids(&candidates), ["member-2", "member-1"]);
    assert!(!ids(&candidates).contains(&"outside"));
}

#[test]
fn group_pool_keeps_original_model_intersection_when_readiness_blocks_everything() {
    let _readiness = readiness::lock_for_tests();
    readiness::clear_for_tests();

    let model = "deepseek-v4.1-flash";
    let provider = catalog::forwarding_providers(model)
        .into_iter()
        .next()
        .expect("测试模型至少应有一个可转发 provider");
    readiness::hold(&provider, "测试用例临时挡住整家");

    let scope = AccountGroupScope::from_canonical(
        "group-readiness",
        vec![
            account("model-member", &provider, 20),
            account("wrong-model", "provider-outside-group", 1),
        ],
    );
    let keys = routing::CooldownKeys::new(model);
    let candidates = account_group_pool::accounts_in_group(&scope, &keys, None);

    assert_eq!(ids(&candidates), ["model-member"]);
    assert!(!ids(&candidates).contains(&"wrong-model"));

    readiness::clear_for_tests();
}

#[test]
fn preserving_input_order_ignores_global_priority_and_applies_sticky_first() {
    let accounts = vec![
        account("canonical-first", "workbuddy", 100),
        account("canonical-second", "workbuddy", 1),
    ];
    let keys = routing::CooldownKeys::new("test-model");
    let counts = HashMap::new();
    let sunk = HashMap::new();
    let resting = HashMap::new();
    let open = HashSet::new();
    let limiter_blocked = HashSet::new();
    let meta = plan_meta(
        RouteStrategy::Order,
        None,
        None,
        Some("canonical-second"),
        &sunk,
        &resting,
        &open,
        &limiter_blocked,
    );

    let plan =
        routing::plan_route_preserving_input_order(&accounts, &keys, &counts, &[], 1_000, &meta);

    assert_eq!(
        plan.ordered
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["canonical-second", "canonical-first"]
    );
    assert!(plan.sticky_hit);
}

#[test]
fn preserving_input_order_rotates_only_remaining_group_members() {
    let accounts = vec![
        account("canonical-first", "workbuddy", 1),
        account("canonical-second", "workbuddy", 2),
        account("canonical-third", "workbuddy", 3),
    ];
    let keys = routing::CooldownKeys::new("test-model");
    let counts = HashMap::new();
    let sunk = HashMap::new();
    let resting = HashMap::new();
    let open = HashSet::new();
    let limiter_blocked = HashSet::new();
    let meta = plan_meta(
        RouteStrategy::Rotate,
        Some(1),
        None,
        Some("canonical-third"),
        &sunk,
        &resting,
        &open,
        &limiter_blocked,
    );

    let plan =
        routing::plan_route_preserving_input_order(&accounts, &keys, &counts, &[], 1_000, &meta);

    assert_eq!(
        plan.ordered
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["canonical-third", "canonical-second", "canonical-first"]
    );
}

#[test]
fn preserving_input_order_applies_provider_blocks_without_global_priority() {
    let accounts = vec![
        account("a1", "A", 100),
        account("b1", "B", 1),
        account("a3", "A", 120),
    ];
    let keys = routing::CooldownKeys::new("test-model");
    let counts = HashMap::new();
    let sunk = HashMap::new();
    let resting = HashMap::new();
    let open = HashSet::new();
    let limiter_blocked = HashSet::new();
    let provider_cursors = HashMap::from([("A".to_string(), 1usize)]);
    let meta = plan_meta(
        RouteStrategy::Provider,
        None,
        Some(&provider_cursors),
        None,
        &sunk,
        &resting,
        &open,
        &limiter_blocked,
    );

    let plan =
        routing::plan_route_preserving_input_order(&accounts, &keys, &counts, &[], 1_000, &meta);

    assert_eq!(
        plan.ordered
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["a3", "a1", "b1"]
    );
}

#[test]
fn provider_cursors_are_partitioned_by_group_and_default_pool() {
    let service = test_service();
    let sizes = vec![("A".to_string(), 2usize)];

    assert_eq!(
        service.route.rotate_take_per_provider("test-model", &sizes),
        HashMap::from([("A".to_string(), 0usize)])
    );
    assert_eq!(
        service
            .route
            .rotate_take_per_provider("g:group-a:test-model", &sizes),
        HashMap::from([("A".to_string(), 0usize)])
    );
    assert_eq!(
        service
            .route
            .rotate_take_per_provider("g:group-a:test-model", &sizes),
        HashMap::from([("A".to_string(), 1usize)])
    );
    assert_eq!(
        service.route.rotate_take_per_provider("test-model", &sizes),
        HashMap::from([("A".to_string(), 1usize)])
    );
}

#[test]
fn rotate_and_sticky_state_are_partitioned_by_group() {
    let service = test_service();

    assert_eq!(service.route.rotate_take("test-model"), 0);
    assert_eq!(service.route.rotate_take("test-model"), 1);
    assert_eq!(service.route.rotate_take("g:group-a:test-model"), 0);
    assert_eq!(service.route.rotate_take("g:group-a:test-model"), 1);
    assert_eq!(service.route.rotate_take("g:group-b:test-model"), 0);

    service
        .route
        .sticky_bind("g:group-a:conversation", "group-a-account", 1_000);
    assert_eq!(
        service
            .route
            .sticky_lookup("g:group-a:conversation", 1_001, 60_000),
        Some("group-a-account".to_string())
    );
    assert_eq!(
        service
            .route
            .sticky_lookup("g:group-b:conversation", 1_001, 60_000),
        None
    );
    assert_eq!(
        service.route.sticky_lookup("conversation", 1_001, 60_000),
        None
    );

    service.route.sticky_bind("", "group-a-account", 1_002);
    assert_eq!(service.route.sticky_lookup("", 1_003, 60_000), None);
}

#[test]
fn shared_account_connection_count_is_one_table_and_releases_on_drop() {
    let connections = Connections::new();
    let group_a = AccountGroupScope::from_canonical("group-a", vec![]);
    let group_b = AccountGroupScope::from_canonical("group-b", vec![]);
    assert_ne!(group_a.group_id(), group_b.group_id());

    let mut first = ConnectionGuard::new(connections.clone());
    first.rebind(Some("shared-account".to_string()));
    let mut second = ConnectionGuard::new(connections.clone());
    second.rebind(Some("shared-account".to_string()));
    assert_eq!(connections.snapshot(), [("shared-account".to_string(), 2)]);

    drop(first);
    assert_eq!(connections.snapshot(), [("shared-account".to_string(), 1)]);
    drop(second);
    assert!(connections.snapshot().is_empty());
}
