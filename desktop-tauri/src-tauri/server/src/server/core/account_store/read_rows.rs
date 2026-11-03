use rusqlite::{params, Connection, OptionalExtension};

use super::sql::{self, DecodeError};
use super::state::StoredAccount;

pub(crate) struct RawAccount {
    pub rowid: i64,
    pub data: String,
}

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawAccount> {
    Ok(RawAccount { rowid: row.get(0)?, data: row.get(1)? })
}

pub(crate) fn all(conn: &Connection) -> rusqlite::Result<Vec<RawAccount>> {
    let mut statement = conn.prepare("SELECT rowid, data FROM accounts ORDER BY rowid")?;
    let rows = statement.query_map([], row)?;
    rows.collect()
}

pub(crate) fn provider(conn: &Connection, provider: &str) -> rusqlite::Result<Vec<RawAccount>> {
    let mut statement = conn.prepare("SELECT rowid, data FROM accounts WHERE provider = ?1 ORDER BY rowid")?;
    let rows = statement.query_map(params![provider], row)?;
    rows.collect()
}

pub(crate) fn by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<RawAccount>> {
    conn.query_row("SELECT rowid, data FROM accounts WHERE id = ?1", params![id], row).optional()
}

pub(crate) fn changed(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<(i64, Option<RawAccount>)>> {
    if ids.is_empty() { return Ok(Vec::new()); }
    let mut statement = conn.prepare("SELECT rowid, data FROM accounts WHERE rowid IN (SELECT CAST(value AS INTEGER) FROM json_each(?1)) ORDER BY rowid")?;
    let present = statement.query_map(params![serde_json::json!(ids).to_string()], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut by_id: std::collections::HashMap<_, _> = present.into_iter().map(|row| (row.rowid, row)).collect();
    Ok(ids.iter().map(|id| (*id, by_id.remove(id))).collect())
}

pub(crate) fn decode(raw: RawAccount) -> Result<(i64, StoredAccount), DecodeError> {
    sql::decode_data(&raw.data).map(|record| (raw.rowid, record))
}

pub(crate) fn decode_many(rows: Vec<RawAccount>, context: &str) -> Vec<(i64, StoredAccount)> {
    let mut failed = 0;
    let decoded = rows.into_iter().filter_map(|row| match decode(row) {
        Ok(record) => Some(record),
        Err(_) => { failed += 1; None }
    }).collect();
    if failed > 0 {
        sql::warn_load_failure(context, &format!("{failed} 条账号记录解密或解析失败，已跳过；原始数据未修改"));
    }
    decoded
}
