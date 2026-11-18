use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{account_store::AccountStore, limiter, plugin_host, providers, routing, usage_records};

mod balance;
mod balance_cache;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountListQuery {
    #[serde(default)] pub q: String,
    #[serde(default)] pub providers: Vec<String>,
    #[serde(default)] pub enabled: String,
    #[serde(default)] pub limit: String,
    #[serde(default)] pub status: String,
    #[serde(default)] pub expiry: String,
    #[serde(default)] pub usage: String,
    #[serde(default)] pub ids: Vec<String>,
    #[serde(default)] pub collapsed: Vec<String>,
    #[serde(default)] pub offset: usize,
    #[serde(default = "default_limit")] pub page_size: usize,
    #[serde(default)] pub mobile: bool,
    #[serde(default)] pub group_total_filtered: bool,
    #[serde(default)] pub metadata_only: bool,
    #[serde(default)] pub utc_offset_minutes: i32,
}

fn default_limit() -> usize { 50 }

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCounts {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub limited: usize,
    pub available: usize,
}

pub fn summary(store: &AccountStore) -> Value {
    let cached = store.routing_snapshot();
    let booting = plugin_host::startup::booting();
    let mut snapshot = store.account_list_snapshot(true, booting);
    let sources = limiter::sources::facts();
    let counters = limiter::counter::facts();
    let now = crate::server::logging::now_ms();
    let mut counts = AccountCounts::default();
    let mut list_limited = HashMap::new();
    for account in cached.public_accounts.iter().filter(|account| super::account_store::store_view::account_list_visible(routing::provider_of(account), booting)) {
        let decorated = store.decorate_policy_cooldown(account.as_ref().clone(), "");
        let limited = limited(&decorated, &sources, &counters, now);
        count(&mut counts, account, limited);
        if let Some(id) = routing::account_id(account) { list_limited.insert(id.to_string(), limited); }
    }
    if let Some(indexes) = snapshot.get_mut("accounts").and_then(Value::as_array_mut) {
        for account in indexes {
            let id = routing::account_id(account).unwrap_or("");
            if let Some(limited) = list_limited.get(id) { account["listLimited"] = json!(limited); }
        }
    }
    snapshot["summary"] = json!(counts);
    snapshot
}

