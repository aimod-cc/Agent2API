use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use serde_json::{json, Value};

use crate::server::config::{self, AccountSortRule, AccountSortSettings, AddedSortDirection, BalanceSortDirection, CreditSortDirection};
use super::{account_order_balance::{self, AccountSortBalance, BalanceSortState}, routing, usage_records::UsageRecord};

struct UsageFacts {
    credit_expiries: Vec<i64>,
    balance: AccountSortBalance,
    query_config: String,
}

type AccountFacts = HashMap<String, Arc<UsageFacts>>;

fn usage_slot() -> &'static Mutex<AccountFacts> {
    static FACTS: OnceLock<Mutex<AccountFacts>> = OnceLock::new();
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

fn usage_facts(usage: &Value, query_config: &str) -> Arc<UsageFacts> {
    Arc::new(UsageFacts { credit_expiries: credit_expiries(usage), balance: AccountSortBalance::from_usage(usage), query_config: query_config.to_string() })
}

pub fn note_usage(id: &str, usage: &Value, query_config: &str) {
    if let Ok(mut facts) = usage_slot().lock() { facts.insert(id.to_string(), usage_facts(usage, query_config)); }
}

pub fn reload_usage(records: &HashMap<String, UsageRecord>) {
    let fresh = records.iter().filter_map(|(id, record)| {
        record.usage.as_ref().map(|usage| (id.clone(), usage_facts(usage, &record.query_config)))
    }).collect();
    if let Ok(mut facts) = usage_slot().lock() { *facts = fresh; }
}

pub fn supports_credit_expiry(provider: &str) -> bool {
    super::plugin_host::account_features_of(provider).map(|features| features.credit_expiry)
        .unwrap_or_else(|| matches!(provider, "workbuddy" | "workbuddy-intl" | "trae" | "officeace"))
}

pub fn rule_enabled(provider: &str, rule: &AccountSortRule) -> bool {
    match rule {
        AccountSortRule::Credit { on, .. } => *on && supports_credit_expiry(provider),
        AccountSortRule::Balance { on, .. } => *on && super::usage_query::supports_usage_provider(provider),
        AccountSortRule::Added { on, .. } => *on,
    }
}

pub fn rules_for(provider: &str, settings: &AccountSortSettings) -> Vec<AccountSortRule> {
    let defaults = vec![
        AccountSortRule::Credit { on: supports_credit_expiry(provider), dir: CreditSortDirection::Soon },
        AccountSortRule::Balance { on: false, dir: BalanceSortDirection::High },
        AccountSortRule::Added { on: false, dir: AddedSortDirection::New },
    ];
    let Some(configured) = settings.get(provider) else { return defaults; };
    let mut rules = configured.clone();
    for mut rule in defaults {
        if rules.iter().any(|current| current.key() == rule.key()) { continue; }
        match &mut rule {
            AccountSortRule::Credit { on, .. } | AccountSortRule::Balance { on, .. } | AccountSortRule::Added { on, .. } => *on = false,
        }
        rules.push(rule);
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
    pub balances: BTreeMap<String, AccountSortBalance>,
    pub balance_sort_states: BTreeMap<String, BalanceSortState>,
}

pub fn sorting(accounts: &[Value], now: i64) -> AccountSorting {
    let configured = config::account_sort_settings();
    let mut result = AccountSorting {
        updated_at: now,
        configured: (*configured).clone(), rules: BTreeMap::new(), provider_order: Vec::new(),
        account_order: BTreeMap::new(), credit_expires_at: BTreeMap::new(),
        balances: BTreeMap::new(), balance_sort_states: BTreeMap::new(),
    };
    let facts = usage_slot().lock().map(|facts| facts.clone()).unwrap_or_default();
    for (provider, mut members) in manual_groups(accounts) {
        let credit_supported = supports_credit_expiry(&provider);
        let rules = rules_for(&provider, &configured);
        for account in &members {
            if let Some(id) = routing::account_id(account) {
                let fact = facts.get(id).filter(|fact| fact.query_config.is_empty()
                    || account.get("usageConfigKey").and_then(Value::as_str).is_none_or(|key| key == fact.query_config));
                let expiry = fact.and_then(|fact| fact.credit_expiries.iter().find(|expiry| **expiry > now)).copied();
                result.credit_expires_at.insert(id.to_string(), expiry);
                let balance = if account.get("isGuest") == Some(&Value::Bool(true)) { AccountSortBalance::default() }
                    else { fact.map(|fact| fact.balance.clone()).unwrap_or_default() };
                result.balances.insert(id.to_string(), balance);
            }
        }
        let balance_state = account_order_balance::state(super::usage_query::supports_usage_provider(&provider),
            members.iter().filter_map(|account| routing::account_id(account).and_then(|id| result.balances.get(id))));
        result.balance_sort_states.insert(provider.clone(), balance_state);
        members.sort_by(|left, right| {
            for rule in &rules {
                let order = match rule {
                    AccountSortRule::Credit { on: true, dir } if credit_supported => {
                        let expiry = |account: &Value| routing::account_id(account)
                            .and_then(|id| result.credit_expires_at.get(id)).copied().flatten();
                        compare_expiry(expiry(left), expiry(right), *dir)
                    }
                    AccountSortRule::Balance { on: true, dir } if balance_state == BalanceSortState::Ready => {
                        let balance = |account: &Value| routing::account_id(account)
                            .and_then(|id| result.balances.get(id)).and_then(AccountSortBalance::comparable);
                        account_order_balance::compare(balance(left), balance(right), *dir)
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
