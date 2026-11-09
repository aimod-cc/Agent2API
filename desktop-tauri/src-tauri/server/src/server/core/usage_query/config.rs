use std::sync::{Mutex, MutexGuard};
use serde_json::{json, Map, Value};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use crate::server::core::crypto::{self, SecretScope};

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::{custom_providers, proxies, proxy_pool};

static CONFIG_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock() -> MutexGuard<'static, ()> {
    CONFIG_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct QueryConfig {
    pub fingerprint: String,
    pub reading_key: String,
}

fn digest(value: &Value) -> String {
    let Ok(key) = crypto::secret_key(SecretScope::Accounts) else { return String::new(); };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&key) else { return String::new(); };
    mac.update(b"agent2api/usage-query/v1\0");
    mac.update(value.to_string().as_bytes());
    mac.finalize().into_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn select(fields: &Map<String, Value>, keys: &[&str]) -> Value {
    Value::Object(keys.iter().filter_map(|key| {
        fields.get(*key).filter(|value| !value.is_null()).map(|value| ((*key).to_string(), value.clone()))
    }).collect())
}

fn connection(proxy: &Value) -> Value {
    if proxy.get("source").and_then(Value::as_str) == Some("rotate") {
        let all = proxy.get("all").and_then(Value::as_bool) != Some(false);
        let ids = proxy.get("proxyIds").and_then(Value::as_array);
        let candidates: Vec<_> = proxy_pool::raw_items().into_iter().filter(|item| {
            proxy_pool::is_enabled(item) && (all || ids.is_some_and(|ids| ids.iter().any(|id| Some(id) == item.get("id"))))
        }).map(|item| connection(&json!({ "source": "pool", "proxyId": item["id"] }))).collect();
        return json!({ "source": "rotate", "candidates": candidates });
    }
    match proxies::resolve_account_proxy(Some(proxy)) {
        None => Value::Null,
        Some(proxies::ProxyResolution::Resolved(proxy)) => json!({
            "protocol": proxy.protocol, "host": proxy.host, "port": proxy.port,
            "username": proxy.username, "password": proxy.password, "nonDirect": proxy.non_direct,
        }),
        Some(proxies::ProxyResolution::Failed(_)) => json!({ "unavailable": true, "config": proxy }),
    }
}

pub(crate) fn reading_key(record: &StoredAccount) -> String { digest(&reading(record)) }

fn reading(record: &StoredAccount) -> Value {
    let provider_id = record.provider();
    let provider = custom_providers::get(&provider_id).unwrap_or(Value::Null);
    let mut reading = select(record.fields(), &[
        "id", "provider", "addedAt", "desktop", "edition", "domain", "endpoint", "prefixPath",
        "baseUrl", "apiKey", "noAuth", "balanceToken", "balanceQuery", "zcodePlan", "pool",
        "uid", "userId", "account", "enterpriseId", "organizationId", "tenantId", "usageCredentialRevision",
    ]);
    // 可刷新的访问令牌不是账号身份；续期本身不能让正在查询的结果失效。
    if record.refresh_token().is_empty() && record.get("pluginSecret").is_none() {
        reading["accessToken"] = record.get("accessToken").cloned().unwrap_or(Value::Null);
    }
    if let Some(public) = record.get("pluginPublic").and_then(Value::as_object) {
        reading["pluginIdentity"] = select(public, &["uid", "userId", "account", "edition", "pool", "tenantId"]);
    }
    if let Some(fields) = provider.as_object() {
        reading["provider"] = select(fields, &["id", "balanceScheme", "protocol", "headers", "urlSuffix"]);
        if record.get("baseUrl").and_then(Value::as_str).is_none_or(|value| value.trim().is_empty()) {
            reading["provider"]["baseUrl"] = provider["baseUrl"].clone();
        }
    }
    reading
}

pub(crate) fn fingerprint(record: &StoredAccount) -> String { of(record).fingerprint }

pub(super) fn of(record: &StoredAccount) -> QueryConfig {
    let reading = reading(record);
    let reading_key = digest(&reading);
    let proxy = record.proxy();
    let fingerprint = digest(&json!({
        "reading": reading,
        "proxy": connection(&proxy),
        "proxyConfig": proxy.as_object().map(|fields| select(fields, &["source", "proxyId", "proxyIds", "all", "listenerUid"])),
        "userAgent": record.get("userAgent"),
        "timeoutSeconds": crate::server::core::usage_records::query_timeout_of(record.get("usageQuery")),
        "retries": crate::server::core::usage_records::query_retries_of(record.get("usageQuery")),
    }));
    QueryConfig { fingerprint, reading_key }
}
