//! 全局顺序来自账号库与消耗规则；参与范围独立于展示名单。

use std::collections::HashMap;

use serde_json::Value;

use crate::server::config::{self, RoutingProfile, RoutingProviderScope};
use super::{account_order, routing};

pub fn available_providers(accounts: &[Value]) -> Vec<String> {
    let mut sorted = accounts.to_vec();
    sorted.sort_by_key(|account| account.get("priority").and_then(Value::as_i64).unwrap_or(i64::MAX));
    let mut ids = Vec::new();
    for account in sorted {
        let id = routing::provider_of(&account).to_string();
        if !ids.contains(&id) { ids.push(id); }
    }
    for id in crate::server::core::plugin_host::installed_ids() {
        if !ids.contains(&id) { ids.push(id); }
    }
    for provider in crate::server::core::custom_providers::list() {
        if let Some(id) = provider.get("id").and_then(Value::as_str) {
            if !ids.iter().any(|known| known == id) { ids.push(id.to_string()); }
        }
    }
    ids
}

pub fn providers_for(model: &str, available: &[String]) -> Vec<String> {
    let profiles = config::routing_profiles();
    match profiles.effective(model) {
        Some(profile) => {
            let mut providers = if profiles.model_profile(model).is_some() {
                profile.providers.clone()
            } else {
                available.to_vec()
            };
            if profile.scope() == RoutingProviderScope::All {
                for provider in available {
                    if !providers.contains(provider) { providers.push(provider.clone()); }
                }
            }
            providers.retain(|id| available.contains(id) && profile.includes_provider(id));
            providers
        }
        None => available.to_vec(),
    }
}

pub fn apply_accounts(accounts: &[Value], model: &str) -> Vec<Value> {
    let profiles = config::routing_profiles();
    let mut ordered = if let Some(profile) = profiles.model_profile(model) {
        project_accounts(accounts, profile)
    } else {
        let sorting = account_order::sorting(accounts, crate::server::logging::now_ms());
        let mut ordered = account_order::ordered_accounts(accounts, &sorting);
        if let Some(profile) = &profiles.global {
            ordered.retain(|account| profile.includes_provider(routing::provider_of(account))
                && !routing::account_id(account).is_some_and(|id| profile.excluded_accounts.iter().any(|excluded| excluded == id)));
        }
        ordered
    };
    for (index, account) in ordered.iter_mut().enumerate() {
        if let Some(object) = account.as_object_mut() {
            object.insert("routingPriority".to_string(), Value::from(index));
        }
    }
    ordered
}

pub fn project_accounts(accounts: &[Value], profile: &RoutingProfile) -> Vec<Value> {
    let sorting = account_order::sorting(accounts, crate::server::logging::now_ms());
    let baseline = account_order::ordered_accounts(accounts, &sorting);
    let mut providers = profile.providers.clone();
    if profile.scope() == RoutingProviderScope::All {
        for provider in &sorting.provider_order {
            if !providers.contains(provider) { providers.push(provider.clone()); }
        }
    }
    let mut ordered = Vec::new();
    for provider in &providers {
        if !profile.includes_provider(provider) { continue; }
        let mut members: Vec<Value> = baseline.iter()
            .filter(|account| routing::provider_of(account) == provider)
            .filter(|account| routing::account_id(account)
                .is_some_and(|id| !profile.excluded_accounts.iter().any(|excluded| excluded == id)))
            .cloned().collect();
        if let Some(ids) = profile.account_order.get(provider).or_else(|| sorting.account_order.get(provider)) {
            let positions: HashMap<&str, usize> = ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
            members.sort_by_key(|account| routing::account_id(account)
                .and_then(|id| positions.get(id)).copied().unwrap_or(usize::MAX));
        }
        ordered.extend(members);
    }
    ordered
}

pub fn materialize_profile(accounts: &[Value], available: &[String], model: &str) -> RoutingProfile {
    materialize_profile_with(accounts, available, model, &config::routing_profiles())
}

pub fn materialize_profile_with(accounts: &[Value], available: &[String], model: &str, profiles: &config::RoutingProfiles) -> RoutingProfile {
    let sorting = account_order::sorting(accounts, crate::server::logging::now_ms());
    let independent = profiles.model_profile(model).is_some();
    let mut profile = profiles.effective(model).cloned().unwrap_or_else(|| RoutingProfile {
        provider_scope: Some(RoutingProviderScope::All),
        providers: available.to_vec(),
        ..Default::default()
    });
    let scope = profile.scope();
    profile.provider_scope = Some(scope);
    if independent && scope == RoutingProviderScope::All {
        for provider in available {
            if !profile.providers.contains(provider) { profile.providers.push(provider.clone()); }
        }
    }
    if !independent {
        let mut providers = sorting.provider_order.clone();
        providers.extend(available.iter().filter(|id| !sorting.provider_order.contains(id)).cloned());
        for id in profile.providers.iter().chain(&profile.disabled_providers) {
            if !providers.contains(id) { providers.push(id.clone()); }
        }
        if scope == RoutingProviderScope::Selected {
            providers.retain(|id| profile.providers.contains(id));
        }
        profile.providers = providers;
        profile.account_order.clear();
    }
    if !independent && !model.is_empty() {
        profile.providers.retain(|id| available.contains(id));
        if scope == RoutingProviderScope::Selected {
            profile.disabled_providers.retain(|id| profile.providers.contains(id));
        }
    }
    for provider in &profile.providers {
        let ids = profile.account_order.entry(provider.clone()).or_default();
        for id in sorting.account_order.get(provider).into_iter().flatten() {
            if !ids.contains(id) { ids.push(id.clone()); }
        }
    }
    profile
}
