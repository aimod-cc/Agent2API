use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};

use crate::server::db::Db;

use super::RequestStats;

const CACHE_LIFETIME: Duration = Duration::from_secs(5);
const BACKFILL_BATCH: i64 = 512;
const BACKFILL_PAUSE: Duration = Duration::from_millis(50);

type LastUsed = HashMap<String, i64>;

#[derive(Default)]
pub(super) struct ModelRecency {
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    values: Arc<LastUsed>,
    refreshed_at: Option<Instant>,
    backfilling: bool,
}

impl RequestStats {
    pub fn model_last_used(&self) -> Arc<LastUsed> {
        let Some(db) = self.db.as_ref() else { return Arc::new(LastUsed::new()) };
        self.model_recency.snapshot(db)
    }
}

impl ModelRecency {
    fn snapshot(self: &Arc<Self>, db: &Db) -> Arc<LastUsed> {
        let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        if cache.refreshed_at.is_some_and(|at| at.elapsed() < CACHE_LIFETIME) {
            return cache.values.clone();
        }
        cache.refreshed_at = Some(Instant::now());
        let pending = match db.with(load_snapshot) {
            Some(Ok((values, pending))) => {
                cache.values = Arc::new(values);
                pending
            }
            Some(Err(error)) => {
                eprintln!("[Stats] 读取模型最近使用记录失败: {error}");
                false
            }
            None => false,
        };
        let values = cache.values.clone();
        let start_backfill = pending && !cache.backfilling;
        if start_backfill { cache.backfilling = true; }
        drop(cache);
        if start_backfill { self.backfill(db.clone()); }
        values
    }

    fn backfill(self: &Arc<Self>, db: Db) {
        let recency = self.clone();
        crate::spawn_task(async move {
            loop {
                let db = db.clone();
                let result = tokio::task::spawn_blocking(move || db.with_mut(backfill_batch)).await;
                match result {
                    Ok(Some(Ok(true))) => tokio::time::sleep(BACKFILL_PAUSE).await,
                    Ok(Some(Ok(false))) => break,
                    Ok(Some(Err(error))) => {
                        eprintln!("[Stats] 补齐模型最近使用记录失败: {error}");
                        break;
                    }
                    Ok(None) => break,
                    Err(error) => {
                        eprintln!("[Stats] 模型最近使用记录后台任务失败: {error}");
                        break;
                    }
                }
            }
            let mut cache = recency.cache.lock().unwrap_or_else(|error| error.into_inner());
            cache.backfilling = false;
            cache.refreshed_at = None;
        });
    }
}

fn backfill_cursor(conn: &Connection) -> rusqlite::Result<i64> {
    Ok(conn.query_row(
        "SELECT before_row_id FROM model_usage_recency_backfill WHERE id = 1", [], |row| row.get(0),
    ).optional()?.unwrap_or(0))
}

fn load_snapshot(conn: &Connection) -> rusqlite::Result<(LastUsed, bool)> {
    let mut stmt = conn.prepare_cached("SELECT model, last_used_at FROM model_usage_recency")?;
    let values = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<LastUsed>>()?;
    Ok((values, backfill_cursor(conn)? > 0))
}

fn backfill_batch(conn: &mut Connection) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let before = backfill_cursor(&tx)?;
    if before <= 0 { return Ok(false); }
    // 按主键倒序分批，先恢复最近的模型；限制读取行数，不把正文或尝试明细搬进内存。
    let rows = {
        let mut stmt = tx.prepare_cached(
            "SELECT row_id, ts, model, upstream_model,
             provider <> '' AND (is_test = 0 OR client IN ('assistant', 'assistant_images'))
             FROM requests WHERE row_id <= ?1 ORDER BY row_id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![before, BACKFILL_BATCH], |row| Ok((
            row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?,
            row.get::<_, String>(3)?, row.get::<_, bool>(4)?,
        )))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let next = rows.last().map(|row| row.0.saturating_sub(1).max(0)).unwrap_or(0);
    let mut values = LastUsed::new();
    for (_, at, model, upstream, used) in rows {
        if !used || at <= 0 { continue; }
        for name in [model, upstream] {
            let name = name.trim().to_ascii_lowercase();
            if !name.is_empty() {
                let latest = values.entry(name).or_insert(at);
                *latest = (*latest).max(at);
            }
        }
    }
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO model_usage_recency (model, last_used_at) VALUES (?1, ?2)
             ON CONFLICT(model) DO UPDATE SET last_used_at = excluded.last_used_at
             WHERE excluded.last_used_at > model_usage_recency.last_used_at",
        )?;
        for (model, at) in values { stmt.execute(params![model, at])?; }
    }
    tx.execute("UPDATE model_usage_recency_backfill SET before_row_id = ?1 WHERE id = 1", params![next])?;
    tx.commit()?;
    Ok(next > 0)
}
