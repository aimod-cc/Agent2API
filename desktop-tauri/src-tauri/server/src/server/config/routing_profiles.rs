//! 按模型独立的路由参与范围与顺序；不改写账号库的全局优先级。

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{CONFIG, RuntimeConfig};

pub const KEY_ROUTING_PROFILES: &str = "routingProfiles";
pub const MAX_ROUTING_MODELS: usize = 500;
pub const MAX_ROUTING_PROVIDERS: usize = 256;
pub const MAX_ROUTING_ACCOUNTS: usize = 10_000;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RoutingProviderScope { All, Selected }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_scope: Option<RoutingProviderScope>,
    pub providers: Vec<String>,
    #[serde(default)]
    pub disabled_providers: Vec<String>,
    #[serde(default)]
    pub account_order: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub excluded_accounts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticky: Option<String>,
}

impl RoutingProfile {
    pub fn scope(&self) -> RoutingProviderScope {
        // 旧名单无法区分主动移除和后来新增，缺少范围标记时保留原有限制。
        self.provider_scope.unwrap_or(RoutingProviderScope::Selected)
    }

    pub fn includes_provider(&self, provider: &str) -> bool {
        (self.scope() == RoutingProviderScope::All || self.providers.iter().any(|id| id == provider))
            && !self.disabled_providers.iter().any(|id| id == provider)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingProfiles {
    #[serde(default)]
    pub global: Option<RoutingProfile>,
    #[serde(default)]
    pub models: BTreeMap<String, RoutingProfile>,
    #[serde(default)]
    pub groups: BTreeMap<String, super::RoutingGroup>,
}

impl RoutingProfiles {
    pub fn group_for_model(&self, model: &str) -> Option<&super::RoutingGroup> {
        self.groups.values().find(|group| group.model.eq_ignore_ascii_case(model.trim()))
    }
    pub fn model_profile(&self, model: &str) -> Option<&RoutingProfile> {
        self.models.get(&model.trim().to_ascii_lowercase())
    }

    pub fn effective(&self, model: &str) -> Option<&RoutingProfile> {
        self.model_profile(model).or(self.global.as_ref())
    }

    pub fn settings_for(&self, model: &str, mut settings: super::RoutingSettings) -> super::RoutingSettings {
        for profile in [self.global.as_ref(), self.model_profile(model)].into_iter().flatten() {
            if let Some(strategy) = &profile.strategy { settings.strategy = super::RouteStrategy::from_id(strategy); }
            if let Some(sticky) = &profile.sticky { settings.sticky = super::StickyMode::from_id(sticky); }
        }
        if let Some(group) = self.group_for_model(model) { return group.settings(settings); }
        settings
    }

    pub fn scope<'a>(&self, model: &'a str) -> &'a str {
        if self.model_profile(model).is_some() { model } else { "" }
    }
}

pub(crate) fn from_raw(raw: &Map<String, Value>) -> Arc<RoutingProfiles> {
    let mut profiles = RoutingProfiles::default();
    let Some(object) = raw.get(KEY_ROUTING_PROFILES).and_then(Value::as_object) else {
        return Arc::new(profiles);
    };
    profiles.global = object.get("global")
        .and_then(|value| serde_json::from_value::<RoutingProfile>(value.clone()).ok())
        .filter(|profile| validate_shape(profile).is_ok());
    if let Some(models) = object.get("models").and_then(Value::as_object) {
        for (model, value) in models.iter().take(MAX_ROUTING_MODELS) {
            let Ok(model) = normalize_model(model) else { continue };
            if model.is_empty() { continue; }
            if let Ok(profile) = serde_json::from_value::<RoutingProfile>(value.clone()) {
                if validate_shape(&profile).is_ok() {
                    profiles.models.insert(model, profile);
                }
            }
        }
    }
    profiles.groups = super::routing_groups::from_value(object.get("groups"));
    Arc::new(profiles)
}

pub fn routing_profiles() -> Arc<RoutingProfiles> {
    CONFIG.read().ok()
        .and_then(|guard| guard.as_ref().map(|config| config.routing_profiles.clone()))
        .unwrap_or_default()
}

pub fn routing_snapshot() -> (Arc<RoutingProfiles>, super::RoutingSettings) {
    CONFIG.read().ok().and_then(|guard| guard.as_ref().map(|config| (config.routing_profiles.clone(), config.routing)))
        .unwrap_or_default()
}

pub fn settings_for_model(model: &str) -> super::RoutingSettings {
    CONFIG.read().ok().and_then(|guard| guard.as_ref().map(|config| config.routing_profiles.settings_for(model, config.routing)))
        .unwrap_or_default()
}

