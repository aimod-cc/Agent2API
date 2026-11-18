//! Free新账号的固定字段/版本/INSERT及有界记录校验；不复用普通upsert或CodeArts投影。
//! JSON Seed只把节点放进外层arena，坏正文半棵树也不会在存储锁内销毁。

use std::{fmt, io};

use rusqlite::{types::ValueRef, Connection};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::{
    identity, storage, FreeEnrollmentError, FreeEnrollmentInput, RetainedRows, StoredAccount,
    PRIVATE_FIELD, PROVIDER,
};
use crate::server::core::{account_store::priority, crypto};

const MAX_NODES: usize = 16_384;
const MAX_DEPTH: usize = 32;

pub(super) fn create(
    conn: &Connection,
    id: &str,
    input: &FreeEnrollmentInput,
    now: i64,
    retained: &mut RetainedRows,
) -> Result<usize, FreeEnrollmentError> {
    let name = input
        .public()
        .get("name")
        .and_then(Value::as_str)
        .ok_or(FreeEnrollmentError::InvalidInput)?;
    let priority = next_priority(conn)?;
    let index = retained.retain_record(StoredAccount::from_map(Map::new()));
    for (key, text) in [
        ("id", id),
        ("provider", PROVIDER),
        ("source", "plugin"),
        ("name", name),
        ("account", input.uid()),
        ("uid", input.uid()),
        ("pluginIdentity", input.identity()),
    ] {
        retained.set_record(index, key, Value::String(text.to_string()));
    }
    for (key, value) in [
        ("desktop", Value::Bool(false)),
        ("enabled", Value::Bool(true)),
        ("priority", Value::from(priority)),
        ("addedAt", Value::from(now)),
        ("updatedAt", Value::from(now)),
        ("pluginIdentityContract", Value::from(1)),
    ] {
        retained.set_record(index, key, value);
    }
    // 仅必需的新行材料副本进入锁外所有者，不Clone Input、不创建原生token镜像。
    retained.set_record(index, "pluginSecret", input.secret().clone());
    retained.set_record(index, "pluginPublic", input.public().clone());
    Ok(index)
}

pub(super) fn establish_version(
    record: usize,
    epoch: usize,
    born: usize,
    retained: &mut RetainedRows,
) -> Result<(), FreeEnrollmentError> {
    let version = retained.retain_value(Value::Object(Map::new()));
    retained.set_value(version, "schema", Value::from(1))?;
    retained.set_value(version, "revision", Value::from(1))?;
    for (key, index) in [("dbEpoch", epoch), ("born", born)] {
        let text = retained.text(index)?;
        if !storage::random_value(text) {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
        let value = Value::String(text.to_string());
        retained.set_value(version, key, value)?;
    }
    retained.move_to_record(record, PRIVATE_FIELD, version)?;
    Ok(())
}

pub(super) fn verify(
    index: usize,
    input: &FreeEnrollmentInput,
    retained: &RetainedRows,
) -> Result<(), FreeEnrollmentError> {
    identity::verify_input(input)?;
    let record = retained.record(index);
    let fields = record.fields();
    check_fields(fields)?;
    let id = crate::server::core::account_store::plugin_contract::contract_id(
        PROVIDER,
        input.identity(),
        false,
    );
    if record.id() != id
        || fields.get("provider").and_then(Value::as_str) != Some(PROVIDER)
        || fields.get("source").and_then(Value::as_str) != Some("plugin")
        || fields.get("desktop").and_then(Value::as_bool) != Some(false)
        || fields.get("enabled").and_then(Value::as_bool) != Some(true)
        || fields
            .get("priority")
            .and_then(Value::as_i64)
            .is_none_or(|value| !(priority::MIN_PRIORITY..=priority::MAX_PRIORITY).contains(&value))
        || fields
            .get("addedAt")
            .and_then(Value::as_i64)
            .is_none_or(|value| value <= 0)
        || fields.get("updatedAt") != fields.get("addedAt")
        || fields.get("name") != input.public().get("name")
        || fields.get("account").and_then(Value::as_str) != Some(input.uid())
        || fields.get("uid").and_then(Value::as_str) != Some(input.uid())
        || fields.get("pluginIdentity").and_then(Value::as_str) != Some(input.identity())
        || fields.get("pluginIdentityContract").and_then(Value::as_u64) != Some(1)
        || fields.get("pluginSecret") != Some(input.secret())
        || fields.get("pluginPublic") != Some(input.public())
    {
        return Err(FreeEnrollmentError::InvalidInput);
    }
    let version = storage::version_fields(
        fields
            .get(PRIVATE_FIELD)
            .ok_or(FreeEnrollmentError::InvalidRecord)?,
    )?;
    if version.get("revision").and_then(Value::as_u64) != Some(1) {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    Ok(())
}

pub(super) fn insert(
    conn: &Connection,
    key: &[u8; 32],
    index: usize,
    retained: &mut RetainedRows,
) -> Result<(), FreeEnrollmentError> {
    let bytes = retained.encode_record(index)?;
    let envelope = crypto::seal_with_key(retained.buffer(bytes), key)
        .map_err(|_| FreeEnrollmentError::Storage)?;
    if envelope.len() > storage::MAX_ROW_BYTES {
        return Err(FreeEnrollmentError::LimitExceeded);
    }
    let record = retained.record(index);
    let changed = conn.execute(
        "INSERT INTO main.accounts(id,provider,priority,enabled,added_at,data) VALUES(?1,?2,?3,?4,?5,?6)",
        rusqlite::params![record.id(), PROVIDER, record.priority(), i64::from(record.enabled()), record.added_at(), envelope],
    ).map_err(|_| FreeEnrollmentError::Storage)?;
    if changed != 1 {
        return Err(FreeEnrollmentError::Storage);
    }
    Ok(())
}

fn next_priority(conn: &Connection) -> Result<i64, FreeEnrollmentError> {
    let max = conn
        .query_row(
            "SELECT MAX(priority) FROM main.accounts",
            [],
            |row| match row.get_ref(0)? {
                ValueRef::Null => Ok(None),
                ValueRef::Integer(value) => Ok(Some(value)),
                _ => Err(rusqlite::Error::InvalidQuery),
            },
        )
        .map_err(|_| FreeEnrollmentError::InvalidRecord)?;
    let next = max
        .map(priority::normalize_priority_value)
        .unwrap_or(priority::DEFAULT_PRIORITY - 1)
        + 1;
    if next <= priority::MAX_PRIORITY && !priority_occupied(conn, next)? {
        return Ok(next);
    }
    for candidate in priority::MIN_PRIORITY..=priority::MAX_PRIORITY {
        if !priority_occupied(conn, candidate)? {
            return Ok(candidate);
        }
    }
    Err(FreeEnrollmentError::PriorityExhausted)
}
fn priority_occupied(conn: &Connection, priority: i64) -> Result<bool, FreeEnrollmentError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.accounts WHERE priority=?1)",
        [priority],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|_| FreeEnrollmentError::Storage)
}

