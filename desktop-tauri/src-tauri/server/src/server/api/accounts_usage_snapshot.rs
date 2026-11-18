use std::collections::HashSet;

use serde_json::{json, Value};

use crate::server::core::{account_store::AccountStore, limiter, routing, usage_query, usage_records};

pub(super) fn snapshot(store: &AccountStore, ids: &[String]) -> Value {
    let cached = store.routing_snapshot();
    let mut accounts = ids.iter().filter_map(|id| cached.public_by_id.get(id))
        .filter(|account| crate::server::core::account_store::store_view::account_visible(routing::provider_of(account)))
        .map(|account| account.as_ref().clone()).collect::<Vec<_>>();
    accounts.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    accounts.dedup_by(|left, right| left["id"] == right["id"]);
    let now = crate::server::logging::now_ms();
    let mut at = 0;
    let mut results = Vec::new();
    for account in &accounts {
        if account["isGuest"] == true { continue; }
        let id = routing::account_id(account).unwrap_or("");
        let record = usage_records::load(id);
        at = at.max(record.last_attempt_at);
        let stale = !record.query_config.is_empty() && account["usageConfigKey"].as_str() != Some(record.query_config.as_str());
        if let Some(error) = record.error.as_deref() {
            results.push(json!({ "id": id, "name": account["name"], "usage": null, "error": error,
                "code": record.code, "diagnostics": record.diagnostics, "at": record.last_attempt_at, "lastUsage": record.usage,
                "lastSuccessAt": record.last_success_at, "requestId": record.query_request_id, "stale": stale }));
        } else if let Some(usage) = record.usage {
            results.push(json!({ "id": id, "name": account["name"], "usage": usage, "error": null,
                "at": record.last_attempt_at, "lastSuccessAt": record.last_success_at, "requestId": record.query_request_id, "stale": stale }));
        }
    }
    let facts = limiter::token_facts();
    let sources = limiter::sources::facts();
    let counts = limiter::counter::facts();
    let mut token_usage = serde_json::Map::new();
    let mut states = serde_json::Map::new();
    let mut quotas = serde_json::Map::new();
    let mut models = serde_json::Map::new();
    let mut limited = serde_json::Map::new();
    for account in &accounts {
        let id = routing::account_id(account).unwrap_or("");
        let provider = routing::provider_of(account);
        token_usage.insert(id.to_string(), json!(limiter::token_usage_rows(account, &facts, now)));
        states.insert(id.to_string(), limiter::types::json_of(&limiter::evaluate::states(account, &sources, &counts, now)));
        quotas.insert(id.to_string(), limiter::sources::rows_for(account, &sources));
        models.entry(provider.to_string()).or_insert_with(|| json!(limiter::sources::account_models(provider)));
        limited.insert(id.to_string(), json!(crate::server::core::accounts_list::limited(account, &sources, &counts, now)));
    }
    let wanted = ids.iter().map(String::as_str).collect::<HashSet<_>>();
    let queries = usage_query::jobs::states(store).into_iter().filter(|query| query["id"].as_str().is_some_and(|id| wanted.contains(id))).collect::<Vec<_>>();
    json!({ "at": at, "queries": queries, "results": results, "skipped": 0, "tokenAt": now,
        "tokenUsage": token_usage, "limiterStates": states, "quotaSources": quotas, "limiterModels": models,
        "listLimited": limited, "partial": true })
}
