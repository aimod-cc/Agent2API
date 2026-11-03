use std::collections::HashMap;

use serde_json::{json, Value};

use crate::server::{config, core::{account_order, routing}};

#[derive(Clone)]
pub(crate) struct RequestRouting {
    pub settings: config::RoutingSettings,
    pub profile: Option<config::RoutingProfile>,
    pub scope: String,
    pub captured_at: i64,
    positions: HashMap<String, usize>,
}

impl RequestRouting {
    pub fn capture(model: &str, store: &crate::server::core::account_store::store::AccountStore) -> Self {
        let snapshot = store.routing_snapshot();
        let accounts: Vec<Value> = snapshot.accounts.iter()
            .filter(|account| crate::server::core::account_store::store_view::account_visible(routing::provider_of(account)))
            .map(|account| account.as_ref().clone()).collect();
        let (profiles, base) = config::routing_snapshot();
        let settings = profiles.settings_for(model, base);
        let ordered = if let Some(profile) = profiles.model_profile(model) {
            crate::server::core::routing_scope::project_accounts(&accounts, profile)
        } else {
            let sorting = account_order::sorting(&accounts, crate::server::logging::now_ms());
            account_order::ordered_accounts(&accounts, &sorting).into_iter().filter(|account| profiles.global.as_ref().map_or(true, |profile|
                profile.includes_provider(routing::provider_of(account)) && !routing::account_id(account).is_some_and(|id| profile.excluded_accounts.iter().any(|excluded| excluded == id))))
                .collect()
        };
        Self {
            settings,
            profile: profiles.effective(model).cloned(),
            scope: profiles.scope(model).to_string(),
            captured_at: crate::server::logging::now_ms(),
            positions: ordered.iter().enumerate().filter_map(|(index, account)| routing::account_id(account).map(|id| (id.to_string(), index))).collect(),
        }
    }

    pub fn providers(&self, available: Vec<String>) -> Vec<String> {
        let Some(profile) = &self.profile else { return available };
        let mut ordered = if self.scope.is_empty() { available.clone() } else { profile.providers.clone() };
        if profile.scope() == config::RoutingProviderScope::All {
            for id in &available { if !ordered.contains(id) { ordered.push(id.clone()); } }
        }
        ordered.retain(|id| available.contains(id) && profile.includes_provider(id));
        ordered
    }

    pub fn accounts(&self, accounts: Vec<Value>) -> Vec<Value> {
        let mut accounts: Vec<_> = accounts.into_iter().filter(|account| routing::account_id(account)
            .is_some_and(|id| self.positions.contains_key(id))).collect();
        accounts.sort_by_key(|account| routing::account_id(account).and_then(|id| self.positions.get(id)).copied().unwrap_or(usize::MAX));
        for (index, account) in accounts.iter_mut().enumerate() {
            if let Some(object) = account.as_object_mut() { object.insert("routingPriority".to_string(), index.into()); }
        }
        accounts
    }

    pub fn json(&self) -> Value {
        json!({
            "capturedAt": self.captured_at, "scope": self.scope,
            "strategy": self.settings.strategy.as_str(), "sticky": self.settings.sticky.as_str(),
            "providerScope": self.profile.as_ref().map(|profile| profile.scope()).unwrap_or(config::RoutingProviderScope::All),
            "providerCount": self.profile.as_ref().map(|profile| profile.providers.len()),
            "accountCount": self.positions.len(),
        })
    }
}
