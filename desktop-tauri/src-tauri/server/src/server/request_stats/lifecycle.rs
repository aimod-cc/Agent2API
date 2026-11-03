use rusqlite::{params, Connection};

use super::clock::{date_key, day_of, now_ms};
use super::record::{DailyEntry, RequestEntry};
use super::{daily, fold_into_daily, sql, RequestStats};

pub(super) fn add_to_daily(conn: &Connection, entry: &RequestEntry) -> rusqlite::Result<()> {
    if entry.is_test || entry.status == 0 { return Ok(()); }
    let date = date_key(day_of(entry.ts));
    let mut day = daily::select_daily_row(conn, &date)?.unwrap_or_else(|| DailyEntry::new(date));
    fold_into_daily(&mut day, entry);
    daily::upsert_daily(conn, &day)
}

/// 收尾与日报在同一个事务里完成；重复收尾不能把同一请求再累计一次。
pub(super) fn finish_stale(conn: &Connection, cutoff: i64, now: i64) -> rusqlite::Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM requests WHERE status = 0 AND ts < ?1)", params![cutoff], |row| row.get(0),
    )?;
    if !exists { return Ok(()); }
    super::source_backfill::backfill_sources(conn)?;
    let mut entries = {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM requests WHERE status = 0 AND ts < ?1", sql::REQUEST_COLUMNS,
        ))?;
        let rows = stmt.query_map(params![cutoff], sql::decode_request)?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    sql::finish_stale_running(conn, cutoff, now)?;
    for entry in &mut entries {
        entry.status = 408;
        entry.error = Some("网关重启或流中断，请求未能完成".to_string());
        entry.total_tokens = 0;
        entry.prompt_tokens = 0;
        entry.completion_tokens = 0;
        entry.cache_read_tokens = 0;
        entry.credit = None;
        add_to_daily(conn, entry)?;
    }
    Ok(())
}

impl RequestStats {
    pub fn finalize_interrupted(&self, id: &str, error: &str) {
        let id = id.trim();
        if id.is_empty() { return; }
        let bounds = self.retention_bounds();
        let guard = self.guard();
        let _ = self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction()?;
            let mut entries = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT {} FROM requests WHERE id = ?1 AND status = 0", sql::REQUEST_COLUMNS,
                ))?;
                let rows = stmt.query_map(params![id], sql::decode_request)?.collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            sql::finish_running_request(&tx, id, error, now_ms())?;
            for entry in &mut entries {
                entry.status = 408;
                entry.error = Some(error.to_string());
                entry.total_tokens = 0;
                entry.prompt_tokens = 0;
                entry.completion_tokens = 0;
                entry.cache_read_tokens = 0;
                entry.credit = None;
                add_to_daily(&tx, entry)?;
            }
            daily::delete_daily_before(&tx, &bounds.daily_key)?;
            tx.commit()
        });
    }
}
