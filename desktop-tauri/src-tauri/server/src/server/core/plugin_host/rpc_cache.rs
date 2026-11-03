//! 插件模型目录的同步快照；成功刷新持久化，装配时恢复后再使用握手兜底。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use crate::server::core::providers::catalog_cache;
use crate::server::logging;

pub const SETTING_MODEL_CATALOG: &str = "modelCatalog";
// 非空信封只用于新增 scoped runtime；不会作为模型广告出去。
const ENVELOPE_ID: &str = "__scoped_plugin_catalog_v1";
const ENVELOPE_VERSION: u64 = 1;

#[derive(Clone, Default)]
struct Entry {
    remote: Option<proto::ModelList>,
    fallback: Vec<Value>,
}

fn table() -> &'static Mutex<HashMap<String, Entry>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn write_gate() -> &'static Mutex<()> {
    static GATE: OnceLock<Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(()))
}

fn scope(provider: &str) -> String {
    format!("plugin:{provider}")
}

fn from_cached(provider: &str, cached: catalog_cache::CachedCatalog) -> Option<proto::ModelList> {
    if crate::server::core::providers::is_scoped_runtime_provider(provider) && cached.models.len() == 1 {
        let envelope = &cached.models[0];
        if envelope.get("id").and_then(Value::as_str) == Some(ENVELOPE_ID) {
            if envelope.get("version").and_then(Value::as_u64) != Some(ENVELOPE_VERSION)
                || envelope.get("provider").and_then(Value::as_str) != Some(provider)
            { return None; }
            let mut list: proto::ModelList = serde_json::from_value(envelope.get("list")?.clone()).ok()?;
            if !list.meta.remote || list.meta.refreshed_at < 0 { return None; }
            list.models = valid_models(list.models, true);
            return Some(list); // 已确认的空目录也恢复，不复活静态或上次模型。
        }
    }
    let models = valid_models(cached.models, crate::server::core::providers::is_scoped_runtime_provider(provider));
    (!models.is_empty()).then_some(proto::ModelList {
        models,
        meta: proto::CatalogMeta { remote: true, refreshed_at: cached.fetched_at },
    })
}

fn restore(provider: &str) -> Option<proto::ModelList> {
    let key = scope(provider);
    if let Some(list) = catalog_cache::load(&key).and_then(|cached| from_cached(provider, cached)) {
        return Some(list);
    }
    let list = catalog_cache::load_legacy(provider).and_then(|cached| from_cached(provider, cached))?;
    // 写事务里再次检查新槽，避免与在线刷新竞争时覆盖它；旧槽保留供回退。
    catalog_cache::save_migrated(&key, catalog_cache::CachedCatalog {
        models: list.models,
        fetched_at: list.meta.refreshed_at,
    }).and_then(|cached| from_cached(provider, cached))
}

fn valid_models(models: Vec<Value>, scoped: bool) -> Vec<Value> {
    let mut claimed = HashSet::new();
    models.into_iter().filter(|model| {
        model.get("id").and_then(Value::as_str)
            .is_some_and(|id| (!scoped || id != ENVELOPE_ID) && !id.trim().is_empty() && claimed.insert(id.to_ascii_lowercase()))
    }).collect()
}

