use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::server::config::RoutingProfile;
use super::{account_order, routing};

pub fn global_queue(accounts: &[Value], profile: &RoutingProfile, account_provider: Option<&str>) -> Result<Vec<String>, String> {
    let sorting = account_order::sorting(accounts, crate::server::logging::now_ms());
    if let Some(provider) = account_provider {
        let automatic = sorting.rules.get(provider).into_iter().flatten()
            .any(|rule| account_order::rule_enabled(provider, rule));
        if automatic { return Err("该提供商已启用自动消耗排序，请先关闭排序规则再手动调整账号顺序".to_string()); }
    }
    let mut groups = account_order::manual_groups(accounts);
    if let Some(provider) = account_provider {
        let Some((_, members)) = groups.iter_mut().find(|(id, _)| id == provider) else {
            return Err("该提供商没有可重排的账号".to_string());
        };
        let Some(ids) = profile.account_order.get(provider) else {
            return Err("缺少该提供商的账号顺序".to_string());
        };
        let known: HashSet<&str> = members.iter().filter_map(routing::account_id).collect();
        let positions: HashMap<&str, usize> = ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
        if !known.iter().all(|id| positions.contains_key(id)) {
            return Err("账号顺序不完整，请刷新后重试".to_string());
        }
        members.sort_by_key(|account| routing::account_id(account)
            .and_then(|id| positions.get(id)).copied().unwrap_or(usize::MAX));
    } else {
        let participating: HashSet<&str> = profile.providers.iter().map(String::as_str).collect();
        let mut replacements: Vec<_> = groups.iter().filter(|(id, _)| participating.contains(id.as_str())).cloned().collect();
        replacements.sort_by_key(|(id, _)| profile.providers.iter().position(|provider| provider == id).unwrap_or(usize::MAX));
        let mut replacements = replacements.into_iter();
        for group in &mut groups {
            if participating.contains(group.0.as_str()) {
                if let Some(next) = replacements.next() { *group = next; }
            }
        }
    }
    Ok(groups.into_iter().flat_map(|(_, accounts)| accounts.into_iter().filter_map(|account| routing::account_id(&account).map(str::to_string))).collect())
}
