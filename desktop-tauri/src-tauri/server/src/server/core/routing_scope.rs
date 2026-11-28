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
        let ids = match profile.order_mode_for(provider) {
            config::AccountOrderMode::FollowGroup => sorting.account_order.get(provider),
            config::AccountOrderMode::Manual => profile.account_order.get(provider).or_else(|| sorting.account_order.get(provider)),
        };
        if let Some(ids) = ids {
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
        profile.account_order_mode.clear();
    }
    if !independent && !model.is_empty() {
        profile.providers.retain(|id| available.contains(id));
        if scope == RoutingProviderScope::Selected {
            profile.disabled_providers.retain(|id| profile.providers.contains(id));
        }
    }
    if independent {
        profile.account_order_mode.retain(|provider, _| profile.providers.contains(provider));
    }
    for provider in &profile.providers {
        if independent {
            let mode = profile.order_mode_for(provider);
            profile.account_order_mode.insert(provider.clone(), mode);
            if mode == config::AccountOrderMode::FollowGroup {
                profile.account_order.insert(
                    provider.clone(),
                    sorting.account_order.get(provider).cloned().unwrap_or_default(),
                );
                continue;
            }
        }
        let ids = profile.account_order.entry(provider.clone()).or_default();
        for id in sorting.account_order.get(provider).into_iter().flatten() {
            if !ids.contains(id) { ids.push(id.clone()); }
        }
    }
    profile
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use serde_json::json;

    #[test]
    fn model_profile_follow_group_uses_live_provider_order_while_manual_keeps_saved_order() {
        let accounts = vec![
            json!({ "id": "a1", "provider": "demo", "priority": 100 }),
            json!({ "id": "a2", "provider": "demo", "priority": 200 }),
        ];
        // 保存的 manual 顺序是 [a2, a1]，但切换到 FollowGroup 后应按账号库实时顺序 [a1, a2]
        let follow = RoutingProfile {
            provider_scope: Some(RoutingProviderScope::Selected),
            providers: vec!["demo".into()],
            account_order: BTreeMap::from([("demo".into(), vec!["a2".into(), "a1".into()])]),
            account_order_mode: BTreeMap::from([("demo".into(), config::AccountOrderMode::FollowGroup)]),
            ..Default::default()
        };
        let ids: Vec<_> = project_accounts(&accounts, &follow)
            .iter()
            .filter_map(routing::account_id)
            .map(str::to_string)
            .collect();
        assert_eq!(ids, vec!["a1", "a2"]);

        // 显式 Manual 或未带 accountOrderMode 的旧版已保存配置，保持 [a2, a1]
        let legacy_manual = RoutingProfile {
            provider_scope: Some(RoutingProviderScope::Selected),
            providers: vec!["demo".into()],
            account_order: BTreeMap::from([("demo".into(), vec!["a2".into(), "a1".into()])]),
            ..Default::default()
        };
        let ids: Vec<_> = project_accounts(&accounts, &legacy_manual)
            .iter()
            .filter_map(routing::account_id)
            .map(str::to_string)
            .collect();
        assert_eq!(ids, vec!["a2", "a1"]);
    }
}
