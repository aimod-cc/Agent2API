use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use agent2api_provider_protocol as proto;
use serde_json::Value;

use crate::server::core::account_store::plugin_accounts;
use crate::server::core::providers::adapter::ProviderAdapter;

const KEY: &str = "host.catalog.v1";
static GATE: Mutex<()> = Mutex::new(());

struct Snapshot {
    revision: u64,
    at: Instant,
    accounts: BTreeMap<String, proto::ModelList>,
}
fn snapshots() -> &'static Mutex<HashMap<String, Arc<Snapshot>>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Arc<Snapshot>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn snapshot(provider: &str) -> Option<Arc<Snapshot>> {
    let store = plugin_accounts::shared()?;
    let revision = store.plugin_state_revision();
    if let Some(cached) = snapshots().lock().ok()?.get(provider)
        .filter(|cached| cached.revision == revision && cached.at.elapsed() < Duration::from_secs(1)).cloned() { return Some(cached); }
    let accounts = store.plugin_states_for_provider(provider, KEY).into_iter()
        .filter_map(|(id, state)| list(state["value"].clone()).ok().map(|list| (id, list))).collect();
    let current = Arc::new(Snapshot { revision, at: Instant::now(), accounts });
    snapshots().lock().ok()?.insert(provider.into(), current.clone());
    Some(current)
}

pub(crate) fn enabled(provider: &str) -> bool { super::has_capability(provider, "catalog.accounts.v1") }

pub(super) fn cached(provider: &str, account: &str) -> Result<Value, proto::WireError> {
    let store = plugin_accounts::shared().ok_or_else(|| super::error("账号存储尚未就绪"))?;
    let state = store.plugin_state(provider, account, KEY).map_err(super::error)?;
    Ok(state["value"].clone())
}

fn list(value: Value) -> Result<proto::ModelList, proto::WireError> {
    if value.to_string().len() > 256 * 1024 { return Err(super::error("账号目录超过大小限制")); }
    let list: proto::ModelList = serde_json::from_value(value).map_err(|_| super::errors::protocol())?;
    if !list.meta.remote || list.meta.refreshed_at < 0 || list.models.len() > 4096
        || list.models.iter().any(|model| model["id"].as_str().is_none_or(|id| id.trim().is_empty() || id.len() > 512)) {
        return Err(super::errors::protocol());
    }
    Ok(list)
}

pub(crate) fn save(provider: &str, account: &str, value: Value) -> Result<usize, proto::WireError> {
    let list = list(value)?;
    let _gate = GATE.lock().map_err(|_| super::error("目录写入不可用"))?;
    let store = plugin_accounts::shared().ok_or_else(|| super::error("账号存储尚未就绪"))?;
    let count = list.models.len();
    let old = store.plugin_state(provider, account, KEY).map_err(super::error)?;
    if old["value"]["meta"]["refreshed_at"].as_i64().is_some_and(|at| at > list.meta.refreshed_at) {
        return Ok(old["value"]["models"].as_array().map(Vec::len).unwrap_or(0));
    }
    let next = serde_json::to_value(&list).map_err(|_| super::errors::protocol())?;
    let result = store.plugin_state_compare_exchange(provider, account, KEY,
        old["revision"].as_u64().unwrap_or(0), 1, next).map_err(super::error)?;
    if result["updated"] != true { return Err(super::error("账号目录已被另一轮刷新，请重试")); }
    rebuild(provider);
    Ok(count)
}

fn merge(target: &mut Value, source: &Value) {
    for key in ["supportsToolCall", "supportsImages", "supportsVideo", "supportsReasoning"] {
        if source[key] == true { target[key] = Value::Bool(true); }
    }
    for key in ["maxInputTokens", "maxOutputTokens"] {
        if let Some(value) = source[key].as_u64() {
            if value > target[key].as_u64().unwrap_or(0) { target[key] = Value::from(value); }
        }
    }
    if let Some(levels) = source["reasoningEffortLevels"].as_array() {
        let mut combined = target["reasoningEffortLevels"].as_array().cloned().unwrap_or_default();
        for level in levels { if !combined.contains(level) { combined.push(level.clone()); } }
        target["reasoningEffortLevels"] = Value::Array(combined);
    }
}

