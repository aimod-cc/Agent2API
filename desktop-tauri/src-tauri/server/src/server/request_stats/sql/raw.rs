//! 两侧原始报文的读写、计数与清理 SQL。

use rusqlite::{params, params_from_iter, Connection};

use super::FilterPlan;
use crate::server::request_stats::record::{MAX_RAW_BYTES, MAX_RAW_ROWS};

#[derive(Clone, Copy, Default)]
pub(in crate::server::request_stats) struct RawCounts {
    pub requests: usize,
    pub downstream: usize,
    pub upstream: usize,
}

fn raw_where(plan: &FilterPlan) -> String {
    if plan.where_sql.is_empty() {
        String::new()
    } else {
        format!(" WHERE id IN (SELECT id FROM requests{})", plan.where_sql)
    }
}

fn count_table(conn: &Connection, table: &str, plan: &FilterPlan) -> rusqlite::Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM {table}{}", raw_where(plan));
    let count: i64 = conn.query_row(&sql, params_from_iter(plan.binds.iter()), |row| row.get(0))?;
    Ok(count.max(0) as usize)
}

pub(in crate::server::request_stats) fn count_raw_matching(
    conn: &Connection,
    plan: &FilterPlan,
) -> rusqlite::Result<RawCounts> {
    let where_sql = raw_where(plan);
    // 两侧共用请求 ID；UNION 避免把同一次请求计为两次。
    let sql = format!(
        "SELECT COUNT(*) FROM (SELECT id FROM request_raw{where_sql} \
         UNION SELECT id FROM debug_traffic{where_sql})"
    );
    let requests: i64 = conn.query_row(
        &sql,
        params_from_iter(plan.binds.iter().chain(plan.binds.iter())),
        |row| row.get(0),
    )?;
    Ok(RawCounts {
        requests: requests.max(0) as usize,
        downstream: count_table(conn, "request_raw", plan)?,
        upstream: count_table(conn, "debug_traffic", plan)?,
    })
}

/// 调用方持有事务，并在删除明细前调用；无筛选时也清理孤立报文。
pub(in crate::server::request_stats) fn delete_raw_matching(
    conn: &Connection,
    plan: &FilterPlan,
) -> rusqlite::Result<RawCounts> {
    let mut counts = count_raw_matching(conn, plan)?;
    let where_sql = raw_where(plan);
    counts.downstream = conn.execute(
        &format!("DELETE FROM request_raw{where_sql}"),
        params_from_iter(plan.binds.iter()),
    )?;
    counts.upstream = conn.execute(
        &format!("DELETE FROM debug_traffic{where_sql}"),
        params_from_iter(plan.binds.iter()),
    )?;
    Ok(counts)
}

/// 正文只为仍有请求明细的 ID 保存，避免清理后迟到的下游回写产生孤立报文。
pub(in crate::server::request_stats) fn upsert_raw(
    conn: &Connection,
    id: &str,
    ts: i64,
    request_body: &str,
    response_body: &str,
    size: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO request_raw (id, ts, request_body, response_body, size, truncated) \
         SELECT ?1, ?2, ?3, ?4, ?5, 0 WHERE EXISTS (SELECT 1 FROM requests WHERE id = ?1) \
         ON CONFLICT(id) DO UPDATE SET ts = excluded.ts, request_body = excluded.request_body, \
         response_body = excluded.response_body, size = excluded.size, truncated = 0",
        params![id, ts, request_body, response_body, size],
    )?;
    Ok(())
}

pub(in crate::server::request_stats) fn count_raw(conn: &Connection) -> rusqlite::Result<usize> {
    count_table(conn, "request_raw", &FilterPlan::of(&super::super::RequestQuery::default()))
}

fn raw_stats(conn: &Connection) -> rusqlite::Result<(usize, i64)> {
    let (count, bytes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM request_raw",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((count.max(0) as usize, bytes.max(0)))
}

pub(in crate::server::request_stats) fn count_upstream_raw(conn: &Connection) -> rusqlite::Result<usize> {
    count_table(conn, "debug_traffic", &FilterPlan::of(&super::super::RequestQuery::default()))
}

pub(in crate::server::request_stats) fn trim_raw_rows(
    conn: &Connection,
    overflow: usize,
) -> rusqlite::Result<usize> {
    if overflow == 0 {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM request_raw WHERE id IN \
         (SELECT id FROM request_raw ORDER BY ts ASC, rowid ASC LIMIT ?1)",
        params![overflow as i64],
    )
}

fn trim_raw_bytes(conn: &Connection, max: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM request_raw WHERE id NOT IN (\
           SELECT id FROM (\
             SELECT id, SUM(size) OVER (\
               ORDER BY ts DESC, rowid DESC ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW\
             ) AS cumulative FROM request_raw\
           ) WHERE cumulative <= ?1\
         )",
        params![max],
    )
}

/// 同时按条数与正文合计字节淘汰最旧的下游正文
pub(in crate::server::request_stats) fn trim_raw_to_caps(
    conn: &Connection,
) -> rusqlite::Result<usize> {
    let mut removed = 0;
    let (_, bytes) = raw_stats(conn)?;
    if bytes > MAX_RAW_BYTES {
        removed += trim_raw_bytes(conn, MAX_RAW_BYTES)?;
    }
    let (count, _) = raw_stats(conn)?;
    if count > MAX_RAW_ROWS {
        removed += trim_raw_rows(conn, count - MAX_RAW_ROWS)?;
    }
    Ok(removed)
}

pub(in crate::server::request_stats) fn select_raw(
    conn: &Connection,
    id: &str,
) -> rusqlite::Result<Option<(String, String, String, bool)>> {
    let mut stmt = conn.prepare(
        "SELECT id, request_body, response_body, truncated FROM request_raw WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))),
        None => Ok(None),
    }
}
