//! 真实单行和kv读取。只用调用方已选的密钥，不访问配置、文件、代理或插件注册表。

use rusqlite::{types::ValueRef, Connection, ErrorCode, Row};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{bounded, Conflict, Error, StorageFailure, Version};
use crate::server::core::account_store::{credential_version, state::StoredAccount};
use crate::server::core::crypto;

pub(super) fn sql_error(error: rusqlite::Error, otherwise: StorageFailure) -> Error {
    Error::Storage(match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => StorageFailure::Busy,
        _ => otherwise,
    })
}

pub(super) fn ensure_no_triggers(conn: &Connection) -> Result<(), Error> {
    let triggers: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND tbl_name IN ('accounts','kv'))
             OR EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE type='trigger' AND tbl_name IN ('accounts','kv'))",
        [], |row| row.get(0),
    ).map_err(|error| sql_error(error, StorageFailure::Read))?;
    if triggers != 0 {
        return Err(Error::InvalidRecord);
    }
    Ok(())
}

pub(super) fn load_for(
    conn: &Connection,
    key: &[u8; 32],
    provider: &str,
    id: &str,
) -> Result<StoredAccount, Error> {
    if !super::maintained_provider(provider) {
        return Err(Error::UnsupportedProvider);
    }
    super::check_id(id)?;
    // 外来备份可包含触发器；不执行或信任它们来维护版本，也不隔离普通数据库访问。
    ensure_no_triggers(conn)?;
    let mut stmt = conn.prepare(
        "SELECT CASE WHEN typeof(data) = 'text' AND octet_length(data) <= ?2 THEN data END,
                octet_length(data), typeof(data), id,
                CASE WHEN typeof(provider) = 'text' AND octet_length(provider) <= 128 THEN provider END,
                enabled, priority, added_at
         FROM main.accounts WHERE id = ?1 LIMIT 1",
    ).map_err(|error| sql_error(error, StorageFailure::Read))?;
    let mut rows = stmt
        .query(rusqlite::params![id, bounded::MAX_ROW_BYTES as i64])
        .map_err(|error| sql_error(error, StorageFailure::Read))?;
    let row = rows
        .next()
        .map_err(|error| sql_error(error, StorageFailure::Read))?
        .ok_or(Error::Conflict(Conflict::MissingAccount))?;
    if text(row, 2, 16)? != "text" {
        return Err(Error::InvalidRecord);
    }
    let count = integer(row, 1)?;
    if count <= 0 {
        return Err(Error::InvalidRecord);
    }
    if count > bounded::MAX_ROW_BYTES as i64 {
        return Err(Error::LimitExceeded);
    }
    let raw = text(row, 0, bounded::MAX_ROW_BYTES)?;
    let column_id = text(row, 3, 512)?;
    let column_provider = text(row, 4, 128)?;
    if column_id != id {
        return Err(Error::InvalidRecord);
    }
    if column_provider != provider {
        return Err(Error::Conflict(Conflict::ProviderChanged));
    }
    let enabled = integer(row, 5)?;
    if !matches!(enabled, 0 | 1) {
        return Err(Error::InvalidRecord);
    }
    let record = decode(raw, key)?;
    if record.id() != column_id
        || record.get("provider").and_then(Value::as_str) != Some(provider)
        || record
            .get("enabled")
            .is_some_and(|value| !value.is_boolean())
        || (if record.enabled() { 1 } else { 0 }) != enabled
        || record
            .get("priority")
            .is_some_and(|value| value.as_i64().is_none())
        || record.priority() != integer(row, 6)?
        || record
            .get("addedAt")
            .is_some_and(|value| value.as_i64().is_none())
        || record.added_at() != integer(row, 7)?
    {
        return Err(Error::InvalidRecord);
    }
    if rows
        .next()
        .map_err(|error| sql_error(error, StorageFailure::Read))?
        .is_some()
    {
        return Err(Error::InvalidRecord);
    }
    drop(rows);
    stmt.finalize()
        .map_err(|error| sql_error(error, StorageFailure::Read))?;
    Ok(record)
}

fn text<'a>(row: &'a Row<'_>, column: usize, limit: usize) -> Result<&'a str, Error> {
    let ValueRef::Text(bytes) = row
        .get_ref(column)
        .map_err(|error| sql_error(error, StorageFailure::Read))?
    else {
        return Err(Error::InvalidRecord);
    };
    if bytes.len() > limit {
        return Err(Error::LimitExceeded);
    }
    std::str::from_utf8(bytes).map_err(|_| Error::InvalidRecord)
}

fn integer(row: &Row<'_>, column: usize) -> Result<i64, Error> {
    match row
        .get_ref(column)
        .map_err(|error| sql_error(error, StorageFailure::Read))?
    {
        ValueRef::Integer(value) => Ok(value),
        _ => Err(Error::InvalidRecord),
    }
}

fn decode(raw: &str, key: &[u8; 32]) -> Result<StoredAccount, Error> {
    let encrypted = crypto::is_envelope(raw);
    let plaintext;
    let bytes = if encrypted {
        plaintext =
            Zeroizing::new(crypto::open_with_key(raw, key).map_err(|_| Error::InvalidRecord)?);
        plaintext.as_slice()
    } else {
        raw.as_bytes()
    };
    let record = decode_plain(bytes)?;
    // 版本只由宿主写进加密行；旧明文导入中的同名JSON不能取得已有合同资格。
    if !encrypted && record.get(credential_version::PRIVATE_FIELD).is_some() {
        return Err(Error::InvalidRecord);
    }
    Ok(record)
}

