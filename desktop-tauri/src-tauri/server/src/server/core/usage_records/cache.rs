use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use super::{database, row_to_record, UsageRecord, RECORD_COLUMNS};

struct CachedRecords {
    revision: u64,
    database: u64,
    external: i64,
    checked_at: Instant,
    records: Arc<HashMap<String, UsageRecord>>,
}

#[derive(Default)]
struct Cache {
    build: Mutex<()>,
    state: RwLock<Option<CachedRecords>>,
}

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default)
}

fn fresh(cache: &Cache, revision: u64, database: u64) -> Option<Arc<HashMap<String, UsageRecord>>> {
    let state = cache.state.read().ok()?;
    let cached = state.as_ref()?;
    (cached.revision == revision && cached.database == database
        && cached.checked_at.elapsed() < Duration::from_secs(1)).then(|| cached.records.clone())
}

pub(super) fn snapshot() -> Arc<HashMap<String, UsageRecord>> {
    let Some(db) = database() else { return Arc::new(HashMap::new()); };
    let cache = cache();
    let database = crate::server::db::query_cache_generation();
    if let Some(records) = fresh(cache, db.usage_revision(), database) { return records; }
    let _build = cache.build.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(records) = fresh(cache, db.usage_revision(), database) { return records; }
    let previous = cache.state.read().ok().and_then(|state| state.as_ref().map(|cached| {
        (cached.revision, cached.database, cached.external, cached.records.clone())
    }));
    let loaded = db.with(|conn| -> rusqlite::Result<_> {
        let external: i64 = conn.pragma_query_value(None, "data_version", |row| row.get(0))?;
        let revision = db.usage_revision();
        if let Some((old_revision, old_database, old_external, records)) = &previous {
            if *old_revision == revision && *old_database == database && *old_external == external {
                return Ok((revision, external, records.clone()));
            }
        }
        let mut statement = conn.prepare(&format!("SELECT account_id, {RECORD_COLUMNS} FROM account_usage_records"))?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>("account_id")?, row_to_record(row)?)))?;
        let records = rows.collect::<rusqlite::Result<HashMap<_, _>>>()?;
        Ok((revision, external, Arc::new(records)))
    }).and_then(Result::ok);
    let Some((revision, external, records)) = loaded else { return Arc::new(HashMap::new()); };
    if db.usage_revision() == revision && crate::server::db::query_cache_generation() == database {
        if let Ok(mut state) = cache.state.write() {
            *state = Some(CachedRecords { revision, database, external, checked_at: Instant::now(), records: records.clone() });
        }
    }
    records
}
