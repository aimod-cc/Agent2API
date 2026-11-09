use std::sync::{Arc, OnceLock};

use agent2api_provider_protocol as proto;
use serde_json::Value;

use crate::server::request_stats::{NewRequestEntry, RequestStats};

static STATS: OnceLock<Arc<RequestStats>> = OnceLock::new();

pub fn install_stats(stats: Arc<RequestStats>) {
    let _ = STATS.set(stats);
}

pub(in crate::server::core::plugin_host) fn record(params: &Value) -> Result<Value, proto::WireError> {
    let provider = params["provider"].as_str().unwrap_or("");
    let account = params["account"].as_str().unwrap_or("");
    let model = params["model"].as_str().unwrap_or("");
    let kind = params["kind"].as_str().unwrap_or("");
    if model.is_empty() || model.len() > 200 || kind.is_empty() || kind.len() > 40
        || !kind.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)) {
        return Err(proto::WireError::with_code("插件用量报告无效", proto::WireError::BAD_PARAMS));
    }
    let Some(stats) = STATS.get() else {
        return Err(proto::WireError::with_code("请求统计尚未就绪", proto::WireError::INTERNAL));
    };
    let number = |key: &str| params[key].as_i64().unwrap_or(0).clamp(0, 1_000_000_000_000);
    let mut entry = NewRequestEntry::new(model, number("status"));
    entry.id = crate::server::core::protocol::random_id("plugin");
    entry.provider = Some(provider.to_string());
    entry.account_id = account.to_string();
    entry.account_name = crate::server::core::account_store::plugin_accounts::shared()
        .and_then(|store| store.route_account(account)).and_then(|value| value["name"].as_str().map(str::to_string)).unwrap_or_default();
    entry.upstream_model = model.to_string();
    entry.duration_ms = number("ms");
    entry.prompt_tokens = number("input").saturating_add(number("cacheRead")).saturating_add(number("cacheWrite"));
    entry.completion_tokens = number("output");
    entry.total_tokens = entry.prompt_tokens.saturating_add(entry.completion_tokens);
    entry.cache_read_tokens = number("cacheRead");
    entry.error = params["error"].as_str().filter(|text| !text.is_empty()).map(|text| text.chars().take(500).collect());
    entry.route = serde_json::json!({ "pluginUsage": { "kind": kind, "cacheWrite": number("cacheWrite") } });
    stats.record(entry);
    Ok(Value::Bool(true))
}