fn decode_plain(bytes: &[u8]) -> Result<StoredAccount, Error> {
    let mut budgeted = bounded::parse(bytes, bounded::MAX_PLAIN_BYTES)?;
    if !budgeted.is_object() {
        return Err(Error::InvalidRecord);
    }
    if let Value::Object(fields) = &mut budgeted {
        for name in ["pluginSecret", "pluginPublic"] {
            if let Some(Value::String(text)) = fields.get(name) {
                let parsed = bounded::parse(
                    text.as_bytes(),
                    if name == "pluginSecret" {
                        bounded::MAX_SECRET_BYTES
                    } else {
                        bounded::MAX_PUBLIC_BYTES
                    },
                )?;
                if !parsed.is_object() {
                    return Err(Error::InvalidRecord);
                }
                fields.insert(name.to_string(), parsed);
            }
        }
    }
    bounded::shape_check(&budgeted)?;
    let record = StoredAccount::from_value(budgeted).ok_or(Error::InvalidRecord)?;
    bounded::check(&record.to_value(), bounded::MAX_PLAIN_BYTES)?;
    Ok(record)
}

pub(super) fn epoch(conn: &Connection) -> Result<Option<String>, Error> {
    let mut stmt = conn
        .prepare(
            "SELECT CASE WHEN typeof(value) = 'text' AND octet_length(value) = 64 THEN value END
         FROM main.kv WHERE key = ?1 LIMIT 1",
        )
        .map_err(|error| sql_error(error, StorageFailure::Read))?;
    let mut rows = stmt
        .query([credential_version::EPOCH_KEY])
        .map_err(|error| sql_error(error, StorageFailure::Read))?;
    let result = if let Some(row) = rows
        .next()
        .map_err(|error| sql_error(error, StorageFailure::Read))?
    {
        let value = text(row, 0, 64)?;
        if !valid_id(value) {
            return Err(Error::InvalidRecord);
        }
        Some(value.to_string())
    } else {
        None
    };
    if rows
        .next()
        .map_err(|error| sql_error(error, StorageFailure::Read))?
        .is_some()
    {
        return Err(Error::InvalidRecord);
    }
    drop(rows);
    stmt.finalize()
        .map_err(|error| sql_error(error, StorageFailure::Read))?;
    Ok(result)
}

pub(super) fn establish_epoch(conn: &Connection) -> Result<String, Error> {
    let epoch = random_id()?;
    let changed = conn
        .execute(
            "INSERT INTO main.kv(key, value) VALUES(?1, ?2)",
            rusqlite::params![credential_version::EPOCH_KEY, epoch],
        )
        .map_err(|error| sql_error(error, StorageFailure::Write))?;
    if changed != 1 || self::epoch(conn)?.as_deref() != Some(epoch.as_str()) {
        return Err(Error::Storage(StorageFailure::Write));
    }
    Ok(epoch)
}

pub(super) fn random_id() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| Error::Storage(StorageFailure::RandomUnavailable))?;
    // 不回退时间戳、账号ID或全0；系统熵源异常时只拒绝新合同。
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(Error::Storage(StorageFailure::RandomUnavailable));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn valid_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && value.bytes().any(|byte| byte != b'0')
}

pub(super) fn version(record: &StoredAccount) -> Result<Option<Version>, Error> {
    let Some(value) = record.get(credential_version::PRIVATE_FIELD) else {
        return Ok(None);
    };
    let fields = value.as_object().ok_or(Error::InvalidRecord)?;
    if fields.len() != 4 || fields.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err(Error::InvalidRecord);
    }
    let text = |name: &str| {
        fields
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| valid_id(value))
            .map(str::to_string)
            .ok_or(Error::InvalidRecord)
    };
    let revision = fields
        .get("revision")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or(Error::InvalidRecord)?;
    Ok(Some(Version {
        epoch: text("dbEpoch")?,
        born: text("born")?,
        revision,
    }))
}

pub(super) fn set_version(record: &mut StoredAccount, version: &Version) {
    record.set(credential_version::PRIVATE_FIELD, json!({
        "schema": 1, "dbEpoch": version.epoch, "born": version.born, "revision": version.revision,
    }));
}

pub(super) fn update_existing_for(
    conn: &Connection,
    key: &[u8; 32],
    provider: &str,
    record: &StoredAccount,
) -> Result<(), Error> {
    if !super::maintained_provider(provider) || record.provider() != provider {
        return Err(Error::UnsupportedProvider);
    }
    let bytes = Zeroizing::new(bounded::encode(
        &record.to_value(),
        bounded::MAX_PLAIN_BYTES,
    )?);
    let data = crypto::seal_with_key(&bytes, key)
        .map_err(|_| Error::Storage(StorageFailure::Encryption))?;
    if data.len() > bounded::MAX_ROW_BYTES {
        return Err(Error::LimitExceeded);
    }
    // 本合同只修改data；id/provider/priority/enabled/added_at均取事务内当前行而且不变。
    // 刻意不走update_in_place：它的缺行INSERT回落不适用于条件提交。
    let changed = conn
        .execute(
            "UPDATE main.accounts SET data = ?3 WHERE id = ?1 AND provider = ?2",
            rusqlite::params![record.id(), provider, data],
        )
        .map_err(|error| sql_error(error, StorageFailure::Write))?;
    if changed != 1 {
        return Err(Error::Conflict(Conflict::MissingAccount));
    }
    Ok(())
}