pub(crate) fn aggregate(provider: &str) -> Option<proto::ModelList> {
    let snapshot = snapshot(provider)?;
    let mut models = BTreeMap::new();
    let mut at = 0;
    for list in snapshot.accounts.values() {
        at = at.max(list.meta.refreshed_at);
        for model in &list.models {
            if let Some(id) = model["id"].as_str() {
                let entry = models.entry(id.to_ascii_lowercase()).or_insert_with(|| model.clone());
                merge(entry, model);
            }
        }
    }
    Some(proto::ModelList { models: models.into_values().collect(),
        meta: proto::CatalogMeta { remote: true, refreshed_at: at } })
}

pub(crate) fn rebuild(provider: &str) {
    if let Ok(mut table) = snapshots().lock() { table.remove(provider); }
    if let Some(list) = aggregate(provider) { super::super::rpc_cache::set_remote(provider, list); }
}

pub(crate) fn route_allowed(account: &Value, model: &str) -> Option<bool> {
    let provider = account["provider"].as_str()?;
    if !enabled(provider) || model.is_empty() { return None; }
    let permissions = &account["pluginModelPermissions"];
    if permissions["known"] != true { return None; }
    Some(permissions["models"].as_array()?.iter().any(|id| id.as_str().is_some_and(|id| id.eq_ignore_ascii_case(model))))
}

pub(crate) fn allowed(provider: &str, account: &str, model: &str) -> Option<bool> {
    if !enabled(provider) || model.is_empty() { return None; }
    let snapshot = snapshot(provider)?;
    let list = snapshot.accounts.get(account)?;
    Some(list.models.iter().any(|entry| entry["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(model))))
}

pub(crate) async fn ensure(adapter: &super::super::RpcAdapter, account: &str, model: &str) -> Result<(), crate::server::errors::GatewayError> {
    if !enabled(adapter.provider_id()) { return Ok(()) }
    let store = plugin_accounts::shared().ok_or_else(|| crate::server::errors::GatewayError::with_status(503, "账号存储尚未就绪"))?;
    if allowed(adapter.provider_id(), account, model).is_none() {
        let _ = adapter.refresh_models(&store, account, false).await;
    }
    match allowed(adapter.provider_id(), account, model) {
        Some(true) => Ok(()),
        Some(false) => Err(crate::server::errors::GatewayError::with_status(403, "当前账号没有该模型的使用权限")
            .with_code("account_model_unavailable")),
        None => Err(crate::server::errors::GatewayError::with_status(503, "当前账号的模型权限尚未取得，未使用其他账号的目录代替")
            .with_code("account_model_permissions_unknown")),
    }
}

pub(crate) async fn refresh_all(adapter: &super::super::RpcAdapter, force: bool) -> crate::server::core::providers::adapter::ModelRefreshOutcome {
    use crate::server::core::providers::adapter::ModelRefreshOutcome;
    let Some(store) = plugin_accounts::shared() else { return ModelRefreshOutcome::failed("账号存储尚未就绪") };
    let accounts = store.accounts_for_provider(adapter.provider_id());
    let mut refreshed = false;
    let mut failures = Vec::new();
    for account in accounts {
        let Some(id) = account["id"].as_str() else { continue };
        let result = adapter.refresh_models(&store, id, force).await;
        refreshed |= result.refreshed;
        if let Some(message) = result.message { failures.push(message); }
    }
    rebuild(adapter.provider_id());
    let count = super::super::rpc_cache::models_for(adapter.provider_id()).map(|models| models.len()).unwrap_or(0);
    ModelRefreshOutcome { refreshed, count, message: (!failures.is_empty()).then(|| format!("部分账号目录未刷新：{}", failures.join("；"))) }
}
