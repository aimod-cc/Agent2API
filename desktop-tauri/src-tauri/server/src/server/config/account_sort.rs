use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{RuntimeConfig, CONFIG};

pub const KEY_ACCOUNT_SORT: &str = "accountSort";
pub type AccountSortSettings = BTreeMap<String, Vec<AccountSortRule>>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CreditSortDirection { Soon, Late }

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BalanceSortDirection { High, Low }

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AddedSortDirection { New, Old }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "key", rename_all = "camelCase", deny_unknown_fields)]
pub enum AccountSortRule {
    Credit { on: bool, dir: CreditSortDirection },
    Balance { on: bool, dir: BalanceSortDirection },
    Added { on: bool, dir: AddedSortDirection },
}

impl AccountSortRule {
    pub fn key(&self) -> &'static str {
        match self { Self::Credit { .. } => "credit", Self::Balance { .. } => "balance", Self::Added { .. } => "added" }
    }
}

pub fn validate_account_sort(provider: &str, rules: &[AccountSortRule]) -> Result<(), String> {
    if provider.is_empty() || provider.trim() != provider || provider.len() > 256
        || provider.chars().any(char::is_control)
    {
        return Err("提供商标识无效".to_string());
    }
    if rules.len() > 3 { return Err("账号排序最多允许三条规则".to_string()); }
    let mut keys = HashSet::new();
    for rule in rules {
        let key = rule.key();
        if !keys.insert(key) { return Err(format!("账号排序包含重复规则「{key}」")); }
    }
    Ok(())
}

pub(crate) fn from_raw(raw: &Map<String, Value>) -> Arc<AccountSortSettings> {
    let settings = raw.get(KEY_ACCOUNT_SORT).and_then(Value::as_object).map(|object| {
        object.iter().take(256).filter_map(|(provider, raw)| {
            let rules = serde_json::from_value::<Vec<AccountSortRule>>(raw.clone()).ok()?;
            validate_account_sort(provider, &rules).ok()?;
            Some((provider.clone(), rules))
        }).collect()
    }).unwrap_or_default();
    Arc::new(settings)
}

pub fn account_sort_settings() -> Arc<AccountSortSettings> {
    CONFIG.read().ok()
        .and_then(|guard| guard.as_ref().map(|config| config.account_sort.clone()))
        .unwrap_or_default()
}

pub fn save_account_sort(provider: &str, rules: Option<Vec<AccountSortRule>>, only_if_unset: bool) -> Result<(), String> {
    validate_account_sort(provider, rules.as_deref().unwrap_or_default())?;
    let mut error = None;
    let saved = super::update(|config: &mut RuntimeConfig| {
        let mut next = (*config.account_sort).clone();
        if only_if_unset && next.contains_key(provider) { return; }
        if let Some(rules) = rules {
            if next.len() >= 256 && !next.contains_key(provider) {
                error = Some("账号排序最多允许配置 256 家提供商".to_string());
                return;
            }
            next.insert(provider.to_string(), rules);
        } else {
            next.remove(provider);
        }
        match serde_json::to_value(&next) {
            Ok(value) => {
                config.raw.insert(KEY_ACCOUNT_SORT.to_string(), value);
                config.account_sort = Arc::new(next);
            }
            Err(cause) => error = Some(format!("账号排序序列化失败: {cause}")),
        }
    });
    if let Some(error) = error { return Err(error); }
    if !saved { return Err("账号排序落盘失败，本次修改仅在当前进程生效".to_string()); }
    Ok(())
}
