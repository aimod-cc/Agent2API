use serde_json::{json, Value};

use crate::server::core::account_groups::{AccountGroupMember, MemberType};
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{AccountState, StoredAccount};

use super::test_bootstrap;

fn account(id: &str, provider: &str, priority: i64, name: &str) -> StoredAccount {
    StoredAccount::from_value(json!({
        "id": id,
        "provider": provider,
        "priority": priority,
        "enabled": true,
        "addedAt": priority,
        "name": name,
        "accessToken": format!("token-{id}"),
    }))
    .unwrap()
}

fn member(member_type: MemberType, member_id: &str) -> AccountGroupMember {
    AccountGroupMember {
        position: 0,
        member_type,
        member_id: member_id.to_string(),
    }
}

fn put(env: &test_bootstrap::TestEnv, record: &StoredAccount) {
    env.state
        .db()
        .unwrap()
        .with(|connection| sql::put(connection, record))
        .unwrap()
        .unwrap();
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        crate::server::logging::now_ms()
    )
}

#[test]
fn groups_normalize_names_and_replace_members_atomically() {
    let env = test_bootstrap::lock();
    let service = env.state.account_groups();
    let group = service.create(&format!("  {}  ", unique("Main"))).unwrap();
    assert!(!group.name.starts_with(' '));
    assert!(service.create(&group.name.to_uppercase()).is_err());

    let members = vec![
        AccountGroupMember {
            position: 90,
            member_type: MemberType::Account,
            member_id: "a1".to_string(),
        },
        AccountGroupMember {
            position: 91,
            member_type: MemberType::Provider,
            member_id: "provider-a".to_string(),
        },
    ];
    let saved = service.replace_members(&group.id, members.clone()).unwrap();
    assert_eq!(
        saved
            .members
            .iter()
            .map(|member| member.position)
            .collect::<Vec<_>>(),
        [0, 1]
    );

    let failed = service.replace_members(
        &group.id,
        vec![
            member(MemberType::Account, "a1"),
            member(MemberType::Account, "a1"),
        ],
    );
    assert!(failed.is_err());
    assert_eq!(service.get(&group.id).unwrap().members, saved.members);

    let renamed = service.rename(&group.id, " Renamed ").unwrap();
    assert_eq!(renamed.id, group.id);
    assert_eq!(renamed.name, "Renamed");
    service.delete(&group.id).unwrap();
}

#[test]
fn account_update_keeps_members_but_real_delete_removes_only_account_members() {
    let env = test_bootstrap::lock();
    let store = env.state.store();
    let service = env.state.account_groups();
    let id = unique("account");
    let group = service.create(&unique("Delete group")).unwrap();
    let record = account(
        &id,
        "provider-a",
        20_000 + crate::server::logging::now_ms() % 10_000,
        "before",
    );
    put(&env, &record);
    service
        .replace_members(
            &group.id,
            vec![
                member(MemberType::Account, &id),
                member(MemberType::Provider, "provider-a"),
            ],
        )
        .unwrap();

    let updated = account(&id, "provider-a", record.priority(), "after");
    put(&env, &updated);
    assert_eq!(service.get(&group.id).unwrap().members.len(), 2);

    store.remove_account(&id).unwrap();
    let remaining = service.get(&group.id).unwrap().members;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].member_type, MemberType::Provider);
    assert_eq!(remaining[0].member_id, "provider-a");
    service.delete(&group.id).unwrap();
}

#[test]
fn save_state_uses_encrypted_rows_and_does_not_delete_unreadable_rows() {
    let env = test_bootstrap::lock();
    let store = env.state.store();
    let service = env.state.account_groups();
    let id = unique("state-account");
    let group = service.create(&unique("State group")).unwrap();
    let record = account(
        &id,
        "provider-a",
        30_000 + crate::server::logging::now_ms() % 10_000,
        "before",
    );
    put(&env, &record);
    service
        .replace_members(&group.id, vec![member(MemberType::Account, &id)])
        .unwrap();

    let encrypted: String = env
        .state
        .db()
        .unwrap()
        .with(|connection| {
            connection.query_row("SELECT data FROM accounts WHERE id = ?1", [&id], |row| {
                row.get(0)
            })
        })
        .unwrap()
        .unwrap();
    assert!(encrypted.starts_with("enc1:"));

    let mut changed = record.clone();
    changed.set("name", Value::String("after".to_string()));
    store.with_lock(|guard| {
        let mut state = store.load_locked(guard);
        state.accounts = vec![changed];
        store.save_locked(&state, guard).unwrap();
    });
    assert_eq!(service.get(&group.id).unwrap().members.len(), 1);

    env.state
        .db()
        .unwrap()
        .with(|connection| {
            connection.execute(
                "UPDATE accounts SET data = 'enc1:not-a-valid-envelope' WHERE id = ?1",
                [&id],
            )
        })
        .unwrap()
        .unwrap();
    store.with_lock(|guard| {
        store.save_locked(&AccountState::default(), guard).unwrap();
    });
    let still_there: i64 = env
        .state
        .db()
        .unwrap()
        .with(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM accounts WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
        })
        .unwrap()
        .unwrap();
    assert_eq!(still_there, 1);

    store.remove_account(&id).unwrap();
    service.delete(&group.id).unwrap();
}
