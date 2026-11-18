use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::{store_view::account_list_visible, AccountStore};
use crate::server::core::{account_order, providers, routing};

impl AccountStore {
    pub(crate) fn account_list_snapshot(&self, index_only: bool, booting: bool) -> Value {
        let cached = self.routing_snapshot();
        let mut counts = BTreeMap::new();
        let mut hidden = BTreeMap::new();
        let mut current_ids = Map::new();
        let mut current: Option<std::sync::Arc<Value>> = None;
        let mut visible = Vec::new();
        for account in &cached.public_accounts {
            let provider = routing::provider_of(account);
            if !account_list_visible(provider, booting) {
                *hidden.entry(provider.to_string()).or_insert(0usize) += 1;
                continue;
            }
            *counts.entry(provider.to_string()).or_insert(0usize) += 1;
            visible.push(account.as_ref());
            if account.get("enabled").and_then(Value::as_bool) == Some(false)
                || account.get("hasCredentials").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            let replace = current_ids.get(provider).and_then(Value::as_str)
                .and_then(|id| cached.public_by_id.get(id))
                .is_none_or(|previous| super::store::route_order_key(account) < super::store::route_order_key(previous));
            if replace {
                current_ids.insert(provider.to_string(), account["id"].clone());
            }
            if cached.forwarding_providers.contains(provider)
                && current.as_ref().is_none_or(|previous| super::store::route_order_key(account) < super::store::route_order_key(previous)) {
                current = Some(account.clone());
            }
        }
        for provider in counts.keys() { current_ids.entry(provider.clone()).or_insert(Value::Null); }
        let ids = counts.keys().cloned().collect::<Vec<_>>();
        let route_accounts = visible.iter().map(|account| {
            let mut fields = Map::new();
            for key in ["id", "provider", "priority", "addedAt", "isGuest", "usageConfigKey"] {
                if let Some(value) = account.get(key) { fields.insert(key.to_string(), value.clone()); }
            }
            Value::Object(fields)
        }).collect::<Vec<_>>();
        let sorting = account_order::sorting(&route_accounts, crate::server::logging::now_ms());
        json!({
            "currentAccountId": current.map(|account| account["id"].clone()),
            "currentAccountIds": current_ids,
            "accounts": visible.into_iter().map(|account| self.decorate_policy_cooldown(if index_only { account_index(account) } else { account.clone() }, "")).collect::<Vec<_>>(),
            "providers": providers::summary_json(|id| counts.get(id).copied().unwrap_or(0), &ids),
            "hiddenAccounts": super::hidden_accounts::summary(hidden),
            "sorting": sorting,
            "indexOnly": index_only,
            "total": counts.values().sum::<usize>(),
        })
    }

    pub(crate) fn public_account_by_id(&self, id: &str) -> Option<Value> {
        let cached = self.routing_snapshot();
        let account = cached.public_by_id.get(id)?;
        super::store_view::account_visible(routing::provider_of(account)).then(|| self.decorate_policy_cooldown(refresh_public_account(account), ""))
    }
}

pub(crate) fn refresh_public_account(account: &Value) -> Value {
    let mut value = account.clone();
    if let Some(proxy) = account.get("proxy") {
        value["proxy"] = crate::server::core::proxies::describe_account_proxy(Some(proxy.get("config").unwrap_or(proxy)));
    }
    value
}

fn account_index(account: &Value) -> Value {
    let mut fields = Map::new();
    for key in [
        "id", "provider", "name", "nickname", "displayName", "email", "uid", "userId", "account",
        "loginName", "phone", "nameCustom", "priority", "addedAt", "updatedAt", "enabled", "available",
        "hasCredentials", "desktop", "hasRefreshToken", "canRefresh", "canSyncCredentials", "needsRelogin", "isGuest", "canClaim",
        "chatSupported", "edition", "editionLabel", "zcodePlan", "rateLimits", "routingCooldowns", "upstreamCooldownUntil", "upstreamBlockedReason",
        "claimPlans", "claimAt", "claimPlanId",
        "expiresAt", "tokenExpiresAt", "tokenExpireAt", "accessTokenExpiresAt", "hasAccessToken", "maxConcurrent",
    ] {
        if let Some(value) = account.get(key) { fields.insert(key.to_string(), value.clone()); }
    }
    if let Some(scheme) = account.pointer("/balanceQuery/scheme") {
        fields.insert("balanceQuery".to_string(), json!({ "scheme": scheme }));
    }
    fields.insert("detailsLoaded".to_string(), Value::Bool(false));
    Value::Object(fields)
}
