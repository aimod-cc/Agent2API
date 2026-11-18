//! Free唯一首次Ready固定槽。双锚先拒任何旧行，不解密旧槽、不按缺槽/旧epoch补资格。
//! payload只含行/UID/版本/随机代次绑定，无凭据；仅INSERT一次并同事务密文回读。

use rusqlite::{types::ValueRef, Connection};
use serde_json::{Map, Value};

use super::{
    storage, FreeEnrollmentError, FreeEnrollmentInput, RetainedRows, PRIVATE_FIELD, PROVIDER,
};
use crate::server::core::crypto;
use crate::server::db::schema::CLINE_FREE_REFRESH_SLOT_SCHEMA;

const MAX_ENVELOPE: usize = 8192;
const MAX_PLAIN: usize = 4096;

pub(super) fn require_absent(
    conn: &Connection,
    account: &str,
    identity_key: &str,
) -> Result<(), FreeEnrollmentError> {
    if conn.is_autocommit() {
        return Err(FreeEnrollmentError::Storage);
    }
    storage::check_id(account)?;
    if !hash_value(identity_key) {
        return Err(FreeEnrollmentError::InvalidInput);
    }
    storage::ensure_no_triggers(conn)?;
    let exists: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='cline_free_refresh_slots')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| FreeEnrollmentError::Storage)?;
    if exists == 0 {
        return Ok(());
    }
    validate_schema(conn)?;
    let occupied: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.cline_free_refresh_slots WHERE account_id=?1 OR identity_key=?2)",
        rusqlite::params![account, identity_key], |row| row.get(0),
    ).map_err(|_| FreeEnrollmentError::Storage)?;
    if occupied != 0 {
        return Err(FreeEnrollmentError::ExistingSlot);
    }
    Ok(())
}

pub(super) fn insert_initial(
    conn: &Connection,
    key: &[u8; 32],
    account: &str,
    identity_key: &str,
    row: usize,
    input: &FreeEnrollmentInput,
    retained: &mut RetainedRows,
) -> Result<(), FreeEnrollmentError> {
    require_absent(conn, account, identity_key)?;
    conn.execute_batch(CLINE_FREE_REFRESH_SLOT_SCHEMA)
        .map_err(|_| FreeEnrollmentError::Storage)?;
    validate_schema(conn)?;
    let epoch = storage::epoch(conn, retained)?.ok_or(FreeEnrollmentError::RecoveryIsolated)?;
    let record = retained.record(row);
    let version = storage::version_fields(
        record
            .get(PRIVATE_FIELD)
            .ok_or(FreeEnrollmentError::InvalidRecord)?,
    )?;
    if record.id() != account
        || record.get("account").and_then(Value::as_str) != Some(input.uid())
        || record.get("pluginSecret") != Some(input.secret())
        || record.get("pluginPublic") != Some(input.public())
        || version.get("dbEpoch").and_then(Value::as_str) != Some(retained.text(epoch)?)
        || version.get("revision").and_then(Value::as_u64) != Some(1)
    {
        return Err(FreeEnrollmentError::RecoveryIsolated);
    }
    let payload = retained.retain_value(Value::Object(Map::new()));
    for (name, text) in [
        ("provider", PROVIDER),
        ("accountId", account),
        ("uid", input.uid()),
        ("identityKey", identity_key),
    ] {
        retained.set_value(payload, name, Value::String(text.to_string()))?;
    }
    for name in ["dbEpoch", "born", "revision"] {
        let version = storage::version_fields(
            retained
                .record(row)
                .get(PRIVATE_FIELD)
                .ok_or(FreeEnrollmentError::InvalidRecord)?,
        )?;
        let value = version
            .get(name)
            .ok_or(FreeEnrollmentError::InvalidRecord)?
            .clone();
        retained.set_value(payload, name, value)?;
    }
    retained.set_value(payload, "schema", Value::from(2))?;
    let generation = storage::random_id()?;
    retained.set_value(payload, "generation", Value::String(generation))?;
    let state = retained.retain_value(Value::Object(Map::new()));
    retained.set_value(state, "kind", Value::String("ready".to_string()))?;
    retained.set_value(state, "settledOperation", Value::Null)?;
    retained.move_to_value(payload, "state", state)?;
    valid_initial(retained.value(payload), account, identity_key, input.uid())?;
    let bytes = retained.encode_value(payload, MAX_PLAIN)?;
    let envelope = crypto::seal_with_key(retained.buffer(bytes), key)
        .map_err(|_| FreeEnrollmentError::Storage)?;
    if envelope.len() > MAX_ENVELOPE {
        return Err(FreeEnrollmentError::LimitExceeded);
    }
    let changed = conn.execute(
        "INSERT INTO main.cline_free_refresh_slots(account_id,identity_key,payload) VALUES(?1,?2,?3)",
        rusqlite::params![account, identity_key, envelope],
    ).map_err(|_| FreeEnrollmentError::Storage)?;
    let read = readback(conn, key, account, identity_key, retained)?;
    valid_initial(retained.value(read), account, identity_key, input.uid())?;
    if changed != 1 || retained.value(read) != retained.value(payload) {
        return Err(FreeEnrollmentError::Storage);
    }
    Ok(())
}

