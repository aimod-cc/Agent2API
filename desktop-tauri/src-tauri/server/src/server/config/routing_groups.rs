//! 自定义对话路由组。组是独立入口，成员直指提供商的模型，不借用模型映射。

use std::collections::{BTreeMap, HashSet};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use super::{RoutingProfile, RoutingSettings};

pub const MAX_GROUPS: usize = 500;
pub const MAX_MEMBERS: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingMember {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub included: bool,
    pub accounts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingGroup {
    pub id: String,
    pub name: String,
    pub model: String,
    #[serde(default)]
    pub description: String,
    pub strategy: String,
    pub sticky: String,
    pub members: Vec<RoutingMember>,
}

impl RoutingGroup {
    pub fn settings(&self, mut base: RoutingSettings) -> RoutingSettings {
        base.strategy = super::RouteStrategy::from_id(&self.strategy);
        base.sticky = super::StickyMode::from_id(&self.sticky);
        base
    }

    pub fn profile(&self) -> RoutingProfile {
        let mut providers = Vec::new();
        for member in self.members.iter().filter(|member| member.included) {
            if !providers.contains(&member.provider) { providers.push(member.provider.clone()); }
        }
        RoutingProfile { provider_scope: Some(super::RoutingProviderScope::Selected), providers,
            strategy: Some(self.strategy.clone()), sticky: Some(self.sticky.clone()), ..Default::default() }
    }
}

pub fn revision(group: Option<&RoutingGroup>) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(&group).unwrap_or_default()))
}

pub fn validate(group: &RoutingGroup) -> Result<(), String> {
    fn identifier(value: &str) -> bool {
        !value.is_empty() && value.len() <= 128 && value.bytes().all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
    }
    if !identifier(&group.id) || group.members.iter().any(|member| !identifier(&member.id)) {
        return Err("路由组或成员标识无效".into());
    }
    if group.name.trim().is_empty() || group.name.chars().count() > 64 || group.name.chars().any(char::is_control)
        || group.description.chars().count() > 500 {
        return Err("组名不能为空或超过 64 个字符，说明不能超过 500 个字符".into());
    }
    if group.model.is_empty() || super::normalize_routing_model(&group.model)? != group.model {
        return Err("对外模型 ID 不能为空，请去掉首尾空格并使用小写英文字母".into());
    }
    super::validate_routing_profile(&group.profile())?;
    if group.members.len() > MAX_MEMBERS { return Err(format!("一个路由组最多允许 {MAX_MEMBERS} 个成员")); }
    let mut ids = HashSet::new();
    let mut pairs = HashSet::new();
    let mut count = 0;
    for member in &group.members {
        if !ids.insert(&member.id) || !pairs.insert((&member.provider, member.model.to_ascii_lowercase())) {
            return Err("同一提供商的同一模型不能重复加入，成员标识也不能重复".into());
        }
        if member.provider.is_empty() || member.provider.len() > 256 || member.provider.trim() != member.provider
            || member.provider.chars().any(char::is_control) || member.model.is_empty()
            || member.model.trim() != member.model || super::normalize_routing_model(&member.model).is_err() {
            return Err("成员的提供商或模型 ID 无效".into());
        }
        let unique: HashSet<_> = member.accounts.iter().collect();
        if unique.len() != member.accounts.len() || member.accounts.iter().any(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control)) {
            return Err("成员的参与账号为空、重复或格式不正确".into());
        }
        count += member.accounts.len();
    }
    if count > super::routing_profiles::MAX_ROUTING_ACCOUNTS { return Err("路由组的账号候选数量过多".into()); }
    Ok(())
}

pub(crate) fn from_value(value: Option<&Value>) -> BTreeMap<String, RoutingGroup> {
    value.and_then(Value::as_object).into_iter().flatten().take(MAX_GROUPS).filter_map(|(id, raw)| {
        let group: RoutingGroup = serde_json::from_value(raw.clone()).ok()?;
        (group.id == *id && validate(&group).is_ok()).then(|| (id.clone(), group))
    }).collect()
}

pub fn save(id: &str, group: Option<RoutingGroup>, expected: &str) -> Result<(), String> {
    if let Some(group) = &group {
        validate(group)?;
        if group.id != id { return Err("路由组标识不一致".into()); }
    }
    super::try_update_raw_field(super::KEY_ROUTING_PROFILES, |raw| {
        let mut raw_map = serde_json::Map::new();
        raw_map.insert(super::KEY_ROUTING_PROFILES.into(), raw.clone());
        let mut profiles = (*super::routing_profiles::from_raw(&raw_map)).clone();
        let old = profiles.groups.get(id);
        if revision(old) != expected { return Err("路由配置已在其他位置修改，请刷新后重试；草稿未丢失".to_string()); }
        if let Some(group) = &group {
            if old.is_none() && profiles.groups.len() >= MAX_GROUPS { return Err("自定义路由组数量已达上限".into()); }
            if profiles.groups.values().any(|known| known.id != id && known.model.eq_ignore_ascii_case(&group.model))
                || profiles.models.contains_key(&group.model)
                || !crate::server::core::providers::catalog::providers_for_model(&group.model).is_empty()
                || crate::server::core::model_rules::shared().mappings.iter().any(|mapping| mapping.alias.eq_ignore_ascii_case(&group.model))
                || crate::server::core::custom_providers::list().iter().any(|provider| provider.get("mappings").and_then(Value::as_array)
                    .is_some_and(|items| items.iter().any(|item| item.get("alias").and_then(Value::as_str).is_some_and(|alias| alias.eq_ignore_ascii_case(&group.model))))) {
                return Err("对外模型 ID 已被模型、映射或其他路由占用，请换一个名称".into());
            }
            for member in &group.members {
                if profiles.groups.values().any(|known| known.model.eq_ignore_ascii_case(&member.model)) || group.model.eq_ignore_ascii_case(&member.model) {
                    return Err("路由组不能包含自己或其他路由组".into());
                }
            }
            profiles.groups.insert(id.to_string(), group.clone());
        } else { profiles.groups.remove(id); }
        *raw = json!(profiles);
        Ok(())
    }, || "路由配置落盘失败，未应用本次修改".to_string())
}
