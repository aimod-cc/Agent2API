use serde_json::{json, Value};

use super::resolver::resolve;
use super::store::{AccountGroupMember, MemberType};

fn account(id: &str, provider: &str, priority: i64) -> Value {
    json!({
        "id": id,
        "provider": provider,
        "priority": priority,
        "enabled": true,
        "accessToken": format!("secret-{id}"),
    })
}

fn member(position: i64, member_type: MemberType, member_id: &str) -> AccountGroupMember {
    AccountGroupMember {
        position,
        member_type,
        member_id: member_id.to_string(),
    }
}

fn ids(accounts: &[Value]) -> Vec<&str> {
    accounts
        .iter()
        .map(|account| account["id"].as_str().unwrap())
        .collect()
}

#[test]
fn resolver_expands_provider_members_in_default_order_and_keeps_group_order() {
    let accounts = vec![
        account("a-late", "provider-a", 30),
        account("b-first", "provider-b", 10),
        account("a-first", "provider-a", 5),
    ];
    let members = vec![
        member(20, MemberType::Provider, "provider-b"),
        member(0, MemberType::Provider, "provider-a"),
    ];

    let resolved = resolve(&members, &accounts);
    assert_eq!(ids(&resolved.accounts), ["a-first", "a-late", "b-first"]);
    assert_eq!(resolved.providers, ["provider-a", "provider-b"]);
    assert!(resolved.missing_members.is_empty());
    assert!(resolved
        .accounts
        .iter()
        .all(|account| { account.get("accessToken").is_none() }));
}

#[test]
fn resolver_deduplicates_accounts_and_reports_missing_members() {
    let accounts = vec![
        account("a1", "provider-a", 1),
        account("b1", "provider-b", 2),
    ];
    let members = vec![
        member(0, MemberType::Account, "a1"),
        member(1, MemberType::Provider, "provider-a"),
        member(2, MemberType::Account, "missing"),
        member(3, MemberType::Provider, "provider-c"),
        member(4, MemberType::Account, "b1"),
    ];

    let resolved = resolve(&members, &accounts);
    assert_eq!(ids(&resolved.accounts), ["a1", "b1"]);
    assert_eq!(resolved.providers, ["provider-a", "provider-b"]);
    assert_eq!(resolved.missing_members.len(), 2);
    assert_eq!(resolved.missing_members[0].member_id, "missing");
    assert_eq!(resolved.missing_members[1].member_id, "provider-c");
}