pub fn page(store: &AccountStore, query: &AccountListQuery, ids_only: bool) -> Value {
    let booting = plugin_host::startup::booting();
    let mut snapshot = store.account_list_snapshot(true, booting);
    let cached = store.routing_snapshot();
    let mut accounts = cached.public_accounts.iter().filter(|account| super::account_store::store_view::account_list_visible(routing::provider_of(account), booting))
        .map(|account| account.as_ref()).collect::<Vec<_>>();
    let sorting = snapshot["sorting"].clone();
    let order = sorting["providerOrder"].as_array().into_iter().flatten().filter_map(Value::as_str)
        .flat_map(|provider| sorting["accountOrder"][provider].as_array().into_iter().flatten().filter_map(Value::as_str))
        .enumerate().map(|(index, id)| (id.to_string(), index)).collect::<HashMap<_, _>>();
    accounts.sort_by_key(|account| order.get(routing::account_id(account).unwrap_or("")).copied().unwrap_or(usize::MAX));
    let sources = limiter::sources::facts();
    let counters = limiter::counter::facts();
    let now = crate::server::logging::now_ms();
    let usage = usage_records::cached_snapshot();
    let features = features();
    let ids: HashSet<&str> = query.ids.iter().map(String::as_str).collect();
    let keyword = query.q.trim().to_lowercase();
    let mut summary = AccountCounts::default();
    let mut groups: Vec<Value> = Vec::new();
    let mut group_indexes = HashMap::new();
    let mut counts = json!({ "enabledAll": 0, "enabled": 0, "disabled": 0, "limitAll": 0, "normal": 0, "limited": 0 });
    let mut mobile_counts = json!({
        "status": { "all": 0, "enabled": 0, "disabled": 0, "limited": 0, "relogin": 0, "expired": 0 },
        "expiry": { "all": 0, "soon": 0, "expired": 0, "longlived": 0 },
        "usage": { "all": 0, "has": 0, "unqueried": 0, "failed": 0 },
    });
    let mut matched = Vec::new();
    let mut list_limited = HashMap::new();
    for account in &accounts {
        let provider = routing::provider_of(account);
        let enabled = account["enabled"] != false;
        let decorated = store.decorate_policy_cooldown((*account).clone(), "");
        let is_limited = limited(&decorated, &sources, &counters, now);
        if let Some(id) = routing::account_id(account) { list_limited.insert(id.to_string(), is_limited); }
        count(&mut summary, account, is_limited);
        let group_index = *group_indexes.entry(provider.to_string()).or_insert_with(|| {
            groups.push(json!({ "provider": provider, "rank": groups.len() + 1, "total": 0, "enabled": 0, "disabled": 0, "limited": 0, "matched": 0 }));
            groups.len() - 1
        });
        increment(&mut groups[group_index], "total");
        increment(&mut groups[group_index], if enabled { "enabled" } else { "disabled" });
        if enabled && is_limited { increment(&mut groups[group_index], "limited"); }
        let feature = &features[provider];
        let expiry = expires_at(account, feature);
        let expired = expiry > 0 && expiry <= now;
        let soon = expiry > now && expiry <= now + 7 * 86_400_000;
        let long_lived = feature["long_lived"] == true;
        let usage_state = usage_state(account, usage.get(routing::account_id(account).unwrap_or("")));
        let search = matches_query(account, &keyword, feature, query.mobile);
        if query.mobile && search {
            increment(&mut mobile_counts["status"], "all");
            if enabled && is_limited { increment(&mut mobile_counts["status"], "limited"); }
            if account["needsRelogin"] == true { increment(&mut mobile_counts["status"], "relogin"); }
            if expired { increment(&mut mobile_counts["status"], "expired"); }
            increment(&mut mobile_counts["expiry"], "all");
            if soon { increment(&mut mobile_counts["expiry"], "soon"); }
            if expired { increment(&mut mobile_counts["expiry"], "expired"); }
            if long_lived { increment(&mut mobile_counts["expiry"], "longlived"); }
            increment(&mut mobile_counts["usage"], "all");
            increment(&mut mobile_counts["usage"], usage_state);
        }
        let provider_match = query.providers.is_empty() || query.providers.iter().any(|id| id == provider);
        let enabled_match = match query.enabled.as_str() { "enabled" => enabled, "disabled" => !enabled, _ => true };
        if search && provider_match {
            increment(&mut counts, "enabledAll");
            increment(&mut counts, if enabled { "enabled" } else { "disabled" });
            if enabled_match {
                increment(&mut counts, "limitAll");
                if enabled { increment(&mut counts, if is_limited { "limited" } else { "normal" }); }
            }
        }
        let status_match = match query.status.as_str() {
            "enabled" => enabled, "disabled" => !enabled, "limited" => enabled && is_limited,
            "relogin" => account["needsRelogin"] == true, "expired" => expired, _ => true,
        };
        let expiry_match = match query.expiry.as_str() { "soon" => soon, "expired" => expired, "longlived" => long_lived, _ => true };
        let limit_match = match query.limit.as_str() { "limited" => enabled && is_limited, "normal" => enabled && !is_limited, _ => true };
        if search && provider_match && enabled_match && status_match && expiry_match && limit_match
            && (query.usage.is_empty() || query.usage == "all" || query.usage == usage_state)
            && (ids.is_empty() || ids.contains(routing::account_id(account).unwrap_or(""))) {
            increment(&mut groups[group_index], "matched");
            matched.push(*account);
        }
    }
    let total = matched.len();
    if ids_only {
        return json!({ "ids": matched.iter().filter_map(|account| routing::account_id(account)).collect::<Vec<_>>(), "total": total });
    }
    let balance_members = if query.group_total_filtered { &matched } else { &accounts };
    let mut balance_groups: HashMap<&str, Vec<std::sync::Arc<Value>>> = HashMap::new();
    for account in balance_members {
        let id = routing::account_id(account).unwrap_or("");
        if let Some(saved) = cached.public_by_id.get(id) {
            balance_groups.entry(routing::provider_of(account)).or_default().push(saved.clone());
        }
    }
    for group in &mut groups {
        let provider = group["provider"].as_str().unwrap_or("").to_string();
        let members = balance_groups.get(provider.as_str()).map(Vec::as_slice).unwrap_or(&[]);
        group["balance"] = balance_cache::summary(&provider, members, &usage, query.group_total_filtered, now, query.utc_offset_minutes);
    }
    let display = matched.iter().copied().filter(|account| !query.collapsed.iter().any(|id| id == routing::provider_of(account))).collect::<Vec<_>>();
    snapshot["matchedIds"] = json!(matched.iter().filter_map(|account| routing::account_id(account)).collect::<Vec<_>>());
    snapshot["displayIds"] = json!(display.iter().filter_map(|account| routing::account_id(account)).collect::<Vec<_>>());
    let display_total = display.len();
    let size = query.page_size.clamp(1, 200);
    let offset = if display_total == 0 { 0 } else { query.offset.min((display_total - 1) / size * size) };
    snapshot["accounts"] = if query.metadata_only { json!([]) } else { json!(display.into_iter().skip(offset).take(size).map(|account| {
        let mut account = store.decorate_policy_cooldown(super::account_store::refresh_public_account(account), "");
        if let Some(limited) = list_limited.get(routing::account_id(&account).unwrap_or("")) {
            account["listLimited"] = json!(limited);
        }
        account["detailsLoaded"] = Value::Bool(true);
        account
    }).collect::<Vec<_>>()) };
    snapshot["summary"] = json!(summary);
    snapshot["groups"] = json!(groups);
    snapshot["counts"] = if query.mobile { mobile_counts } else { counts };
    snapshot["total"] = json!(total);
    snapshot["displayTotal"] = json!(display_total);
    snapshot["offset"] = json!(offset);
    snapshot["pageSize"] = json!(size);
    snapshot["indexOnly"] = Value::Bool(false);
    snapshot
}

