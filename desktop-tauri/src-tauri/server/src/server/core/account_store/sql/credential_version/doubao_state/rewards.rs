//! 豆包其他任务领奖的固定耐久记忆；不改每日签到记忆，Pending不按超时、跨日或查询false恢复。

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::{envelope::Envelope, record::{self, RawSlot}, retained_json, RowStamp, StateError, StateFailure, state_error};

const KEY: &str = "host:doubaowork:task-rewards";
const MAX_ENTRIES: usize = 128;
const MAX_MEMORY: usize = 256 * 1024;

pub(in crate::server::core::account_store) struct Retained {
    raw: RawSlot,
    written: RawSlot,
    output: Envelope,
    displaced: Vec<Value>,
}
impl Retained {
    pub(in crate::server::core::account_store) fn new() -> Self {
        Self { raw: RawSlot::default(), written: RawSlot::default(), output: Envelope::default(), displaced: Vec::new() }
    }
}

pub(in crate::server::core::account_store) fn read(
    conn: &Connection, key: &[u8; 32], stamp: &RowStamp, retained: &mut Retained,
) -> Result<Value, StateError> {
    record::load_raw_into(conn, key, stamp, &mut retained.raw)?;
    retained.raw.verify(conn, stamp)?;
    memory(&retained.raw.value)
}

pub(in crate::server::core::account_store) fn verify(
    conn: &Connection, key: &[u8; 32], stamp: &RowStamp, expected: &Value, retained: &mut Retained,
) -> Result<(), StateError> {
    let current = read(conn, key, stamp, retained)?;
    if &current != expected { return Err(state_error(StateFailure::StateChanged)); }
    Ok(())
}

pub(in crate::server::core::account_store) fn write(
    conn: &Connection, key: &[u8; 32], stamp: &RowStamp, expected: &Value, next: &Value, retained: &mut Retained,
) -> Result<(), StateError> {
    verify(conn, key, stamp, expected, retained)?;
    validate(next)?;
    let before = expected.get("revision").and_then(Value::as_u64).unwrap_or(0);
    if next["revision"].as_u64() != before.checked_add(1) { return Err(state_error(StateFailure::RevisionExhausted)); }
    let raw = retained.raw.value.as_object_mut().ok_or_else(|| state_error(StateFailure::InvalidContainer))?;
    if !raw.contains_key("pluginState") { raw.insert("pluginState".into(), Value::Object(Map::new())); }
    let fields = raw.get_mut("pluginState").and_then(Value::as_object_mut).ok_or_else(|| state_error(StateFailure::InvalidContainer))?;
    if fields.len() >= 128 && !fields.contains_key(KEY) { return Err(state_error(StateFailure::InvalidContainer)); }
    if let Some(previous) = fields.insert(KEY.to_string(), next.clone()) { retained.displaced.push(previous); }
    record::update_raw(conn, key, stamp, &retained.raw.value, &mut retained.output)?;
    record::load_raw_into(conn, key, stamp, &mut retained.written)?;
    retained.written.verify(conn, stamp)?;
    if retained.written.value != retained.raw.value || memory(&retained.written.value)? != *next {
        return Err(state_error(StateFailure::StateChanged));
    }
    Ok(())
}

fn memory(raw: &Value) -> Result<Value, StateError> {
    let root = raw.as_object().ok_or_else(|| state_error(StateFailure::InvalidContainer))?;
    let entry = match root.get("pluginState") {
        None => return Ok(Value::Null),
        Some(Value::Object(fields)) if fields.len() <= 128 => fields.get(KEY),
        _ => return Err(state_error(StateFailure::InvalidContainer)),
    };
    match entry { None => Ok(Value::Null), Some(value) => { validate(value)?; Ok(value.clone()) } }
}

fn validate(value: &Value) -> Result<(), StateError> {
    retained_json::check(value, MAX_MEMORY)?;
    let fields = value.as_object().ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    if fields.len() != 3 || fields.get("schema").and_then(Value::as_u64) != Some(1)
        || fields.get("revision").and_then(Value::as_u64).is_none_or(|revision| revision == 0) {
        return Err(state_error(StateFailure::InvalidEntry));
    }
    let state = fields.get("value").and_then(Value::as_object).ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    if state.len() != 1 { return Err(state_error(StateFailure::InvalidEntry)); }
    let entries = state.get("entries").and_then(Value::as_array).filter(|entries| entries.len() <= MAX_ENTRIES)
        .ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    let mut keys = std::collections::BTreeSet::new();
    for entry in entries {
        let fields = entry.as_object().ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
        if fields.len() != 6 || !["pending", "confirmed"].contains(&entry["state"].as_str().unwrap_or(""))
            || entry["at"].as_i64().is_none_or(|at| at <= 0)
            || entry["operation"].as_str().is_none_or(|operation| operation.len() != 64 || !operation.bytes().all(|byte| byte.is_ascii_hexdigit())) {
            return Err(state_error(StateFailure::InvalidEntry));
        }
        let mut identity = Vec::new();
        for name in ["group", "item", "task"] {
            let text = entry[name].as_str().filter(|text| text.len() <= 256 && text.trim() == *text
                && !text.chars().any(|ch| ch.is_control() || ch.is_whitespace()) && !text.contains("://"))
                .ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
            if name != "task" && text.is_empty() { return Err(state_error(StateFailure::InvalidEntry)); }
            identity.push(text);
        }
        if !keys.insert(identity) { return Err(state_error(StateFailure::InvalidEntry)); }
    }
    Ok(())
}

pub(in crate::server::core::account_store) fn pending(
    expected: &Value, group: &str, item: &str, task: &str, operation: &str, at: i64,
) -> Result<Value, StateError> {
    let mut entries = expected.pointer("/value/entries").and_then(Value::as_array).cloned().unwrap_or_default();
    if entries.len() >= MAX_ENTRIES || entries.iter().any(|entry| entry["group"] == group && entry["item"] == item && entry["task"] == task) {
        return Err(state_error(StateFailure::CandidateChanged));
    }
    entries.push(json!({"group": group, "item": item, "task": task, "operation": operation, "at": at, "state": "pending"}));
    let revision = expected.get("revision").and_then(Value::as_u64).unwrap_or(0).checked_add(1)
        .ok_or_else(|| state_error(StateFailure::RevisionExhausted))?;
    let next = json!({"schema": 1, "revision": revision, "value": {"entries": entries}});
    validate(&next)?;
    Ok(next)
}

pub(in crate::server::core::account_store) fn confirmed(expected: &Value, operation: &str, at: i64) -> Result<Value, StateError> {
    validate(expected)?;
    let mut next = expected.clone();
    let entries = next.pointer_mut("/value/entries").and_then(Value::as_array_mut).ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    let entry = entries.iter_mut().find(|entry| entry["operation"] == operation && entry["state"] == "pending")
        .ok_or_else(|| state_error(StateFailure::CandidateChanged))?;
    entry["state"] = json!("confirmed");
    entry["at"] = json!(at);
    next["revision"] = json!(expected["revision"].as_u64().and_then(|value| value.checked_add(1)).ok_or_else(|| state_error(StateFailure::RevisionExhausted))?);
    validate(&next)?;
    Ok(next)
}
