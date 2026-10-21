//! Management-lock barrier tests for account-group HTTP entry points.

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use tokio::sync::Barrier;

use crate::server::core::account_groups::{AccountGroupMember, MemberType};

use super::account_groups_http_support::{
    add_account, create_default_key, create_group, delete_group, delete_key, key_by_id, lock,
    request, unique,
};

fn account_group(
    env: &super::account_groups_http_support::TestEnv,
    id: &str,
) -> crate::server::core::account_groups::AccountGroup {
    env.state
        .account_groups()
        .list()
        .expect("账号组应能读取")
        .into_iter()
        .find(|group| group.id == id)
        .expect("账号组应存在")
}

#[tokio::test]
async fn c09_group_delete_and_key_binding_leave_no_dangling_reference() {
    let env = lock();
    let group = create_group(&env, "c09-group").await;
    let key_id = create_default_key(&env, "c09-key").await;

    let barrier = Arc::new(Barrier::new(2));
    let delete_barrier = Arc::clone(&barrier);
    let bind_barrier = Arc::clone(&barrier);
    let delete_path = format!("/api/account-groups/{group}");
    let bind_path = format!("/api/keys/{key_id}");
    let delete_request = async {
        delete_barrier.wait().await;
        request(&env, Method::DELETE, &delete_path, Value::Null).await
    };
    let bind_request = async {
        bind_barrier.wait().await;
        request(
            &env,
            Method::PATCH,
            &bind_path,
            json!({
                "routeMode": "account_group",
                "accountGroupId": group
            }),
        )
        .await
    };
    let ((delete_status, delete_body), (bind_status, bind_body)) =
        tokio::join!(delete_request, bind_request);

    match (delete_status, bind_status) {
        (StatusCode::OK, StatusCode::NOT_FOUND) => {
            assert!(bind_body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("账号组不存在"));
            let key = key_by_id(&env, &key_id).await;
            assert_eq!(key["routeMode"], "default_pool");
            assert_eq!(key["accountGroupId"], Value::Null);
        }
        (StatusCode::CONFLICT, StatusCode::OK) => {
            assert!(delete_body["usedBy"]
                .as_array()
                .expect("冲突响应应包含 usedBy")
                .iter()
                .any(|entry| entry["id"] == key_id));
            let key = key_by_id(&env, &key_id).await;
            assert_eq!(key["routeMode"], "account_group");
            assert_eq!(key["accountGroupId"], group);
            let (status, _) = request(
                &env,
                Method::PATCH,
                &bind_path,
                json!({ "routeMode": "default_pool", "accountGroupId": null }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            delete_group(&env, &group).await;
        }
        _ => panic!("管理锁竞态出现非法响应组合: delete={delete_status}, bind={bind_status}"),
    }

    delete_key(&env, &key_id).await;
}

#[tokio::test]
async fn c09_logout_entries_remove_explicit_group_members() {
    let env = lock();
    for path in ["/api/session/logout", "/auth/logout"] {
        let account = add_account(&env.state.store(), &unique("c09-logout-account"));
        let group = create_group(&env, "c09-logout-group").await;
        env.state
            .with_management_lock(|| {
                env.state.account_groups().replace_members(
                    &group,
                    vec![AccountGroupMember {
                        position: 0,
                        member_type: MemberType::Account,
                        member_id: account.clone(),
                    }],
                )
            })
            .expect("显式账号成员应能写入");

        let (status, _) = request(&env, Method::POST, path, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{path} 应成功");
        assert!(!env.state.store().list_accounts()["accounts"]
            .as_array()
            .expect("账号列表应为数组")
            .iter()
            .any(|item| item["id"] == account));
        assert!(account_group(&env, &group).members.is_empty());
        delete_group(&env, &group).await;
    }
}

#[tokio::test]
async fn c09_account_delete_and_member_replace_leave_no_dangling_member() {
    let env = lock();
    let account = add_account(&env.state.store(), &unique("c09-delete-account"));
    let group = create_group(&env, "c09-delete-group").await;
    env.state
        .with_management_lock(|| {
            env.state.account_groups().replace_members(
                &group,
                vec![AccountGroupMember {
                    position: 0,
                    member_type: MemberType::Account,
                    member_id: account.clone(),
                }],
            )
        })
        .expect("显式账号成员应能写入");

    let barrier = Arc::new(Barrier::new(2));
    let delete_barrier = Arc::clone(&barrier);
    let replace_barrier = Arc::clone(&barrier);
    let delete_path = format!("/api/accounts/{account}");
    let replace_path = format!("/api/account-groups/{group}/members");
    let delete_member = async {
        delete_barrier.wait().await;
        request(&env, Method::DELETE, &delete_path, Value::Null).await
    };
    let replace_member = async {
        replace_barrier.wait().await;
        request(
            &env,
            Method::PUT,
            &replace_path,
            json!({ "members": [{ "type": "account", "id": account }] }),
        )
        .await
    };
    let ((delete_status, _), (replace_status, _)) = tokio::join!(delete_member, replace_member);

    assert_eq!(delete_status, StatusCode::OK);
    assert!(matches!(
        replace_status,
        StatusCode::OK | StatusCode::BAD_REQUEST
    ));
    assert!(account_group(&env, &group).members.is_empty());
    assert!(!env.state.store().list_accounts()["accounts"]
        .as_array()
        .expect("账号列表应为数组")
        .iter()
        .any(|item| item["id"] == account));
    delete_group(&env, &group).await;
}