pub fn normalize_model(model: &str) -> Result<String, String> {
    let model = model.trim();
    if model.chars().count() > 256 || model.chars().any(char::is_control) {
        return Err("模型名不能超过 256 个字符或包含控制字符".to_string());
    }
    Ok(model.to_ascii_lowercase())
}

pub fn validate_shape(profile: &RoutingProfile) -> Result<(), String> {
    if profile.strategy.as_deref().is_some_and(|value| !["order", "rotate", "provider"].contains(&value)) {
        return Err("路由策略必须为 order、rotate 或 provider".to_string());
    }
    if profile.sticky.as_deref().is_some_and(|value| !["auto", "session", "turn", "off"].contains(&value)) {
        return Err("会话粘滞必须为 auto、session、turn 或 off".to_string());
    }
    fn unique(values: &[String], max: usize, label: &str) -> Result<(), String> {
        if values.len() > max {
            return Err(format!("{label}最多允许 {max} 项"));
        }
        let mut seen = HashSet::new();
        for value in values {
            if value.is_empty() || value.trim() != value || value.len() > 256
                || value.chars().any(char::is_control)
            {
                return Err(format!("{label}包含空白或非法标识"));
            }
            if !seen.insert(value) {
                return Err(format!("{label}包含重复项「{value}」"));
            }
        }
        Ok(())
    }
    unique(&profile.providers, MAX_ROUTING_PROVIDERS, "提供商列表")?;
    unique(&profile.disabled_providers, MAX_ROUTING_PROVIDERS, "停用提供商列表")?;
    unique(&profile.excluded_accounts, MAX_ROUTING_ACCOUNTS, "排除账号列表")?;
    if profile.scope() == RoutingProviderScope::Selected
        && profile.disabled_providers.iter().any(|id| !profile.providers.contains(id)) {
        return Err("停用提供商必须保留在当前路由列表中".to_string());
    }
    if profile.account_order.len() > MAX_ROUTING_PROVIDERS {
        return Err("账号顺序的提供商数量过多".to_string());
    }
    let mut all_ids = HashSet::new();
    for (provider, ids) in &profile.account_order {
        if !profile.providers.contains(provider) {
            return Err(format!("账号顺序中的提供商「{provider}」不在当前路由中"));
        }
        unique(ids, MAX_ROUTING_ACCOUNTS, "账号顺序")?;
        for id in ids {
            if !all_ids.insert(id) {
                return Err(format!("账号「{id}」出现在多个提供商中"));
            }
        }
    }
    if all_ids.len() > MAX_ROUTING_ACCOUNTS {
        return Err("路由账号数量过多".to_string());
    }
    Ok(())
}

pub fn save_routing_profile(model: &str, profile: Option<RoutingProfile>) -> Result<(), String> {
    let model = normalize_model(model)?;
    let profile = profile.map(|mut profile| {
        profile.provider_scope = Some(profile.scope());
        profile
    });
    if let Some(profile) = &profile { validate_shape(profile)?; }
    let mut result = Ok(());
    let saved = super::update(|config: &mut RuntimeConfig| {
        let mut next = (*config.routing_profiles).clone();
        if next.group_for_model(&model).is_some() { result = Err("此模型 ID 已被自定义路由组占用".into()); return; }
        if model.is_empty() {
            if next.global.as_ref().is_some_and(super::routing_workspace::global_accounts::has_restrictions)
                || profile.as_ref().is_some_and(super::routing_workspace::global_accounts::has_restrictions) {
                result = Err("全局默认已改为账号启停，请先转换旧版全局参与设置；模型独立路由仍可设置参与名单".into());
                return;
            }
            next.global = profile;
        } else if let Some(profile) = profile {
            if next.models.len() >= MAX_ROUTING_MODELS && !next.models.contains_key(&model) {
                result = Err(format!("最多允许 {MAX_ROUTING_MODELS} 个模型独立配置"));
                return;
            }
            next.models.insert(model, profile);
        } else {
            next.models.remove(&model);
        }
        match serde_json::to_value(&next) {
            Ok(value) => {
                config.raw.insert(KEY_ROUTING_PROFILES.to_string(), value);
                config.routing_profiles = Arc::new(next);
            }
            Err(error) => result = Err(format!("路由配置序列化失败: {error}")),
        }
    });
    result?;
    if !saved { return Err("路由配置落盘失败，本次修改仅在当前进程生效".to_string()); }
    Ok(())
}
