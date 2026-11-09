use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use super::types::{json_of, QuotaSource};

#[derive(Clone, Debug, Default)]
pub struct SourceFacts {
    pub sources: Vec<QuotaSource>,
    pub remaining: Option<f64>,
    pub unlimited: bool,
    pub unit: String,
    pub query_config: String,
}

pub type Facts = HashMap<String, SourceFacts>;

fn slot() -> &'static Mutex<Facts> {
    static FACTS: OnceLock<Mutex<Facts>> = OnceLock::new();
    FACTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn facts() -> Facts { slot().lock().map(|facts| facts.clone()).unwrap_or_default() }

pub fn parse(usage: &Value) -> SourceFacts {
    let sources = usage.get("quotaSources").and_then(Value::as_array).map(|rows| {
        rows.iter().filter_map(|row| serde_json::from_value::<QuotaSource>(row.clone()).ok())
            .filter(|source| !source.id.is_empty() && source.id.len() <= 512).collect()
    }).unwrap_or_default();
    let (remaining, unlimited) = crate::server::core::usage_records::extract_remaining(usage);
    let unit = usage.get("unit").and_then(Value::as_str).unwrap_or_else(|| {
        if usage.get("totalLeft").is_some() { "积分" } else { "" }
    }).trim().to_string();
    SourceFacts { sources, remaining, unlimited, unit, query_config: String::new() }
}

pub fn note(id: &str, usage: &Value, query_config: &str) {
    let mut value = parse(usage);
    value.query_config = query_config.to_string();
    if let Ok(mut facts) = slot().lock() { facts.insert(id.to_string(), value); }
}

pub fn current_for(account: &Value, facts: &SourceFacts) -> bool {
    facts.query_config.is_empty() || account.get("usageConfigKey").and_then(Value::as_str)
        .is_none_or(|key| key == facts.query_config)
}

pub fn reload() {
    let fresh = crate::server::core::usage_records::load_all().into_iter()
        .filter_map(|(id, row)| row.usage.as_ref().map(|usage| {
            let mut facts = parse(usage);
            facts.query_config = row.query_config.clone();
            (id, facts)
        })).collect();
    if let Ok(mut facts) = slot().lock() { *facts = fresh; }
}

pub fn rows_for(account: &Value, facts: &Facts) -> Value {
    let id = account.get("id").and_then(Value::as_str).unwrap_or("");
    let provider = crate::server::core::routing::provider_of(account);
    let record = facts.get(id).filter(|facts| current_for(account, facts));
    let sources = record.map(|facts| facts.sources.as_slice()).unwrap_or_default();
    let mut rows = json_of(&sources).as_array().cloned().unwrap_or_default();
    let has_balance = crate::server::core::usage_query::supports_usage_provider(provider);
    if has_balance {
        let unit = record.map(|facts| facts.unit.as_str()).filter(|unit| !unit.is_empty())
            .or_else(|| sources.iter().find(|source| source.id == "account").map(|source| source.unit.as_str()))
            .unwrap_or("");
        let credit = unit == "积分" || unit.eq_ignore_ascii_case("credits");
        let label = if credit { "总积分" } else { "账号主额度" };
        if let Some(account) = rows.iter_mut().find(|source| source.get("id").and_then(Value::as_str) == Some("account")) {
            if credit { account["label"] = json!(label); }
            account["unit"] = json!(unit);
        } else {
            rows.insert(0, json!({ "id": "account", "label": label, "provider": provider,
                "unit": unit, "remaining": record.and_then(|facts| facts.remaining),
                "unlimited": record.is_some_and(|facts| facts.unlimited) }));
        }
    }
    if provider == crate::server::core::account_store::codearts_accounts::CODEARTS_PROVIDER_ID {
        rows.retain(|source| source.get("id").and_then(Value::as_str) == Some("account")
            || source.get("unit").and_then(Value::as_str) == Some("积分"));
    } else {
        rows.push(json!({ "id": "local", "label": "本地用量保护", "provider": provider,
            "localOnly": true, "localEstimate": true }));
    }
    Value::Array(rows)
}

pub fn account_models(provider: &str) -> Vec<Value> {
    let models = if crate::server::core::custom_providers::is_custom_provider_id(provider) {
        crate::server::core::custom_providers::models_of(provider)
    } else {
        crate::server::core::providers::catalog::manifest_for_provider(provider)
    };
    models.iter().filter_map(|model| {
        let id = model.get("id").and_then(Value::as_str)?;
        Some(json!({ "id": id, "name": model.get("name").and_then(Value::as_str).unwrap_or(id) }))
    }).collect()
}
