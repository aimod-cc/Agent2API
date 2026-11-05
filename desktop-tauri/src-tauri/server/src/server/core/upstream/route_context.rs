use std::collections::HashMap;
use serde_json::{json, Value};
use crate::server::{config, core::{account_order, routing, routing_groups::{self, GroupProjection, RequestNeeds}}};

#[derive(Clone)]
pub(crate) struct RequestRouting {
    pub settings: config::RoutingSettings,
    pub profile: Option<config::RoutingProfile>,
    pub scope: String,
    pub captured_at: i64,
    pub group: Option<config::RoutingGroup>,
    pub group_projection: GroupProjection,
    positions: HashMap<String, usize>,
}

impl RequestRouting {
    fn capture_with(model: &str, store: &crate::server::core::account_store::store::AccountStore,
        profiles: &config::RoutingProfiles, base: config::RoutingSettings) -> Self {
        let snapshot = store.routing_snapshot();
        let accounts: Vec<Value> = snapshot.accounts.iter()
            .filter(|account| crate::server::core::account_store::store_view::account_visible(routing::provider_of(account)))
            .map(|account| account.as_ref().clone()).collect();
        let ordered = if let Some(profile) = profiles.model_profile(model) {
            crate::server::core::routing_scope::project_accounts(&accounts, profile)
        } else {
            let sorting = account_order::sorting(&accounts, crate::server::logging::now_ms());
            account_order::ordered_accounts(&accounts, &sorting).into_iter().filter(|account| profiles.global.as_ref().map_or(true, |profile|
                profile.includes_provider(routing::provider_of(account)) && !routing::account_id(account).is_some_and(|id| profile.excluded_accounts.iter().any(|excluded| excluded == id))))
                .collect()
        };
        Self { settings: profiles.settings_for(model, base), profile: profiles.effective(model).cloned(),
            scope: profiles.scope(model).to_string(), captured_at: crate::server::logging::now_ms(), group: None,
            group_projection: Default::default(), positions: ordered.iter().enumerate().filter_map(|(index, account)|
                routing::account_id(account).map(|id| (id.to_string(), index))).collect() }
    }

    pub fn capture_request(model: &str, store: &crate::server::core::account_store::store::AccountStore,
        body: &Value, scope: Option<&crate::server::core::key_scope::KeyScope>, ignore: bool) -> Result<Self, crate::server::errors::GatewayError> {
        let (profiles, base) = config::routing_snapshot();
        if !ignore && body.get("_wb_route_group_id").and_then(Value::as_str).is_some_and(|id|
            !profiles.group_for_model(model).is_some_and(|group| group.id == id)) {
            return Err(crate::server::errors::GatewayError::with_status(409, "路由组已删除或入口已变更，请重新发起请求；未回退其他路由"));
        }
        let mut routing = Self::capture_with(model, store, &profiles, base);
        if !ignore {
            if let Some(group) = profiles.group_for_model(model).cloned() {
                if !crate::server::core::key_scope::allows_model(scope, &group.model) {
                    return Err(crate::server::errors::GatewayError::with_status(404, "此路由组不在网关 Key 的可用模型列表里").with_code("model_not_found"));
                }
                let snapshot = store.routing_snapshot();
                let accounts: Vec<_> = snapshot.accounts.iter().map(|account| account.as_ref().clone()).collect();
                routing.group_projection = routing_groups::project(&group, &accounts, &RequestNeeds::from_body(body), scope);
                if let Some(continuation) = body.get("_wb_route_continue") {
                    routing.group_projection.accounts.retain(|account| {
                        continuation.get("candidateId").and_then(Value::as_str) == Some(routing_groups::attempt_key(account).as_str())
                            && continuation.get("accountId").and_then(Value::as_str) == routing::account_id(account)
                            && continuation.get("model").and_then(Value::as_str) == routing_groups::candidate_model(account)
                    });
                }
                routing.settings = group.settings(base);
                routing.profile = Some(group.profile());
                routing.scope = group.model.clone();
                routing.group = Some(group);
            }
        }
        Ok(routing)
    }

    pub fn providers(&self, available: Vec<String>) -> Vec<String> {
        if let Some(group) = &self.group { return group.profile().providers; }
        let Some(profile) = &self.profile else { return available };
        let mut ordered = if self.scope.is_empty() { available.clone() } else { profile.providers.clone() };
        if profile.scope() == config::RoutingProviderScope::All {
            for id in &available { if !ordered.contains(id) { ordered.push(id.clone()); } }
        }
        ordered.retain(|id| available.contains(id) && profile.includes_provider(id));
        ordered
    }

    pub fn accounts(&self, accounts: Vec<Value>) -> Vec<Value> {
        if self.group.is_some() {
            return self.group_projection.accounts.iter().filter_map(|candidate| {
                let mut current = accounts.iter().find(|account| routing::account_id(account) == routing::account_id(candidate)
                    && routing::provider_of(account) == routing::provider_of(candidate))?.clone();
                for key in ["routingCandidateId", "routingMemberId", "routingModel", "routingPriority"] { current[key] = candidate[key].clone(); }
                Some(current)
            }).collect();
        }
        let mut accounts: Vec<_> = accounts.into_iter().filter(|account| routing::account_id(account)
            .is_some_and(|id| self.positions.contains_key(id))).collect();
        accounts.sort_by_key(|account| routing::account_id(account).and_then(|id| self.positions.get(id)).copied().unwrap_or(usize::MAX));
        for (index, account) in accounts.iter_mut().enumerate() {
            if let Some(object) = account.as_object_mut() { object.insert("routingPriority".to_string(), index.into()); }
        }
        accounts
    }

    pub fn json(&self) -> Value {
        json!({ "capturedAt": self.captured_at, "scope": self.scope,
            "strategy": self.settings.strategy.as_str(), "sticky": self.settings.sticky.as_str(),
            "providerScope": self.profile.as_ref().map(|profile| profile.scope()).unwrap_or(config::RoutingProviderScope::All),
            "providerCount": self.profile.as_ref().map(|profile| profile.providers.len()),
            "accountCount": if self.group.is_some() { self.group_projection.accounts.len() } else { self.positions.len() },
            "groupId": self.group.as_ref().map(|group| &group.id), "groupName": self.group.as_ref().map(|group| &group.name),
            "members": self.group.as_ref().map(|group| group.members.iter().map(|member| json!({
                "id": member.id, "provider": member.provider, "model": member.model, "included": member.included,
                "accountCount": member.accounts.len(),
            })).collect::<Vec<_>>()) })
    }
}