pub fn set_defaults(provider: &str, models: Vec<Value>) -> Option<(usize, i64)> {
    let _write = crate::server::core::providers::is_scoped_runtime_provider(provider)
        .then(|| write_gate().lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    let restored = restore(provider);
    crate::server::core::providers::catalog::invalidate();
    let mut map = table().lock().ok()?;
    let entry = map.entry(provider.to_string()).or_default();
    let recovered = if entry.remote.is_none() {
        let recovered = restored.as_ref().map(|list| (list.models.len(), list.meta.refreshed_at));
        entry.remote = restored;
        recovered
    } else {
        None
    };
    entry.fallback = valid_models(models, crate::server::core::providers::is_scoped_runtime_provider(provider));
    crate::server::core::providers::catalog::invalidate();
    recovered
}

pub fn set_remote(provider: &str, mut list: proto::ModelList) -> usize {
    let scoped = crate::server::core::providers::is_scoped_runtime_provider(provider);
    let _write = scoped.then(|| write_gate().lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    if scoped && !list.meta.remote {
        return 0; // 静态或失败目录不能在持久层被重新标成远程成功。
    }
    list.models = valid_models(list.models, scoped);
    if !scoped && list.models.is_empty() {
        return 0;
    }
    list.meta.remote = true;
    if list.meta.refreshed_at <= 0 {
        list.meta.refreshed_at = logging::now_ms();
    }
    let count = list.models.len();
    let saved = if scoped {
        vec![json!({ "id": ENVELOPE_ID, "version": ENVELOPE_VERSION, "provider": provider, "list": list })]
    } else { list.models.clone() };
    if !scoped {
        catalog_cache::save(&scope(provider), &saved, list.meta.refreshed_at);
        let Ok(mut map) = table().lock() else { return 0 };
        crate::server::core::providers::catalog::invalidate();
        map.entry(provider.to_string()).or_default().remote = Some(list);
        crate::server::core::providers::catalog::invalidate();
        return count;
    }
    let mut map = table().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::server::core::providers::catalog::invalidate();
    map.entry(provider.to_string()).or_default().remote = Some(list.clone());
    crate::server::core::providers::catalog::invalidate();
    // table 与 DB 不重叠持锁；scoped gate 仍保持跨请求的持久化写入顺序。
    drop(map);
    catalog_cache::save(&scope(provider), &saved, list.meta.refreshed_at);
    count
}

pub fn snapshot(provider: &str) -> Option<proto::ModelList> {
    let map = table().lock().ok()?;
    let entry = map.get(provider)?;
    entry.remote.clone().or_else(|| {
        (!entry.fallback.is_empty()).then(|| proto::ModelList {
            models: entry.fallback.clone(),
            meta: proto::CatalogMeta::default(),
        })
    })
}

pub fn models_for(provider: &str) -> Option<Vec<Value>> {
    snapshot(provider).map(|list| list.models)
}

pub fn meta(provider: &str) -> proto::CatalogMeta {
    let Ok(map) = table().lock() else { return proto::CatalogMeta::default() };
    map.get(provider).and_then(|entry| entry.remote.as_ref())
        .map(|list| list.meta.clone()).unwrap_or_default()
}

fn rename_list(models: &mut Vec<Value>, mapping: &HashMap<String, String>) -> usize {
    let mut changed = 0;
    for model in models.iter_mut() {
        let Some(id) = model.get("id").and_then(Value::as_str) else { continue };
        let Some(base) = mapping.get(&id.trim().to_ascii_lowercase()).cloned() else { continue };
        if !id.eq_ignore_ascii_case(&base) {
            model["id"] = Value::String(base);
            changed += 1;
        }
    }
    if changed > 0 {
        let mut seen = HashSet::new();
        models.retain(|model| {
            model
                .get("id")
                .and_then(Value::as_str)
                .map(|id| seen.insert(id.to_ascii_lowercase()))
                .unwrap_or(true)
        });
    }
    changed
}

/// 就地把插件缓存里的历史模型 ID 改成原始 ID（启动期后缀迁移用）。
/// 只改内存中的清单并持久化；真正的路由表由插件下一次刷新重建。
pub fn rename_models(provider: &str, mapping: &HashMap<String, String>) -> usize {
    if mapping.is_empty() {
        return 0;
    }
    let (changed, list) = {
        let Ok(mut map) = table().lock() else { return 0 };
        let Some(entry) = map.get_mut(provider) else { return 0 };
        let mut changed = 0;
        if let Some(remote) = entry.remote.as_mut() {
            changed += rename_list(&mut remote.models, mapping);
        }
        changed += rename_list(&mut entry.fallback, mapping);
        (changed, entry.remote.clone())
    };
    if changed > 0 {
        crate::server::core::providers::catalog::invalidate();
        if let Some(list) = list {
            set_remote(provider, list);
        }
        crate::server::core::providers::catalog::invalidate();
    }
    changed
}

pub fn forget(provider: &str) {
    if let Ok(mut map) = table().lock() {
        crate::server::core::providers::catalog::invalidate();
        map.remove(provider);
        crate::server::core::providers::catalog::invalidate();
    }
}

pub fn clear_all() {
    if let Ok(mut map) = table().lock() {
        crate::server::core::providers::catalog::invalidate();
        map.clear();
        crate::server::core::providers::catalog::invalidate();
    }
}
