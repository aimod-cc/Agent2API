use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use serde_json::{json, Value};

use crate::server::config::{self, AccountSortRule, AccountSortSettings, AddedSortDirection, BalanceSortDirection, CreditSortDirection};
use super::{account_order_balance::{self, AccountSortBalance, BalanceSortState}, routing, usage_records::UsageRecord};

struct UsageFacts {
    credit_expiries: Vec<i64>,
    credit_expiry_declared: bool,
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

fn credit_expiry_facts(usage: &Value) -> (Vec<i64>, bool) {
    let mut expiries = Vec::new();
    let mut declared = false;
    for pack in usage.get("credits").and_then(Value::as_array).into_iter().flatten() {
        if ["snapshot", "record", "daily", "unlimited"].iter().any(|key| pack.get(key).and_then(Value::as_bool) == Some(true)) {
            continue;
        }
        let Some(expiry) = pack.get("expireAt").and_then(number).filter(|value| *value > 0.0) else { continue; };
        declared = true;
        if pack.get("remain").and_then(number).is_some_and(|remain| remain <= 0.0) { continue; }
        expiries.push(expiry as i64);
    }
    for source in usage.get("quotaSources").and_then(Value::as_array).into_iter().flatten() {
        if source.get("unlimited").and_then(Value::as_bool) == Some(true)
            || source.get("localOnly").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(expiry) = source.get("expiresAt").and_then(number).filter(|value| *value > 0.0) else { continue; };
        declared = true;
        if source.get("expiresAmount").and_then(number).is_some_and(|value| value <= 0.0) { continue; }
        let remaining = source.get("remaining").and_then(number);
        if remaining.is_some_and(|value| value <= 0.0) { continue; }
        if remaining.is_none() && (source.get("remainingPercent").and_then(number).is_some_and(|value| value <= 0.0)
            || source.get("used").and_then(number).zip(source.get("total").and_then(number))
                .is_some_and(|(used, total)| total > 0.0 && used >= total))
        {
            continue;
        }
        expiries.push(expiry as i64);
    }
    for row in usage.get("windows").and_then(Value::as_array).into_iter().flatten() {
        if row.get("unlimited").and_then(Value::as_bool) == Some(true) { continue; }
        let Some(expiry) = row.get("expiresAt").and_then(number).filter(|value| *value > 0.0) else { continue; };
        declared = true;
        if row.get("expiresAmount").and_then(number).is_some_and(|value| value <= 0.0) { continue; }
        let remaining = row.get("remaining").and_then(number);
        let balance = row.get("balance").and_then(number);
        if remaining.is_some_and(|value| value <= 0.0) || balance.is_some_and(|value| value <= 0.0) { continue; }
        if remaining.is_none() && balance.is_none()
            && row.get("remainingPercent").and_then(number).is_some_and(|value| value <= 0.0)
        {
            continue;
        }
        expiries.push(expiry as i64);
    }
    expiries.sort_unstable();
    expiries.dedup();
    (expiries, declared)
}

fn usage_facts(usage: &Value, query_config: &str) -> Arc<UsageFacts> {
    let (credit_expiries, credit_expiry_declared) = credit_expiry_facts(usage);
    Arc::new(UsageFacts {
        credit_expiries,
        credit_expiry_declared,
        balance: AccountSortBalance::from_usage(usage),
        query_config: query_config.to_string(),
    })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CreditSortState { Ready, NoData, Unsupported }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSorting {
    pub updated_at: i64,
    pub configured: AccountSortSettings,
    pub rules: AccountSortSettings,
    pub provider_order: Vec<String>,
    pub account_order: BTreeMap<String, Vec<String>>,
    pub credit_expires_at: BTreeMap<String, Option<i64>>,
    pub credit_sort_states: BTreeMap<String, CreditSortState>,
    pub balances: BTreeMap<String, AccountSortBalance>,
    pub balance_sort_states: BTreeMap<String, BalanceSortState>,
}

impl AccountSorting {
    pub fn rule_enabled(&self, provider: &str, rule: &AccountSortRule) -> bool {
        match rule {
            AccountSortRule::Credit { on, .. } => *on && self.credit_sort_states.get(provider)
                .map_or_else(|| supports_credit_expiry(provider), |state| *state != CreditSortState::Unsupported),
            AccountSortRule::Balance { on, .. } => *on && self.balance_sort_states.get(provider)
                .map_or_else(|| super::usage_query::supports_usage_provider(provider), |state| *state != BalanceSortState::Unsupported),
            AccountSortRule::Added { on, .. } => *on,
        }
    }
}

pub fn sorting(accounts: &[Value], now: i64) -> AccountSorting {
    let configured = config::account_sort_settings();
    let facts = usage_slot().lock().map(|facts| facts.clone()).unwrap_or_default();
    sorting_with(accounts, now, &configured, &facts)
}

fn sorting_with(accounts: &[Value], now: i64, configured: &AccountSortSettings, facts: &AccountFacts) -> AccountSorting {
    let mut result = AccountSorting {
        updated_at: now,
        configured: configured.clone(), rules: BTreeMap::new(), provider_order: Vec::new(),
        account_order: BTreeMap::new(), credit_expires_at: BTreeMap::new(),
        credit_sort_states: BTreeMap::new(),
        balances: BTreeMap::new(), balance_sort_states: BTreeMap::new(),
    };
    for (provider, mut members) in manual_groups(accounts) {
        let credit_supported = supports_credit_expiry(&provider);
        let rules = rules_for(&provider, configured);
        let mut any_credit_ready = false;
        let mut any_credit_declared = false;
        for account in &members {
            if let Some(id) = routing::account_id(account) {
                let guest = account.get("isGuest") == Some(&Value::Bool(true));
                let fact = if guest { None } else {
                    facts.get(id).filter(|fact| fact.query_config.is_empty()
                        || account.get("usageConfigKey").and_then(Value::as_str).is_none_or(|key| key == fact.query_config))
                };
                let expiry = fact.and_then(|fact| fact.credit_expiries.iter().find(|expiry| **expiry > now)).copied();
                if expiry.is_some() { any_credit_ready = true; }
                if fact.is_some_and(|fact| fact.credit_expiry_declared) { any_credit_declared = true; }
                result.credit_expires_at.insert(id.to_string(), expiry);
                let balance = fact.map(|fact| fact.balance.clone()).unwrap_or_default();
                result.balances.insert(id.to_string(), balance);
            }
        }
        let credit_configured_on = configured.get(&provider)
            .is_some_and(|rules| rules.iter().any(|rule| matches!(rule, AccountSortRule::Credit { on: true, .. })));
        let credit_state = if any_credit_ready {
            CreditSortState::Ready
        } else if credit_supported || any_credit_declared
            || (credit_configured_on && super::usage_query::supports_usage_provider(&provider))
        {
            CreditSortState::NoData
        } else {
            CreditSortState::Unsupported
        };
        result.credit_sort_states.insert(provider.clone(), credit_state);
        let balance_state = account_order_balance::state(super::usage_query::supports_usage_provider(&provider),
            members.iter().filter_map(|account| routing::account_id(account).and_then(|id| result.balances.get(id))));
        result.balance_sort_states.insert(provider.clone(), balance_state);
        members.sort_by(|left, right| {
            for rule in &rules {
                let order = match rule {
                    AccountSortRule::Credit { on: true, dir } if credit_state == CreditSortState::Ready => {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, provider: &str, priority: i64) -> Value {
        json!({ "id": id, "provider": provider, "priority": priority, "addedAt": priority * 1000 })
    }

    #[test]
    fn window_and_quota_source_expiries_unlock_credit_sort_for_script_plugins() {
        let now = 1_700_000_000_000i64;
        let provider = "magpie-workbuddy";
        let accounts = vec![
            account("a1", provider, 1),
            account("a2", provider, 2),
            account("a3", provider, 3),
        ];
        let mut facts = AccountFacts::new();
        // a1: only cyclic resetsAt, no credit expiry
        facts.insert("a1".into(), usage_facts(&json!({
            "windows": [{ "key": "plan", "remaining": 200, "resetsAt": now + 10_000 }],
            "quotaSources": [{ "id": "window:plan", "remaining": 200, "resetsAt": now + 10_000 }],
        }), ""));
        // a2: later expiry in quotaSources + exhausted earlier window ignored
        facts.insert("a2".into(), usage_facts(&json!({
            "windows": [
                { "key": "spent", "remaining": 0, "expiresAt": now + 5_000, "expiresAmount": 50 },
                { "key": "zero-exp", "remaining": 100, "expiresAt": now + 6_000, "expiresAmount": 0 },
                { "key": "total", "remaining": 300, "expiresAt": now + 30_000, "expiresAmount": 100 },
            ],
            "quotaSources": [
                { "id": "window:total", "remaining": 300, "expiresAt": now + 30_000, "expiresAmount": 100 },
            ],
        }), ""));
        // a3: earliest expiry on an aside bonus window (not in quotaSources)
        facts.insert("a3".into(), usage_facts(&json!({
            "windows": [
                { "key": "plan", "remaining": 200, "resetsAt": now + 50_000 },
                { "key": "extra", "aside": true, "remaining": 80, "expiresAt": now + 15_000, "expiresAmount": 80 },
            ],
            "quotaSources": [
                { "id": "window:plan", "remaining": 200, "resetsAt": now + 50_000 },
            ],
        }), ""));

        // Default remains OFF so existing order is untouched until user opts in.
        let default_sort = sorting_with(&accounts, now, &AccountSortSettings::new(), &facts);
        assert_eq!(default_sort.credit_sort_states.get(provider), Some(&CreditSortState::Ready));
        assert_eq!(default_sort.account_order.get(provider).unwrap(), &vec!["a1", "a2", "a3"]);
        assert_eq!(default_sort.credit_expires_at.get("a1"), Some(&None));
        assert_eq!(default_sort.credit_expires_at.get("a2"), Some(&Some(now + 30_000)));
        assert_eq!(default_sort.credit_expires_at.get("a3"), Some(&Some(now + 15_000)));

        // Enabling credit/soon sorts earliest expiry first and places no-expiry accounts last.
        let mut configured = AccountSortSettings::new();
        configured.insert(provider.into(), vec![AccountSortRule::Credit { on: true, dir: CreditSortDirection::Soon }]);
        let enabled_sort = sorting_with(&accounts, now, &configured, &facts);
        assert_eq!(enabled_sort.account_order.get(provider).unwrap(), &vec!["a3", "a2", "a1"]);
        assert!(enabled_sort.rule_enabled(provider, &AccountSortRule::Credit { on: true, dir: CreditSortDirection::Soon }));

        // Once all expiring credits pass, provider degrades to NoData instead of Unsupported.
        let expired_sort = sorting_with(&accounts, now + 40_000, &configured, &facts);
        assert_eq!(expired_sort.credit_sort_states.get(provider), Some(&CreditSortState::NoData));
        assert!(expired_sort.rule_enabled(provider, &AccountSortRule::Credit { on: true, dir: CreditSortDirection::Soon }));
    }

    #[test]
    fn resets_only_script_plugin_stays_unsupported_for_credit_expiry() {
        let now = 1_700_000_000_000i64;
        let provider = "magpie-copilot";
        let accounts = vec![account("c1", provider, 1)];
        let mut facts = AccountFacts::new();
        facts.insert("c1".into(), usage_facts(&json!({
            "windows": [{ "key": "5h", "remainingPercent": 80, "resetsAt": now + 10_000 }],
            "quotaSources": [{ "id": "window:5h", "remainingPercent": 80, "resetsAt": now + 10_000 }],
        }), ""));

        let sort = sorting_with(&accounts, now, &AccountSortSettings::new(), &facts);
        assert_eq!(sort.credit_sort_states.get(provider), Some(&CreditSortState::Unsupported));
        assert_eq!(sort.credit_expires_at.get("c1"), Some(&None));
    }
}