pub(super) fn check_fields(fields: &Map<String, Value>) -> Result<(), FreeEnrollmentError> {
    let mut remaining = MAX_NODES - 1;
    for (key, value) in fields {
        if key.len() > storage::MAX_PLAIN_BYTES {
            return Err(FreeEnrollmentError::LimitExceeded);
        }
        shape(value, 1, &mut remaining)?;
    }
    for (name, limit) in [
        ("pluginSecret", storage::MAX_SECRET_BYTES),
        ("pluginPublic", storage::MAX_PUBLIC_BYTES),
    ] {
        if let Some(value) = fields.get(name).filter(|value| !value.is_null()) {
            if !value.is_object() {
                return Err(FreeEnrollmentError::InvalidRecord);
            }
            reject_reserved(value)?;
            let mut count = Count { bytes: 0, limit };
            serde_json::to_writer(&mut count, value)
                .map_err(|_| FreeEnrollmentError::LimitExceeded)?;
        }
    }
    let mut count = Count {
        bytes: 0,
        limit: storage::MAX_PLAIN_BYTES,
    };
    serde_json::to_writer(&mut count, fields).map_err(|_| FreeEnrollmentError::LimitExceeded)?;
    Ok(())
}
fn reject_reserved(value: &Value) -> Result<(), FreeEnrollmentError> {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key == PRIVATE_FIELD {
                    return Err(FreeEnrollmentError::InvalidRecord);
                }
                reject_reserved(value)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_reserved(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn check_shape(value: &Value) -> Result<(), FreeEnrollmentError> {
    let mut remaining = MAX_NODES;
    shape(value, 0, &mut remaining)
}
fn shape(value: &Value, depth: usize, remaining: &mut usize) -> Result<(), FreeEnrollmentError> {
    if *remaining == 0 {
        return Err(FreeEnrollmentError::LimitExceeded);
    }
    *remaining -= 1;
    match value {
        Value::String(text) if text.len() > storage::MAX_PLAIN_BYTES => {
            return Err(FreeEnrollmentError::LimitExceeded)
        }
        Value::Object(fields) => {
            if depth >= MAX_DEPTH {
                return Err(FreeEnrollmentError::LimitExceeded);
            }
            for (key, value) in fields {
                if key.len() > storage::MAX_PLAIN_BYTES {
                    return Err(FreeEnrollmentError::LimitExceeded);
                }
                shape(value, depth + 1, remaining)?;
            }
        }
        Value::Array(items) => {
            if depth >= MAX_DEPTH {
                return Err(FreeEnrollmentError::LimitExceeded);
            }
            for value in items {
                shape(value, depth + 1, remaining)?;
            }
        }
        _ => {}
    }
    Ok(())
}

struct Count {
    bytes: usize,
    limit: usize,
}
impl io::Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes) {
            return Err(io::Error::other("Free JSON limit"));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn parse(
    bytes: &[u8],
    arena: &mut Vec<Value>,
    limit: usize,
) -> Result<usize, FreeEnrollmentError> {
    if bytes.len() > limit {
        return Err(FreeEnrollmentError::LimitExceeded);
    }
    let mut remaining = MAX_NODES;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let root = Node {
        arena,
        remaining: &mut remaining,
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| FreeEnrollmentError::InvalidRecord)?;
    deserializer
        .end()
        .map_err(|_| FreeEnrollmentError::InvalidRecord)?;
    Ok(root)
}
struct Node<'a> {
    arena: &'a mut Vec<Value>,
    remaining: &'a mut usize,
    depth: usize,
}
impl Node<'_> {
    fn retain(&mut self, value: Value) -> usize {
        let index = self.arena.len();
        self.arena.push(value);
        index
    }
}
impl<'de> DeserializeSeed<'de> for Node<'_> {
    type Value = usize;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<usize, D::Error> {
        if *self.remaining == 0 {
            return Err(de::Error::custom("Free JSON nodes"));
        }
        *self.remaining -= 1;
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Node<'_> {
    type Value = usize;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded Free JSON")
    }
    fn visit_bool<E: de::Error>(mut self, value: bool) -> Result<usize, E> {
        Ok(self.retain(Value::Bool(value)))
    }
    fn visit_i64<E: de::Error>(mut self, value: i64) -> Result<usize, E> {
        Ok(self.retain(Value::from(value)))
    }
    fn visit_u64<E: de::Error>(mut self, value: u64) -> Result<usize, E> {
        Ok(self.retain(Value::from(value)))
    }
    fn visit_f64<E: de::Error>(mut self, value: f64) -> Result<usize, E> {
        let value = Number::from_f64(value).ok_or_else(|| de::Error::custom("Free JSON number"))?;
        Ok(self.retain(Value::Number(value)))
    }
    fn visit_str<E: de::Error>(mut self, value: &str) -> Result<usize, E> {
        Ok(self.retain(Value::String(value.to_string())))
    }
    fn visit_string<E: de::Error>(mut self, value: String) -> Result<usize, E> {
        Ok(self.retain(Value::String(value)))
    }
    fn visit_unit<E: de::Error>(mut self) -> Result<usize, E> {
        Ok(self.retain(Value::Null))
    }
    fn visit_seq<A: SeqAccess<'de>>(mut self, mut sequence: A) -> Result<usize, A::Error> {
        if self.depth >= MAX_DEPTH {
            return Err(de::Error::custom("Free JSON depth"));
        }
        let root = self.retain(Value::Array(Vec::new()));
        while let Some(child) = sequence.next_element_seed(Node {
            arena: &mut *self.arena,
            remaining: &mut *self.remaining,
            depth: self.depth + 1,
        })? {
            let value = std::mem::replace(&mut self.arena[child], Value::Null);
            if let Value::Array(items) = &mut self.arena[root] {
                items.push(value);
            }
        }
        Ok(root)
    }
    fn visit_map<A: MapAccess<'de>>(mut self, mut object: A) -> Result<usize, A::Error> {
        if self.depth >= MAX_DEPTH {
            return Err(de::Error::custom("Free JSON depth"));
        }
        let root = self.retain(Value::Object(Map::new()));
        while let Some(key) = object.next_key::<String>()? {
            let key = self.retain(Value::String(key));
            let text = self.arena[key]
                .as_str()
                .ok_or_else(|| de::Error::custom("Free JSON key"))?;
            if text.len() > storage::MAX_PLAIN_BYTES || self.arena[root].get(text).is_some() {
                return Err(de::Error::custom("Free JSON duplicate"));
            }
            let child = object.next_value_seed(Node {
                arena: &mut *self.arena,
                remaining: &mut *self.remaining,
                depth: self.depth + 1,
            })?;
            let Value::String(key) = std::mem::replace(&mut self.arena[key], Value::Null) else {
                return Err(de::Error::custom("Free JSON key"));
            };
            let value = std::mem::replace(&mut self.arena[child], Value::Null);
            if let Value::Object(fields) = &mut self.arena[root] {
                fields.insert(key, value);
            }
        }
        Ok(root)
    }
}