pub(super) fn readback(
    conn: &Connection,
    key: &[u8; 32],
    account: &str,
    identity_key: &str,
    retained: &mut RetainedRows,
) -> Result<usize, FreeEnrollmentError> {
    let mut statement = conn.prepare(
        "SELECT account_id,identity_key,CASE WHEN typeof(payload)='text' AND octet_length(payload)<=?3 THEN payload END
         FROM main.cline_free_refresh_slots WHERE account_id=?1 AND identity_key=?2 LIMIT 2",
    ).map_err(|_| FreeEnrollmentError::Storage)?;
    let mut rows = statement
        .query(rusqlite::params![
            account,
            identity_key,
            MAX_ENVELOPE as i64
        ])
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let row = rows
        .next()
        .map_err(|_| FreeEnrollmentError::Storage)?
        .ok_or(FreeEnrollmentError::Storage)?;
    if storage::text(row, 0, 512)? != account || storage::text(row, 1, 64)? != identity_key {
        return Err(FreeEnrollmentError::Storage);
    }
    let raw = storage::text(row, 2, MAX_ENVELOPE)?;
    let read = retained.open_payload(raw, key, MAX_PLAIN)?;
    if rows
        .next()
        .map_err(|_| FreeEnrollmentError::Storage)?
        .is_some()
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    drop(rows);
    statement
        .finalize()
        .map_err(|_| FreeEnrollmentError::Storage)?;
    Ok(read)
}

fn valid_initial(
    value: &Value,
    account: &str,
    identity_key: &str,
    uid: &str,
) -> Result<(), FreeEnrollmentError> {
    let fields = value
        .as_object()
        .ok_or(FreeEnrollmentError::InvalidRecord)?;
    if fields.len() != 10
        || fields.get("schema").and_then(Value::as_u64) != Some(2)
        || fields.get("provider").and_then(Value::as_str) != Some(PROVIDER)
        || fields.get("accountId").and_then(Value::as_str) != Some(account)
        || fields.get("uid").and_then(Value::as_str) != Some(uid)
        || fields.get("identityKey").and_then(Value::as_str) != Some(identity_key)
        || fields.get("revision").and_then(Value::as_u64) != Some(1)
        || ["dbEpoch", "born", "generation"].iter().any(|name| {
            fields
                .get(*name)
                .and_then(Value::as_str)
                .is_none_or(|value| !storage::random_value(value))
        })
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    let state = fields
        .get("state")
        .and_then(Value::as_object)
        .ok_or(FreeEnrollmentError::InvalidRecord)?;
    if state.len() != 2
        || state.get("kind").and_then(Value::as_str) != Some("ready")
        || state.get("settledOperation") != Some(&Value::Null)
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    Ok(())
}

pub(super) fn validate_schema(conn: &Connection) -> Result<(), FreeEnrollmentError> {
    storage::ensure_no_triggers(conn)?;
    let mut statement = conn.prepare(
        "SELECT sql FROM main.sqlite_schema WHERE name='cline_free_refresh_slots' AND type='table' LIMIT 2",
    ).map_err(|_| FreeEnrollmentError::Storage)?;
    let mut rows = statement
        .query([])
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let row = rows
        .next()
        .map_err(|_| FreeEnrollmentError::Storage)?
        .ok_or(FreeEnrollmentError::InvalidRecord)?;
    let ddl = storage::text(row, 0, 8192)?;
    if normalized(ddl) != normalized(CLINE_FREE_REFRESH_SLOT_SCHEMA) {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    if rows
        .next()
        .map_err(|_| FreeEnrollmentError::Storage)?
        .is_some()
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    drop(rows);
    statement
        .finalize()
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let mut statement = conn
        .prepare("PRAGMA main.table_xinfo(cline_free_refresh_slots)")
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let mut rows = statement
        .query([])
        .map_err(|_| FreeEnrollmentError::Storage)?;
    for (position, name, primary) in [
        (0i64, "account_id", 1i64),
        (1, "identity_key", 0),
        (2, "payload", 0),
    ] {
        let row = rows
            .next()
            .map_err(|_| FreeEnrollmentError::Storage)?
            .ok_or(FreeEnrollmentError::InvalidRecord)?;
        let integer = |index: usize| match row.get_ref(index) {
            Ok(ValueRef::Integer(value)) => Ok(value),
            _ => Err(FreeEnrollmentError::InvalidRecord),
        };
        if integer(0)? != position
            || storage::text(row, 1, 32)? != name
            || storage::text(row, 2, 16)? != "TEXT"
            || integer(3)? != 1
            || !matches!(row.get_ref(4), Ok(ValueRef::Null))
            || integer(5)? != primary
            || integer(6)? != 0
        {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
    }
    if rows
        .next()
        .map_err(|_| FreeEnrollmentError::Storage)?
        .is_some()
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    drop(rows);
    statement
        .finalize()
        .map_err(|_| FreeEnrollmentError::Storage)?;
    Ok(())
}
fn normalized(ddl: &str) -> String {
    let mut out = String::with_capacity(ddl.len());
    let mut literal = false;
    for ch in ddl.chars() {
        if ch == '\'' {
            literal = !literal;
            out.push(ch);
        } else if literal {
            // CHECK内的'text'和GLOB大小写/空白是实际语义，不能随SQL关键字一起归一。
            out.push(ch);
        } else if !ch.is_whitespace() && ch != ';' {
            out.push(ch.to_ascii_lowercase());
        }
    }
    let body = out.strip_prefix("createtable").unwrap_or(&out);
    let body = body.strip_prefix("ifnotexists").unwrap_or(body);
    let body = body.strip_prefix("main.").unwrap_or(body);
    format!("createtable{body}")
}
fn hash_value(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
