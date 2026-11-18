//! Free独立有界存储与锁外明文所有者。只用显式已缓存key，不读配置/文件/代理。
//! 解析中间节点、原行、新行、回读和编码/解码缓冲均保留到外层退出全部锁后清理。

use std::io;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{types::ValueRef, Connection};
use serde_json::{Map, Value};
use zeroize::Zeroize;

use super::{
    record, FreeEnrollmentError, StoredAccount, EPOCH_KEY, MAX_ACCOUNTS, MAX_RETAINED_BYTES,
    PRIVATE_FIELD, PROVIDER,
};
use crate::server::core::crypto;

pub(super) const MAX_ROW_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_PLAIN_BYTES: usize = 1024 * 1024;
pub(super) const MAX_SECRET_BYTES: usize = 256 * 1024;
pub(super) const MAX_PUBLIC_BYTES: usize = 64 * 1024;

#[must_use]
pub(in crate::server::core::account_store) struct RetainedRows {
    records: Vec<StoredAccount>,
    values: Vec<Value>,
    buffers: Vec<Vec<u8>>,
}

impl RetainedRows {
    pub(in crate::server::core::account_store) fn new() -> Self {
        Self {
            records: Vec::new(),
            values: Vec::new(),
            buffers: Vec::new(),
        }
    }
    pub(super) fn record(&self, index: usize) -> &StoredAccount {
        &self.records[index]
    }
    pub(super) fn value(&self, index: usize) -> &Value {
        &self.values[index]
    }
    pub(super) fn text(&self, index: usize) -> Result<&str, FreeEnrollmentError> {
        self.value(index)
            .as_str()
            .ok_or(FreeEnrollmentError::InvalidRecord)
    }
    pub(super) fn retain_value(&mut self, value: Value) -> usize {
        let index = self.values.len();
        self.values.push(value);
        index
    }
    pub(super) fn retain_record(&mut self, record: StoredAccount) -> usize {
        let index = self.records.len();
        self.records.push(record);
        index
    }
    pub(super) fn set_record(&mut self, index: usize, name: &str, value: Value) {
        if let Some(previous) = self.records[index]
            .fields_mut()
            .insert(name.to_string(), value)
        {
            self.values.push(previous);
        }
    }
    pub(super) fn set_value(
        &mut self,
        index: usize,
        name: &str,
        value: Value,
    ) -> Result<(), FreeEnrollmentError> {
        let value = self.retain_value(value);
        if !self.values[index].is_object() {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
        let value = self.take_value(value);
        let Some(fields) = self.values[index].as_object_mut() else {
            return Err(FreeEnrollmentError::InvalidRecord);
        };
        if let Some(previous) = fields.insert(name.to_string(), value) {
            self.values.push(previous);
        }
        Ok(())
    }
    pub(super) fn move_to_record(
        &mut self,
        record: usize,
        name: &str,
        value: usize,
    ) -> Result<(), FreeEnrollmentError> {
        if self.values[value].is_null() {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
        let value = self.take_value(value);
        self.set_record(record, name, value);
        Ok(())
    }
    pub(super) fn move_to_value(
        &mut self,
        target: usize,
        name: &str,
        source: usize,
    ) -> Result<(), FreeEnrollmentError> {
        if !self.values[target].is_object() || source == target {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
        let value = self.take_value(source);
        self.set_value(target, name, value)
    }
    fn take_value(&mut self, index: usize) -> Value {
        std::mem::replace(&mut self.values[index], Value::Null)
    }
    pub(super) fn buffer(&self, index: usize) -> &[u8] {
        &self.buffers[index]
    }
    fn retain_buffer(&mut self, bytes: Vec<u8>) -> usize {
        let index = self.buffers.len();
        self.buffers.push(bytes);
        index
    }
    pub(super) fn encode_record(&mut self, index: usize) -> Result<usize, FreeEnrollmentError> {
        record::check_fields(self.records[index].fields())?;
        let buffer = self.buffers.len();
        self.buffers.push(Vec::new());
        let mut output = Output {
            bytes: &mut self.buffers[buffer],
            limit: MAX_PLAIN_BYTES,
        };
        serde_json::to_writer(&mut output, self.records[index].fields())
            .map_err(|_| FreeEnrollmentError::LimitExceeded)?;
        Ok(buffer)
    }
    pub(super) fn encode_value(
        &mut self,
        index: usize,
        limit: usize,
    ) -> Result<usize, FreeEnrollmentError> {
        record::check_shape(self.value(index))?;
        let buffer = self.buffers.len();
        self.buffers.push(Vec::new());
        let mut output = Output {
            bytes: &mut self.buffers[buffer],
            limit,
        };
        serde_json::to_writer(&mut output, &self.values[index])
            .map_err(|_| FreeEnrollmentError::LimitExceeded)?;
        Ok(buffer)
    }
    fn parse_buffer(&mut self, index: usize, limit: usize) -> Result<usize, FreeEnrollmentError> {
        record::parse(&self.buffers[index], &mut self.values, limit)
    }
    pub(super) fn open_payload(
        &mut self,
        raw: &str,
        key: &[u8; 32],
        limit: usize,
    ) -> Result<usize, FreeEnrollmentError> {
        if raw.trim() != raw || !crypto::is_envelope(raw) {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
        let plain =
            crypto::open_with_key(raw, key).map_err(|_| FreeEnrollmentError::InvalidRecord)?;
        let bytes = self.retain_buffer(plain);
        self.parse_buffer(bytes, limit)
    }
    pub(super) fn decode_claims(
        &mut self,
        index: usize,
    ) -> Result<Option<usize>, FreeEnrollmentError> {
        let Some(token) = super::identity::access_token(&self.records[index])? else {
            return Ok(None);
        };
        if !token.contains('.') {
            return Ok(None);
        }
        let mut parts = token.split('.');
        let header = parts
            .next()
            .filter(|part| !part.is_empty())
            .ok_or(FreeEnrollmentError::InvalidRecord)?;
        let payload = parts
            .next()
            .filter(|part| !part.is_empty())
            .ok_or(FreeEnrollmentError::InvalidRecord)?;
        let signature = parts
            .next()
            .filter(|part| !part.is_empty())
            .ok_or(FreeEnrollmentError::InvalidRecord)?;
        if parts.next().is_some()
            || header.len() + payload.len() + signature.len() > MAX_SECRET_BYTES
        {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
        let encoded = payload.trim_end_matches('=');
        if payload.len() - encoded.len() > 2 {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
        // 借用records而非整个self，将解码直接写进外层持有的缓冲；不产生短命明文。
        let buffer = self.buffers.len();
        self.buffers.push(Vec::with_capacity(encoded.len()));
        URL_SAFE_NO_PAD
            .decode_vec(encoded, &mut self.buffers[buffer])
            .map_err(|_| FreeEnrollmentError::InvalidRecord)?;
        let claims = self.parse_buffer(buffer, MAX_SECRET_BYTES)?;
        if !self.value(claims).is_object() {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
        Ok(Some(claims))
    }
    fn normalize_plugin(
        &mut self,
        root: usize,
        name: &str,
        limit: usize,
    ) -> Result<(), FreeEnrollmentError> {
        let bytes = match self.value(root).get(name) {
            Some(Value::String(text)) => Some(text.as_bytes().to_vec()),
            Some(Value::Object(_)) | Some(Value::Null) | None => None,
            Some(_) => return Err(FreeEnrollmentError::InvalidRecord),
        };
        if let Some(bytes) = bytes {
            let bytes = self.retain_buffer(bytes);
            let parsed = self.parse_buffer(bytes, limit)?;
            if !self.value(parsed).is_object() {
                return Err(FreeEnrollmentError::InvalidRecord);
            }
            let value = self.take_value(parsed);
            self.set_value(root, name, value)?;
        }
        Ok(())
    }
}

impl Drop for RetainedRows {
    fn drop(&mut self) {
        for row in &mut self.records {
            wipe_fields(row.fields_mut());
        }
        for value in &mut self.values {
            wipe_value(value);
        }
        for bytes in &mut self.buffers {
            bytes.zeroize();
        }
    }
}
pub(in crate::server::core::account_store) fn wipe_record(record: &mut StoredAccount) {
    wipe_fields(record.fields_mut());
}
fn wipe_fields(fields: &mut Map<String, Value>) {
    for (mut key, mut value) in std::mem::take(fields) {
        key.zeroize();
        wipe_value(&mut value);
    }
}
fn wipe_value(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Object(fields) => wipe_fields(fields),
        Value::Array(items) => items.iter_mut().for_each(wipe_value),
        _ => {}
    }
}

pub(super) fn ensure_no_triggers(conn: &Connection) -> Result<(), FreeEnrollmentError> {
    let safe: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='accounts' AND type='table')
          AND EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='kv' AND type='table')
          AND NOT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND tbl_name IN ('accounts','kv','cline_free_refresh_slots'))
          AND NOT EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE type='trigger' AND tbl_name IN ('accounts','kv','cline_free_refresh_slots'))",
        [], |row| row.get(0),
    ).map_err(|_| FreeEnrollmentError::Storage)?;
    if safe != 1 {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    Ok(())
}

pub(super) fn account_ids(
    conn: &Connection,
    retained: &mut RetainedRows,
) -> Result<Vec<usize>, FreeEnrollmentError> {
    let mut statement = conn
        .prepare(
            "SELECT CASE WHEN typeof(id)='text' AND octet_length(id)<=512 THEN id END,
                CASE WHEN typeof(data)='text' THEN octet_length(data) END
         FROM main.accounts WHERE provider='cline-free' ORDER BY rowid LIMIT ?1",
        )
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let mut rows = statement
        .query([MAX_ACCOUNTS as i64 + 1])
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let mut ids = Vec::new();
    let mut bytes = 0i64;
    while let Some(row) = rows.next().map_err(|_| FreeEnrollmentError::Storage)? {
        if ids.len() == MAX_ACCOUNTS {
            return Err(FreeEnrollmentError::LimitExceeded);
        }
        let id = text(row, 0, 512)?;
        check_id(id)?;
        let size = integer(row, 1)?;
        if size <= 0 || size > MAX_ROW_BYTES as i64 {
            return Err(FreeEnrollmentError::LimitExceeded);
        }
        bytes = bytes
            .checked_add(size)
            .ok_or(FreeEnrollmentError::LimitExceeded)?;
        if bytes > MAX_RETAINED_BYTES {
            return Err(FreeEnrollmentError::LimitExceeded);
        }
        ids.push(retained.retain_value(Value::String(id.to_string())));
    }
    drop(rows);
    statement
        .finalize()
        .map_err(|_| FreeEnrollmentError::Storage)?;
    Ok(ids)
}

#[derive(Clone, Copy)]
enum AccountId<'a> {
    Borrowed(&'a str),
    Retained(usize),
}
impl AccountId<'_> {
    fn resolve<'a>(&'a self, retained: &'a RetainedRows) -> Result<&'a str, FreeEnrollmentError> {
        match self {
            Self::Borrowed(id) => Ok(id),
            Self::Retained(index) => retained.text(*index),
        }
    }
}
pub(super) fn load_existing(
    conn: &Connection,
    key: &[u8; 32],
    id: usize,
    retained: &mut RetainedRows,
) -> Result<usize, FreeEnrollmentError> {
    load(conn, key, AccountId::Retained(id), retained)
}
pub(super) fn load_account(
    conn: &Connection,
    key: &[u8; 32],
    id: &str,
    retained: &mut RetainedRows,
) -> Result<usize, FreeEnrollmentError> {
    load(conn, key, AccountId::Borrowed(id), retained)
}
fn load(
    conn: &Connection,
    key: &[u8; 32],
    id: AccountId<'_>,
    retained: &mut RetainedRows,
) -> Result<usize, FreeEnrollmentError> {
    check_id(id.resolve(retained)?)?;
    let mut statement = conn
        .prepare(
            "SELECT CASE WHEN typeof(data)='text' AND octet_length(data)<=?2 THEN data END,
                id, provider, enabled, priority, added_at
         FROM main.accounts WHERE id=?1 AND provider='cline-free' LIMIT 2",
        )
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let mut rows = statement
        .query(rusqlite::params![
            id.resolve(retained)?,
            MAX_ROW_BYTES as i64
        ])
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let row = rows
        .next()
        .map_err(|_| FreeEnrollmentError::Storage)?
        .ok_or(FreeEnrollmentError::Storage)?;
    let raw = text(row, 0, MAX_ROW_BYTES)?;
    if text(row, 1, 512)? != id.resolve(retained)? || text(row, 2, 128)? != PROVIDER {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    let enabled = integer(row, 3)?;
    let priority = integer(row, 4)?;
    let added = integer(row, 5)?;
    if !matches!(enabled, 0 | 1) {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    let encrypted = crypto::is_envelope(raw);
    let plain = if encrypted {
        crypto::open_with_key(raw, key).map_err(|_| FreeEnrollmentError::InvalidRecord)?
    } else {
        raw.as_bytes().to_vec()
    };
    let bytes = retained.retain_buffer(plain);
    let root = retained.parse_buffer(bytes, MAX_PLAIN_BYTES)?;
    if !retained.value(root).is_object() {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    if !encrypted && retained.value(root).get(PRIVATE_FIELD).is_some() {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    retained.normalize_plugin(root, "pluginSecret", MAX_SECRET_BYTES)?;
    retained.normalize_plugin(root, "pluginPublic", MAX_PUBLIC_BYTES)?;
    let fields = retained
        .value(root)
        .as_object()
        .ok_or(FreeEnrollmentError::InvalidRecord)?;
    record::check_fields(fields)?;
    if fields.get("id").and_then(Value::as_str) != Some(id.resolve(retained)?)
        || fields.get("provider").and_then(Value::as_str) != Some(PROVIDER)
        || fields
            .get("enabled")
            .is_some_and(|value| value.as_bool().is_none())
        || fields
            .get("priority")
            .is_some_and(|value| value.as_i64().is_none())
        || fields
            .get("addedAt")
            .is_some_and(|value| value.as_i64().is_none())
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    validate_optional_version(fields)?;
    let Value::Object(fields) = retained.take_value(root) else {
        return Err(FreeEnrollmentError::InvalidRecord);
    };
    let index = retained.retain_record(StoredAccount::from_map(fields));
    let record = retained.record(index);
    if i64::from(record.enabled()) != enabled
        || record.priority() != priority
        || record.added_at() != added
    {
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
    Ok(index)
}

pub(super) fn occupied(conn: &Connection, id: &str) -> Result<bool, FreeEnrollmentError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.accounts WHERE id=?1)",
        [id],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|_| FreeEnrollmentError::Storage)
}
pub(super) fn epoch(
    conn: &Connection,
    retained: &mut RetainedRows,
) -> Result<Option<usize>, FreeEnrollmentError> {
    let mut statement = conn.prepare(
        "SELECT CASE WHEN typeof(value)='text' AND octet_length(value)=64 THEN value END FROM main.kv WHERE key=?1 LIMIT 2",
    ).map_err(|_| FreeEnrollmentError::Storage)?;
    let mut rows = statement
        .query([EPOCH_KEY])
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let result = match rows.next().map_err(|_| FreeEnrollmentError::Storage)? {
        Some(row) => {
            let epoch = text(row, 0, 64)?;
            if !random_value(epoch) {
                return Err(FreeEnrollmentError::InvalidRecord);
            }
            Some(retained.retain_value(Value::String(epoch.to_string())))
        }
        None => None,
    };
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
    Ok(result)
}
pub(super) fn establish_epoch(
    conn: &Connection,
    retained: &mut RetainedRows,
) -> Result<usize, FreeEnrollmentError> {
    let epoch = retained.retain_value(Value::String(random_id()?));
    let changed = conn
        .execute(
            "INSERT INTO main.kv(key,value) VALUES(?1,?2)",
            rusqlite::params![EPOCH_KEY, retained.text(epoch)?],
        )
        .map_err(|_| FreeEnrollmentError::Storage)?;
    let read = self::epoch(conn, retained)?.ok_or(FreeEnrollmentError::Storage)?;
    if changed != 1 || retained.value(read) != retained.value(epoch) {
        return Err(FreeEnrollmentError::Storage);
    }
    Ok(epoch)
}
pub(super) fn random_id() -> Result<String, FreeEnrollmentError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| FreeEnrollmentError::RandomUnavailable)?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(FreeEnrollmentError::RandomUnavailable);
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
pub(super) fn random_value(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && value.bytes().any(|byte| byte != b'0')
}
pub(super) fn check_id(id: &str) -> Result<(), FreeEnrollmentError> {
    if id.is_empty() || id.len() > 512 || id.trim() != id || id.chars().any(char::is_control) {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    Ok(())
}
fn validate_optional_version(fields: &Map<String, Value>) -> Result<(), FreeEnrollmentError> {
    if let Some(version) = fields.get(PRIVATE_FIELD) {
        version_fields(version)?;
    }
    Ok(())
}
pub(super) fn version_fields(value: &Value) -> Result<&Map<String, Value>, FreeEnrollmentError> {
    let fields = value
        .as_object()
        .ok_or(FreeEnrollmentError::InvalidRecord)?;
    if fields.len() != 4
        || fields.get("schema").and_then(Value::as_u64) != Some(1)
        || fields
            .get("revision")
            .and_then(Value::as_u64)
            .is_none_or(|value| value == 0)
        || ["dbEpoch", "born"].iter().any(|name| {
            fields
                .get(*name)
                .and_then(Value::as_str)
                .is_none_or(|text| !random_value(text))
        })
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    Ok(fields)
}
pub(super) fn text<'a>(
    row: &'a rusqlite::Row<'_>,
    column: usize,
    limit: usize,
) -> Result<&'a str, FreeEnrollmentError> {
    let ValueRef::Text(bytes) = row
        .get_ref(column)
        .map_err(|_| FreeEnrollmentError::Storage)?
    else {
        return Err(FreeEnrollmentError::InvalidRecord);
    };
    if bytes.is_empty() || bytes.len() > limit {
        return Err(FreeEnrollmentError::LimitExceeded);
    }
    std::str::from_utf8(bytes).map_err(|_| FreeEnrollmentError::InvalidRecord)
}
fn integer(row: &rusqlite::Row<'_>, column: usize) -> Result<i64, FreeEnrollmentError> {
    match row
        .get_ref(column)
        .map_err(|_| FreeEnrollmentError::Storage)?
    {
        ValueRef::Integer(value) => Ok(value),
        _ => Err(FreeEnrollmentError::InvalidRecord),
    }
}

struct Output<'a> {
    bytes: &'a mut Vec<u8>,
    limit: usize,
}
impl io::Write for Output<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("Free JSON limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
