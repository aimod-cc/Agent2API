use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::{read_rows, AccountStore};

#[derive(Default)]
pub(super) struct SnapshotCache {
    build: Mutex<()>,
    state: RwLock<Option<CachedSnapshot>>,
}

struct CachedSnapshot {
    revision: u64,
    metadata: u64,
    data_version: i64,
    checked_at: Instant,
    rows: Arc<BTreeMap<i64, Arc<Value>>>,
    snapshot: Arc<RoutingSnapshot>,
}

pub(crate) struct RoutingSnapshot {
    pub accounts: Vec<Arc<Value>>,
    pub by_id: HashMap<String, Arc<Value>>,
    pub available_providers: HashSet<String>,
    pub forwarding_providers: HashSet<String>,
}

impl RoutingSnapshot {
    fn from_rows(rows: &BTreeMap<i64, Arc<Value>>) -> Self {
        let accounts: Vec<_> = rows.values().cloned().collect();
        let by_id = accounts.iter().filter_map(|account| {
            Some((account.get("id")?.as_str()?.to_string(), account.clone()))
        }).collect();
        let available_providers = accounts.iter().filter(|account| {
            account.get("enabled").and_then(Value::as_bool) != Some(false)
                && account.get("hasCredentials").and_then(Value::as_bool) == Some(true)
        }).filter_map(|account| account.get("provider")?.as_str().map(str::to_string)).collect();
        let forwarding_providers = accounts.iter().filter_map(|account| {
            account.get("provider")?.as_str().map(str::to_string)
        }).collect::<HashSet<_>>().into_iter()
            .filter(|provider| super::store::provider_forwards_requests(provider)).collect();
        Self { accounts, by_id, available_providers, forwarding_providers }
    }
}

impl AccountStore {
    pub(crate) fn routing_snapshot(&self) -> Arc<RoutingSnapshot> {
        let Some(db) = self.database() else { return empty() };
        let cache = self.snapshot_cache();
        let metadata = crate::server::core::providers::catalog::generation();
        if let Some(snapshot) = fresh(cache, db.account_revision(), metadata) { return snapshot; }
        crate::server::core::blocking::in_place(|| self.build_routing_snapshot(db))
    }

    fn build_routing_snapshot(&self, db: &crate::server::db::Db) -> Arc<RoutingSnapshot> {
        let cache = self.snapshot_cache();
        let _build = cache.build.lock().unwrap_or_else(|error| error.into_inner());
        let metadata = crate::server::core::providers::catalog::generation();
        if let Some(snapshot) = fresh(cache, db.account_revision(), metadata) { return snapshot; }
        let previous = cache.state.read().ok().and_then(|state| state.as_ref().map(|cached| {
            (cached.revision, cached.metadata, cached.data_version, cached.rows.clone())
        }));
        // 连接锁只复制密文行；解密、规则推导和轻量投影全部在锁外执行。
        let batch = db.with(|conn| -> rusqlite::Result<_> {
            let data_version: i64 = conn.pragma_query_value(None, "data_version", |row| row.get(0))?;
            let since = previous.as_ref().filter(|(_, old_metadata, old_version, _)| {
                *old_metadata == metadata && *old_version == data_version
            }).map(|(revision, _, _, _)| *revision);
            let changes = db.account_changes(since);
            let (replace, rows, unchanged) = match changes.rows {
                Some(ids) => {
                    let unchanged = ids.is_empty();
                    (false, read_rows::changed(conn, &ids)?, unchanged)
                }
                None => (true, read_rows::all(conn)?.into_iter().map(|row| (row.rowid, Some(row))).collect(), false),
            };
            Ok((changes.revision, data_version, replace, rows, unchanged))
        }).and_then(Result::ok);
        let Some((revision, data_version, replace, changes, unchanged)) = batch else { return empty() };
        if unchanged && db.account_revision() == revision
            && crate::server::core::providers::catalog::generation() == metadata {
            if let Ok(mut state) = cache.state.write() {
                if let Some(cached) = state.as_mut() {
                    cached.checked_at = Instant::now();
                    return cached.snapshot.clone();
                }
            }
        }
        let mut rows = if replace { BTreeMap::new() } else {
            previous.map(|(_, _, _, rows)| (*rows).clone()).unwrap_or_default()
        };
        let mut failed = 0;
        for (rowid, raw) in changes {
            rows.remove(&rowid);
            if let Some(raw) = raw {
                match read_rows::decode(raw) {
                    Ok((_, record)) => { rows.insert(rowid, Arc::new(route_account(&self.public_account(&record)))); }
                    Err(_) => failed += 1,
                }
            }
        }
        if failed > 0 {
            super::sql::warn_load_failure("读取选路账号", &format!("{failed} 条账号记录解密或解析失败，已跳过；原始数据未修改"));
        }
        let snapshot = Arc::new(RoutingSnapshot::from_rows(&rows));
        if db.account_revision() == revision && crate::server::core::providers::catalog::generation() == metadata {
            if let Ok(mut state) = cache.state.write() {
                *state = Some(CachedSnapshot { revision, metadata, data_version,
                    checked_at: Instant::now(), rows: Arc::new(rows), snapshot: snapshot.clone() });
            }
        }
        snapshot
    }

    pub(crate) fn route_account_ids_for_provider(&self, provider: &str) -> Vec<String> {
        let snapshot = self.routing_snapshot();
        let mut accounts: Vec<_> = snapshot.accounts.iter().filter(|account| {
            account.get("provider").and_then(Value::as_str) == Some(provider)
                && account.get("enabled").and_then(Value::as_bool) != Some(false)
                && account.get("hasCredentials").and_then(Value::as_bool) == Some(true)
        }).collect();
        accounts.sort_by_key(|account| super::store::route_order_key(account));
        accounts.into_iter().filter_map(|account| account.get("id")?.as_str().map(str::to_string)).collect()
    }

    pub(crate) fn route_account(&self, id: &str) -> Option<Arc<Value>> {
        self.routing_snapshot().by_id.get(id).cloned()
    }
}

fn fresh(cache: &SnapshotCache, revision: u64, metadata: u64) -> Option<Arc<RoutingSnapshot>> {
    let state = cache.state.read().ok()?;
    let cached = state.as_ref()?;
    (cached.revision == revision && cached.metadata == metadata
        && cached.checked_at.elapsed() < Duration::from_secs(1)).then(|| cached.snapshot.clone())
}

fn empty() -> Arc<RoutingSnapshot> {
    Arc::new(RoutingSnapshot::from_rows(&BTreeMap::new()))
}

// 复用各家公开投影的字段口径，只缓存选路和发送编排需要的部分。
fn route_account(account: &Value) -> Value {
    let mut public = Map::new();
    for key in [
        "id", "provider", "name", "nickname", "email", "userId", "uid", "account",
        "displayName", "priority", "addedAt", "enabled", "hasCredentials", "nameCustom",
        "needsRelogin", "allowRouteSkip", "maxConcurrent", "rateLimits", "limiterGroups",
        "upstreamCooldownUntil", "zcodePlan", "pool", "quotaChannel",
    ] {
        if let Some(value) = account.get(key) { public.insert(key.to_string(), value.clone()); }
    }
    let source = account.pointer("/proxy/config/source").and_then(Value::as_str).unwrap_or("");
    public.insert("proxy".to_string(), json!({ "config": { "source": source } }));
    Value::Object(public)
}
