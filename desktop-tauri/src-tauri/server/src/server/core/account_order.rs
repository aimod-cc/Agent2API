use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use serde_json::{json, Value};

use crate::server::config::{self, AccountSortRule, AccountSortSettings, AddedSortDirection, CreditSortDirection};
use super::{routing, usage_records::UsageRecord};

type CreditFacts = HashMap<String, Arc<Vec<i64>>>;

fn credit_slot() -> &'static Mutex<CreditFacts> {
    static FACTS: OnceLock<Mutex<CreditFacts>> = OnceLock::new();
    FACTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|value| value.is_finite())
}

fn credit_expiries(usage: &Value) -> Vec<i64> {
    let mut expiries: Vec<i64> = usage.get("credits").and_then(Value::as_array)
        .into_iter().flatten().filter_map(|pack| {
            if ["snapshot", "record", "daily", "unlimited"].iter().any(|key| pack.get(key).and_then(Value::as_bool) == Some(true)) {
                return None;
            }
            if pack.get("remain").and_then(number).is_some_and(|remain| remain <= 0.0) { return None; }
            let expiry = pack.get("expireAt").and_then(number)?;
            (expiry > 0.0).then_some(expiry as i64)
        }).collect();
    expiries.sort_unstable();
    expiries.dedup();
    expiries
}

pub fn note_usage(id: &str, usage: &Value) {
    if let Ok(mut facts) = credit_slot().lock() { facts.insert(id.to_string(), Arc::new(credit_expiries(usage))); }
}

pub fn reload_usage(records: &HashMap<String, UsageRecord>) {
    let fresh = records.iter().filter_map(|(id, record)| {
        record.usage.as_ref().map(|usage| (id.clone(), Arc::new(credit_expiries(usage))))
    }).collect();
    if let Ok(mut facts) = credit_slot().lock() { *facts = fresh; }
}

pub fn supports_credit_expiry(provider: &str) -> bool {
    super::plugin_host::account_features_of(provider).map(|features| features.credit_expiry)
        .unwrap_or_else(|| matches!(provider, "workbuddy" | "workbuddy-intl" | "trae" | "officeace"))
}

pub fn rules_for(
    provider: &str,
    settings: &AccountSortSettings,
    credit_available: bool,
) -> Vec<AccountSortRule> {
    let mut rules = settings.get(provider).cloned().unwrap_or_else(|| vec![
        AccountSortRule::Credit {
            on: supports_credit_expiry(provider) && credit_available,
            dir: CreditSortDirection::Soon,
        },
        AccountSortRule::Added { on: false, dir: AddedSortDirection::New },
    ]);
    // Qoder 没有网页 Cookie 时就算旧设置里勾了到期规则也不该继续生效：
    // 所有账号都没有 expireAt，规则只会显示「到期 近→远」却实际回落到手动。
    // 这里只压有效规则，不改落库配置 —— 之后配置 Cookie 时原设置会自动恢复。
    if !credit_available {
        for rule in &mut rules {
            if let AccountSortRule::Credit { on, .. } = rule {
                *on = false;
            }
        }
    }
    rules
}

pub fn manual_groups(accounts: &[Value]) -> Vec<(String, Vec<Value>)> {
    let mut accounts = accounts.to_vec();
    accounts.sort_by_key(|account| (
        account.get("priority").and_then(Value::as_i64).unwrap_or(i64::MAX),
        account.get("addedAt").and_then(Value::as_i64).unwrap_or(0),
    ));
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    for account in accounts {
        let provider = routing::provider_of(&account).to_string();
        match groups.iter_mut().find(|(id, _)| *id == provider) {
            Some((_, members)) => members.push(account),
            None => groups.push((provider, vec![account])),
        }
    }
    groups
}

fn compare_expiry(left: Option<i64>, right: Option<i64>, direction: CreditSortDirection) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => match direction {
            CreditSortDirection::Soon => left.cmp(&right),
            CreditSortDirection::Late => right.cmp(&left),
        },
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSorting {
    pub updated_at: i64,
    pub configured: AccountSortSettings,
    pub rules: AccountSortSettings,
    pub provider_order: Vec<String>,
    pub account_order: BTreeMap<String, Vec<String>>,
    pub credit_expires_at: BTreeMap<String, Option<i64>>,
}

pub fn sorting(accounts: &[Value], now: i64) -> AccountSorting {
    let configured = config::account_sort_settings();
    let mut result = AccountSorting {
        updated_at: now,
        configured: (*configured).clone(), rules: BTreeMap::new(), provider_order: Vec::new(),
        account_order: BTreeMap::new(), credit_expires_at: BTreeMap::new(),
    };
    let facts = credit_slot().lock().map(|facts| facts.clone()).unwrap_or_default();
    for (provider, mut members) in manual_groups(accounts) {
        let credit_supported = supports_credit_expiry(&provider);
        // Qoder 的到期数据依赖账号级网页 Cookie。组内没有任何账号配置 Cookie 时，
        // 不要把「最快到期优先」当成默认/有效规则；其他家没有这个账号级前提，
        // 保持原行为（查一次余额后自然有数据）。
        let credit_available = if matches!(provider.as_str(), "qoder" | "qoder-intl") {
            members.iter().any(|account| {
                account.get("hasWebCookie").and_then(Value::as_bool) == Some(true)
            })
        } else {
            true
        };
        let rules = rules_for(&provider, &configured, credit_available);
        for account in &members {
            if let Some(id) = routing::account_id(account) {
                let expiry = facts.get(id).and_then(|expiries| expiries.iter().find(|expiry| **expiry > now)).copied();
                result.credit_expires_at.insert(id.to_string(), expiry);
            }
        }
        members.sort_by(|left, right| {
            for rule in &rules {
                let order = match rule {
                    AccountSortRule::Credit { on: true, dir } if credit_supported => {
                        let expiry = |account: &Value| routing::account_id(account)
                            .and_then(|id| result.credit_expires_at.get(id)).copied().flatten();
                        compare_expiry(expiry(left), expiry(right), *dir)
                    }
                    AccountSortRule::Added { on: true, dir } => {
                        let added = |account: &Value| account.get("addedAt").and_then(number).unwrap_or(0.0);
                        let order = added(left).total_cmp(&added(right));
                        match dir { AddedSortDirection::Old => order, AddedSortDirection::New => order.reverse() }
                    }
                    _ => Ordering::Equal,
                };
                if order != Ordering::Equal { return order; }
            }
            Ordering::Equal
        });
        result.provider_order.push(provider.clone());
        result.account_order.insert(provider.clone(), members.iter().filter_map(|account| routing::account_id(account).map(str::to_string)).collect());
        result.rules.insert(provider, rules);
    }
    result
}

pub fn ordered_accounts(accounts: &[Value], sorting: &AccountSorting) -> Vec<Value> {
    let by_id: HashMap<&str, &Value> = accounts.iter().filter_map(|account| routing::account_id(account).map(|id| (id, account))).collect();
    sorting.provider_order.iter().flat_map(|provider| sorting.account_order.get(provider).into_iter().flatten())
        .filter_map(|id| by_id.get(id.as_str()).map(|account| (*account).clone())).collect()
}

pub fn enrich_snapshot(snapshot: &mut Value) {
    let accounts = routing::accounts_of(snapshot);
    let sorting = sorting(&accounts, crate::server::logging::now_ms());
    snapshot["sorting"] = json!(sorting);
}
