use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::{entry_json, sql, RequestStats};

const BASE: &str = "WITH flow AS (
    SELECT row_id, id, ts, client, provider, account_id, account_name, status, error, attempts, route,
      COALESCE(NULLIF(client_model, ''), model, '') AS request_model,
      CASE WHEN json_valid(route) THEN COALESCE(json_extract(route, '$.sessionId'), json_extract(route, '$.stickyId'), '') ELSE '' END AS session_id,
      CASE WHEN json_valid(route) THEN COALESCE(json_extract(route, '$.sessionSource'), '') ELSE '' END AS session_source,
      (status = 0 AND error IS NULL) AS running,
      CASE WHEN status = 0 AND error IS NULL THEN ?1 ELSE ts + MAX(duration_ms, 0) END AS last_seen
    FROM requests
    WHERE (status <> 0 OR error IS NOT NULL OR ts >= ?8)
      AND ((status = 0 AND error IS NULL) OR (?4 = 0 AND ts + MAX(duration_ms, 0) >= ?2))
  ), selected AS (
    SELECT * FROM flow WHERE (?3 = '' OR request_model = ?3 COLLATE NOCASE)
      AND (?5 = '' OR instr(lower(client || ' ' || request_model || ' ' || provider || ' ' || account_name || ' ' || id || ' ' || session_id), lower(?5)) > 0)
      AND (?6 = '' OR json_array(client, request_model, provider, account_id, CASE WHEN account_id = '' THEN account_name ELSE '' END) = ?6)
      AND (?7 = '' OR session_id = ?7)
      AND (?9 = 0 OR client = ?10)
      AND (?11 = '' OR provider = ?11)
      AND (?12 = '' OR account_id = ?12)
      AND (?13 = '' OR instr(lower(session_id || ' ' || account_name), lower(?13)) > 0)
  )";

pub struct FlowQuery {
    pub model: String,
    pub minutes: i64,
    pub running_only: bool,
    pub search: String,
    pub session_search: String,
    pub path: String,
    pub session: String,
    pub client: Option<String>,
    pub provider: String,
    pub account: String,
    pub activity: String,
    pub offset: usize,
    pub limit: usize,
}

fn value_at(conn: &Connection, row_id: i64) -> rusqlite::Result<Value> {
    conn.query_row(&format!("SELECT {}, row_id AS trace_key FROM requests WHERE row_id = ?1", sql::REQUEST_COLUMNS),
        params![row_id], |row| {
            let mut value = entry_json(&sql::decode_request(row)?);
            value["traceKey"] = row_id.to_string().into();
            Ok(value)
        })
}

