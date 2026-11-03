use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::{logging, logs_store};

pub(super) fn receive(store: &AccountStore, level: &str, message: &str, context: Value) {
    if message.trim().is_empty() {
        return;
    }
    logging::console_line("[Plugin]", message);
    let mut data = Map::new();
    data.insert("source".to_string(), json!("plugin"));
    for key in [
        "provider",
        "providerLabel",
        "account",
        "model",
        "action",
        "callId",
        "summary",
        "outcome",
        "status",
        "code",
        "durationMs",
    ] {
        if let Some(value) = context
            .get(key)
            .filter(|value| value.is_string() || value.is_number())
        {
            data.insert(key.to_string(), value.clone());
        }
    }
    let category = context
        .get("category")
        .and_then(Value::as_str)
        .map(logs_store::normalize_category)
        .unwrap_or_else(|| "plugin".to_string());
    let record = logs_store::prepare_entry(logs_store::LogEntry {
        id: 0,
        ts: 0,
        level: logs_store::normalize_level(level),
        category,
        message: format!("[Plugin] {}", message.trim()),
        data: Some(Value::Object(data)),
    });
    if record.level == "debug" && !logging::is_verbose() {
        return;
    }
    let mut data = record
        .data
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if let Some(provider) = data.get("provider").and_then(Value::as_str) {
        data.insert(
            "providerLabel".to_string(),
            json!(crate::server::core::providers::label_of(provider)),
        );
    }
    if let Some(account) = data.get("account").and_then(Value::as_str) {
        if let Some(item) = store.route_account(account).filter(|item| {
            item.get("provider").and_then(Value::as_str) == data.get("provider").and_then(Value::as_str)
                && item.get("enabled").and_then(Value::as_bool) != Some(false)
                && item.get("hasCredentials").and_then(Value::as_bool) == Some(true)
        }) {
            if let Some(name) = item.get("name").and_then(Value::as_str) {
                data.insert("accountName".to_string(), json!(name));
            }
        }
    }
    logging::log_event(
        &record.level,
        &record.category,
        &record.message,
        Some(Value::Object(data)),
    );
}
