//! Pure ordered account-group member expansion.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::server::core::account_groups::store::{AccountGroupMember, MemberType};
use crate::server::core::routing;

/// A missing account or provider reference observed while expanding a group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingGroupMember {
    pub position: i64,
    pub member_type: MemberType,
    pub member_id: String,
}

/// The canonical ordered account view produced from group members.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedAccountGroup {
    pub accounts: Vec<Value>,
    pub providers: Vec<String>,
    pub missing_members: Vec<MissingGroupMember>,
}

/// Expand ordered group members against one account snapshot.
///
/// The function intentionally has no database or error return.  Stale references
/// are reported in `missing_members` and valid references remain usable.
pub fn resolve(members: &[AccountGroupMember], accounts: &[Value]) -> ResolvedAccountGroup {
    let by_id: HashMap<&str, &Value> = accounts
        .iter()
        .filter_map(|account| routing::account_id(account).map(|id| (id, account)))
        .collect();
    let mut ordered_members: Vec<&AccountGroupMember> = members.iter().collect();
    ordered_members.sort_by_key(|member| member.position);

    let mut resolved = ResolvedAccountGroup::default();
    let mut seen_accounts = HashSet::new();
    let mut seen_providers = HashSet::new();

    for member in ordered_members {
        let candidates: Vec<&Value> = match member.member_type {
            MemberType::Account => match by_id.get(member.member_id.as_str()) {
                Some(account) => vec![*account],
                None => {
                    resolved.missing_members.push(MissingGroupMember {
                        position: member.position,
                        member_type: member.member_type,
                        member_id: member.member_id.clone(),
                    });
                    continue;
                }
            },
            MemberType::Provider => {
                let provider_accounts = routing::provider_accounts_in_default_order(
                    accounts,
                    &member.member_id,
                );
                if provider_accounts.is_empty() {
                    resolved.missing_members.push(MissingGroupMember {
                        position: member.position,
                        member_type: member.member_type,
                        member_id: member.member_id.clone(),
                    });
                    continue;
                }
                for account in provider_accounts {
                    if let Some(id) = routing::account_id(&account) {
                        if seen_accounts.insert(id.to_string()) {
                            let account = without_credentials(account);
                            let provider = routing::provider_of(&account).to_string();
                            if seen_providers.insert(provider.clone()) {
                                resolved.providers.push(provider);
                            }
                            resolved.accounts.push(account);
                        }
                    }
                }
                continue;
            }
        };

        for account in candidates {
            let Some(id) = routing::account_id(account) else {
                continue;
            };
            if !seen_accounts.insert(id.to_string()) {
                continue;
            }
            let account = without_credentials(account.clone());
            let provider = routing::provider_of(&account).to_string();
            if seen_providers.insert(provider.clone()) {
                resolved.providers.push(provider);
            }
            resolved.accounts.push(account);
        }
    }

    resolved
}

/// Group snapshots are public account views, so credentials must not cross this
/// boundary even when a caller supplies an internal account record by mistake.
fn without_credentials(mut account: Value) -> Value {
    if let Value::Object(fields) = &mut account {
        for key in [
            "accessToken",
            "refreshToken",
            "token",
            "apiKey",
            "access_token",
            "refresh_token",
            "clientSecret",
            "client_secret",
            "password",
        ] {
            fields.remove(key);
        }
    }
    account
}
