//! 路由页的轻量指标与历史回放；按请求模型隔离，不把映射目标的其它请求混进来。

use rusqlite::params;
use serde_json::{json, Value};

use super::{entry_json, sql, RequestStats};

impl RequestStats {
    pub fn routing_metrics(&self, model: Option<&str>) -> Value {
        let start = chrono::Local::now().date_naive().and_hms_opt(0, 0, 0)
            .and_then(|time| time.and_local_timezone(chrono::Local).earliest())
            .map(|time| time.timestamp_millis()).unwrap_or(0);
        let model = model.unwrap_or("");
        let loaded = {
            let guard = self.guard();
            self.with_conn(&guard, |conn| {
                let (running, requests, successful, duration): (i64, i64, i64, i64) = conn.query_row(
                    "SELECT COALESCE(SUM(status = 0 AND error IS NULL AND ts >= ?3), 0),
                     COALESCE(SUM(ts >= ?1 AND (status <> 0 OR error IS NOT NULL) AND is_test = 0), 0),
                     COALESCE(SUM(ts >= ?1 AND status >= 200 AND status < 300 AND error IS NULL AND is_test = 0), 0),
                     COALESCE(SUM(CASE WHEN ts >= ?1 AND status >= 200 AND status < 300 AND error IS NULL AND is_test = 0 THEN duration_ms ELSE 0 END), 0)
                     FROM requests WHERE (?2 = '' OR COALESCE(NULLIF(client_model, ''), model) = ?2 COLLATE NOCASE)
                     AND (ts >= ?1 OR (status = 0 AND error IS NULL AND ts >= ?3))",
                    params![start, model, crate::server::logging::process_started_at()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT account_id, COUNT(*) FROM requests WHERE status = 0 AND error IS NULL AND ts >= ?2
                     AND (?1 = '' OR COALESCE(NULLIF(client_model, ''), model) = ?1 COLLATE NOCASE) AND account_id <> '' GROUP BY account_id",
                )?;
                let mut counts = serde_json::Map::new();
                let mut rows = stmt.query(params![model, crate::server::logging::process_started_at()])?;
                while let Some(row) = rows.next()? {
                    counts.insert(row.get::<_, String>(0)?, Value::from(row.get::<_, i64>(1)?));
                }
                Ok(json!({
                    "running": running,
                    "todayRequests": requests,
                    "todaySuccessRate": if requests > 0 { Some(successful as f64 * 100.0 / requests as f64) } else { None },
                    "averageDurationMs": if successful > 0 { Some(duration / successful) } else { None },
                    "accountRunning": counts,
                }))
            })
        };
        loaded.unwrap_or_else(|| json!({ "available": false }))
    }

    pub fn routing_replays(&self, model: Option<&str>, offset: usize, limit: usize, status: &str) -> Value {
        let model = model.unwrap_or("");
        let limit = limit.clamp(1, 100);
        let status_clause = match status {
            "error" => " AND status <> 0 AND NOT (status >= 200 AND status < 300 AND error IS NULL)",
            "sticky" => " AND json_valid(route) AND json_extract(route, '$.stickyHit') = 1",
            _ => "",
        };
        let loaded = {
            let guard = self.guard();
            self.with_conn(&guard, |conn| {
                let matched: i64 = conn.query_row(
                    &format!("SELECT COUNT(*) FROM requests WHERE route IS NOT NULL
                     AND (?1 = '' OR model = ?1 COLLATE NOCASE){status_clause}"),
                    params![model], |row| row.get(0),
                )?;
                let query = format!(
                    "SELECT {} FROM requests WHERE route IS NOT NULL
                     AND (?1 = '' OR model = ?1 COLLATE NOCASE){status_clause}
                     ORDER BY ts DESC, row_id DESC LIMIT ?2 OFFSET ?3", sql::REQUEST_COLUMNS,
                );
                let mut stmt = conn.prepare(&query)?;
                let mut rows = stmt.query(params![model, limit as i64, offset as i64])?;
                let mut entries = Vec::new();
                while let Some(row) = rows.next()? { entries.push(entry_json(&sql::decode_request(row)?)); }
                Ok(json!({ "entries": entries, "matched": matched, "offset": offset, "limit": limit }))
            })
        };
        loaded.unwrap_or_else(|| json!({ "entries": [], "matched": 0, "available": false }))
    }
}