impl RequestStats {
    pub fn routing_flow(&self, query: &FlowQuery, detail: Option<&str>) -> Value {
        let now = crate::server::logging::now_ms();
        let since = now.saturating_sub(query.minutes * 60_000);
        let loaded = {
            let guard = self.guard();
            self.with_conn(&guard, |conn| {
                let parameters = params![now, since, query.model, query.running_only, query.search, query.path,
                    query.session, crate::server::logging::process_started_at(), query.client.is_some(), query.client.as_deref().unwrap_or(""), query.provider, query.account, query.session_search];
                if detail == Some("requests") {
                    let matched: i64 = conn.query_row(&format!("{BASE} SELECT COUNT(*) FROM selected"), parameters, |row| row.get(0))?;
                    let mut statement = conn.prepare(&format!("{BASE} SELECT row_id FROM selected ORDER BY running DESC, ts DESC, row_id DESC LIMIT {} OFFSET {}",
                        query.limit.min(100), query.offset.min(i64::MAX as usize)))?;
                    let ids = statement.query_map(parameters, |row| row.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    let entries = ids.into_iter().map(|id| value_at(conn, id)).collect::<rusqlite::Result<Vec<_>>>()?;
                    return Ok(json!({ "available": true, "entries": entries, "matched": matched, "now": now }));
                }
                if detail == Some("sessions") {
                    let activity = match query.activity.as_str() {
                        "running" => " HAVING EXISTS (SELECT 1 FROM flow f WHERE f.session_id = selected.session_id AND f.request_model = selected.request_model COLLATE NOCASE AND f.running = 1)",
                        "idle" => " HAVING NOT EXISTS (SELECT 1 FROM flow f WHERE f.session_id = selected.session_id AND f.request_model = selected.request_model COLLATE NOCASE AND f.running = 1)",
                        _ => "",
                    };
                    let matched: i64 = conn.query_row(&format!("{BASE} SELECT COUNT(*) FROM (SELECT request_model, session_id FROM selected WHERE session_id <> '' GROUP BY request_model, session_id{activity})"), parameters, |row| row.get(0))?;
                    let unknown: i64 = conn.query_row(&format!("{BASE} SELECT COUNT(*) FROM selected WHERE session_id = ''"), parameters, |row| row.get(0))?;
                    let statement = format!("{BASE}, sessions AS (
                        SELECT request_model, session_id, COUNT(*) AS request_count FROM selected
                        WHERE session_id <> '' GROUP BY request_model, session_id{activity}
                      ) SELECT s.request_model, s.session_id, s.request_count, SUM(f.running), MAX(f.last_seen),
                        (SELECT row_id FROM flow latest WHERE latest.session_id = s.session_id AND latest.request_model = s.request_model COLLATE NOCASE ORDER BY running DESC, ts DESC, row_id DESC LIMIT 1)
                      FROM sessions s JOIN flow f ON f.session_id = s.session_id AND f.request_model = s.request_model COLLATE NOCASE
                      GROUP BY s.request_model, s.session_id ORDER BY SUM(f.running) DESC, MAX(f.last_seen) DESC, s.session_id
                      LIMIT {} OFFSET {}", query.limit.min(100), query.offset.min(i64::MAX as usize));
                    let mut statement = conn.prepare(&statement)?;
                    let rows = statement.query_map(parameters, |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    let entries = rows.into_iter().map(|(model, id, count, running, last_seen, latest)| {
                        Ok(json!({ "model": model, "id": id, "requestCount": count, "running": running,
                            "lastSeen": last_seen, "latest": value_at(conn, latest)? }))
                    }).collect::<rusqlite::Result<Vec<_>>>()?;
                    return Ok(json!({ "available": true, "entries": entries, "matched": matched, "unknownRequests": unknown, "now": now }));
                }
                let (requests, running, sessions, unknown): (i64, i64, i64, i64) = conn.query_row(&format!("{BASE} SELECT COUNT(*), COALESCE(SUM(running),0),
                    COUNT(DISTINCT CASE WHEN session_id <> '' THEN json_array(request_model, session_id) END),
                    COALESCE(SUM(session_id = ''),0) FROM selected"), parameters, |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
                let group = "client, request_model, provider, account_id, CASE WHEN account_id = '' THEN account_name ELSE '' END";
                let matched: i64 = conn.query_row(&format!("{BASE} SELECT COUNT(*) FROM (SELECT 1 FROM selected GROUP BY {group})"), parameters, |row| row.get(0))?;
                let mut statement = conn.prepare(&format!("{BASE} SELECT json_array({group}), client, request_model, provider, account_id,
                    MAX(account_name), COUNT(*), SUM(running), SUM(status >= 200 AND status < 300 AND error IS NULL),
                    SUM((status <> 0 OR error IS NOT NULL) AND (status < 200 OR status >= 300 OR error IS NOT NULL)), COUNT(DISTINCT NULLIF(session_id, '')),
                    SUM(CASE WHEN json_valid(route) THEN json_extract(route, '$.stickyHit') = 1 ELSE 0 END), SUM(attempts > 1), MAX(last_seen)
                    FROM selected GROUP BY {group} ORDER BY SUM(running) DESC, COUNT(*) DESC, MAX(last_seen) DESC LIMIT 300"))?;
                let paths = statement.query_map(parameters, |row| Ok(json!({
                    "id": row.get::<_, String>(0)?, "client": row.get::<_, String>(1)?, "model": row.get::<_, String>(2)?,
                    "provider": row.get::<_, String>(3)?, "accountId": row.get::<_, String>(4)?, "accountName": row.get::<_, String>(5)?,
                    "requests": row.get::<_, i64>(6)?, "running": row.get::<_, i64>(7)?, "successful": row.get::<_, i64>(8)?,
                    "failed": row.get::<_, i64>(9)?, "sessions": row.get::<_, i64>(10)?, "stickyHits": row.get::<_, Option<i64>>(11)?.unwrap_or(0),
                    "switched": row.get::<_, i64>(12)?, "lastSeen": row.get::<_, i64>(13)?,
                })))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(json!({ "available": true, "paths": paths, "pathCount": matched, "truncated": matched > 300,
                    "requests": requests, "running": running, "sessions": sessions, "unknownRequests": unknown, "now": now, "since": since }))
            })
        };
        loaded.unwrap_or_else(|| json!({ "available": false, "paths": [], "entries": [], "matched": 0, "now": now }))
    }
}
