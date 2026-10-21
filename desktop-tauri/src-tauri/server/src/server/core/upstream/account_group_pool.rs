//! Request-time account-group candidate pool.

use serde_json::Value;

use crate::server::core::account_groups::AccountGroupScope;
use crate::server::core::providers::catalog;
use crate::server::core::routing;

use super::rotate;
/// Return the group's canonical accounts that can forward the requested model.
///
/// The input order is the group contract. This function only narrows that
/// snapshot; route ordering and retry state remain in `rotate`.
pub(super) fn accounts_in_group(
    scope: &AccountGroupScope,
    keys: &routing::CooldownKeys<'_>,
    pinned: Option<&str>,
) -> Vec<Value> {
    let forwarding_providers = catalog::forwarding_providers(keys.request_model());
    let original_pool: Vec<Value> = scope
        .accounts()
        .iter()
        .filter(|account| {
            forwarding_providers
                .iter()
                .any(|provider| provider == routing::provider_of(account))
        })
        .filter(|account| match pinned {
            Some(id) => routing::account_id(account) == Some(id),
            None => true,
        })
        .cloned()
        .collect();

    rotate::readiness_candidates_or_original_pool(&original_pool)
}
