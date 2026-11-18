//! 旧原生领取桥：显式账号必须先有无副作用的能力确认，不能用真实 poll 探测。
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::sync::atomic::Ordering;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use super::{Host, unavailable, failure};

struct Capability {
    host: Weak<Host>,
    generation: u64,
    value: tokio::sync::OnceCell<bool>,
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Capability>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Capability>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

async fn scoped(host: &Arc<Host>, provider: &str, generation: u64) -> bool {
    let entry = {
        let Ok(mut cache) = cache().lock() else { return false; };
        cache.retain(|_, entry| entry.host.strong_count() > 0);
        if cache.get(provider).is_none_or(|entry| entry.generation != generation
            || entry.host.upgrade().is_none_or(|current| !Arc::ptr_eq(&current, host))) {
            cache.insert(provider.to_string(), Arc::new(Capability { host: Arc::downgrade(host), generation, value: tokio::sync::OnceCell::new() }));
        }
        cache.get(provider).cloned().unwrap()
    };
    let wait = entry.value.get_or_init(|| async {
        let allowed = host.call_coded("admin.action", json!({ "provider": provider, "account": "",
            "action": "claim.verification.capabilities", "payload": {} }), Duration::from_millis(1500)).await
            .is_ok_and(|value| value["accountScoped"] == true && value["schema"].as_u64() == Some(1));
        allowed && host.ready() && host.generation.load(Ordering::Acquire) == generation
    });
    match tokio::time::timeout(Duration::from_secs(2), wait).await {
        Ok(value) => *value,
        Err(_) => { let _ = entry.value.set(false); false }
    }
}

pub(in crate::server::core::plugin_host) async fn call(host: &Arc<Host>, provider: &str, account: &str,
    complete: bool, payload: &Value) -> Result<Value, proto::WireError> {
    if host.options.script || host.options.wasm.is_some() { return Err(failure(403, "forbidden", "当前执行体不是原生领取验证桥")); }
    let _lease = host.activity.acquire().ok_or_else(unavailable)?;
    host.ensure().await.map_err(|_| unavailable())?;
    let generation = host.generation.load(Ordering::Acquire);
    if !complete && !account.is_empty() && !scoped(host, provider, generation).await {
        return Ok(json!({ "supported": false, "active": false, "task": null }));
    }
    if !host.ready() || host.generation.load(Ordering::Acquire) != generation
        || super::super::super::host_for_provider(provider).is_none_or(|current| !Arc::ptr_eq(&current, host)) { return Err(unavailable()); }
    let action = if complete { "claim.verification.complete" } else { "claim.verification.poll" };
    let result = host.call_coded("admin.action", json!({ "provider": provider, "account": account,
        "action": action, "payload": payload }), Duration::from_secs(2)).await;
    let result = match result {
        Ok(value) => value,
        Err(error) if !complete && matches!(error.code.as_deref(), Some(proto::WireError::NO_METHOD | proto::WireError::UNSUPPORTED)) => {
            return Ok(json!({ "supported": false, "active": false, "task": null }));
        }
        Err(error) => return Err(error),
    };
    if !host.ready() || host.generation.load(Ordering::Acquire) != generation { return Err(unavailable()); }
    if !complete && !account.is_empty() && !result["task"].is_null()
        && (result["task"]["accountId"] != account || result["task"]["provider"] != provider) {
        return Err(failure(502, "plugin_protocol_error", "原生领取验证任务未遵守账号范围，结果未公开"));
    }
    Ok(result)
}