fn features() -> Value {
    let mut features = plugin_host::account_features();
    features.extend(providers::subscriptions::account_features());
    Value::Object(features)
}

fn count(counts: &mut AccountCounts, account: &Value, limited: bool) {
    counts.total += 1;
    if account["enabled"] == false { counts.disabled += 1; }
    else {
        counts.enabled += 1;
        if limited { counts.limited += 1; }
        if account["available"] != false { counts.available += 1; }
    }
}

fn increment(value: &mut Value, key: &str) { value[key] = json!(value[key].as_u64().unwrap_or(0) + 1); }

pub(crate) fn limited(account: &Value, sources: &limiter::sources::Facts, counters: &limiter::counter::Facts, now: i64) -> bool {
    account.pointer("/routingCooldowns/@account/resetAt").and_then(Value::as_i64).unwrap_or(0) > now
        || account.pointer("/providerCooldown/resetAt").and_then(Value::as_i64).unwrap_or(0) > now
        || account["upstreamCooldownUntil"].as_i64().unwrap_or(0) > now
        || account["rateLimits"].as_object().into_iter().flat_map(|limits| limits.values())
        .any(|limit| limit["resetAt"].as_i64().unwrap_or(0) > now)
        || limiter::groups::effective_groups(account).iter().any(|group| {
            group.enabled && group.action == limiter::types::Action::Skip
                && limiter::evaluate::state(account, group, sources, counters, now).hit
        })
}

fn expires_at(account: &Value, feature: &Value) -> i64 {
    let fallback = json!(["expiresAt", "tokenExpiresAt"]);
    let keys = feature.get("expiry").unwrap_or(&fallback);
    keys.as_array().into_iter().flatten().filter_map(Value::as_str).find_map(|key| {
        let value = account[key].as_f64().or_else(|| account[key].as_str()?.parse().ok())?;
        (value.is_finite() && value > 0.0).then_some(if value < 1e11 { (value * 1000.0) as i64 } else { value as i64 })
    }).unwrap_or(0)
}

fn matches_query(account: &Value, keyword: &str, feature: &Value, mobile: bool) -> bool {
    if keyword.is_empty() { return true; }
    let mut keys = vec!["name", "nickname", "displayName", "email", "id"];
    if mobile { keys.push("provider"); }
    let declared = feature["identifier"].as_array();
    if let Some(declared) = declared { keys.extend(declared.iter().filter_map(Value::as_str)); }
    else { keys.extend(["uid", "userId", "account", "loginName", "phone"]); }
    keys.into_iter().any(|key| account[key].as_str().is_some_and(|value| value.to_lowercase().contains(keyword)))
}

fn usage_state(account: &Value, record: Option<&usage_records::UsageRecord>) -> &'static str {
    if !super::usage_query::supports_usage(account) { return "unqueried"; }
    match record {
        Some(record) if record.error.is_some() => if matches!(record.code.as_deref(), Some("usage_not_configured" | "usage_unavailable")) { "unqueried" } else { "failed" },
        Some(record) if record.usage.is_some() => "has",
        _ => "unqueried",
    }
}
