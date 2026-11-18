//! WASM持久状态严格区分缺失与损坏；只读本家原账号，不证明出生代次或发送资格。

use std::io::{self, Write};

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::super::{sql, state::StoredAccount, AccountStore};

const MAX_ENTRY_BYTES: usize = 256 * 1024;
const MAX_STATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;
const INVALID: &str = "账号插件状态格式无效，未将损坏记录当作缺失或覆盖";
const UNAVAILABLE: &str = "账号插件状态读取或保存未确认，未使用空状态继续";

fn target(provider: &str, id: &str, key: &str) -> Result<(), String> {
    if provider.is_empty() || provider.len() > 128 || id.is_empty() || id.len() > 512
        || key.is_empty() || key.len() > 160
        || provider.chars().chain(id.chars()).chain(key.chars()).any(char::is_control)
    {
        return Err("插件状态缺少有效的账号归属或状态名称".into());
    }
    Ok(())
}

fn record(conn: &Connection, provider: &str, id: &str) -> Result<StoredAccount, String> {
    let record = sql::load_by_id(conn, id).map_err(|_| UNAVAILABLE)?
        .ok_or("插件状态所属账号不存在，未返回空状态")?;
    if record.id() != id || record.get("provider").and_then(Value::as_str) != Some(provider) {
        return Err("插件状态所属账号或提供商已变化".into());
    }
    Ok(record)
}

fn container(record: &StoredAccount) -> Result<Option<&Map<String, Value>>, String> {
    match record.get("pluginState") {
        None => Ok(None),
        Some(Value::Object(state)) => {
            if state.len() > MAX_ENTRIES || !within_limit(state, MAX_STATE_BYTES) {
                return Err(INVALID.into());
            }
            Ok(Some(state))
        }
        Some(_) => Err(INVALID.into()),
    }
}

fn entry(value: &Value) -> Result<(u64, u32), String> {
    let fields = value.as_object().ok_or(INVALID)?;
    if fields.len() != 3 || !fields.contains_key("value") {
        return Err(INVALID.into());
    }
    let revision = fields.get("revision").and_then(Value::as_u64).filter(|revision| *revision > 0)
        .ok_or(INVALID)?;
    let schema = fields.get("schema").and_then(Value::as_u64)
        .and_then(|schema| u32::try_from(schema).ok()).filter(|schema| *schema > 0).ok_or(INVALID)?;
    if !within_limit(&fields["value"], MAX_ENTRY_BYTES) { return Err(INVALID.into()); }
    Ok((revision, schema))
}

impl AccountStore {
    pub(crate) fn plugin_state_strict(&self, provider: &str, id: &str, key: &str) -> Result<Value, String> {
        target(provider, id, key)?;
        let guard = self.guard();
        self.with_conn(&guard, |conn| {
            let result: Result<Value, String> = (|| {
                let record = record(conn, provider, id)?;
                let state = container(&record)?;
                let Some(value) = state.and_then(|state| state.get(key)) else {
                    return Ok(json!({ "present": false, "revision": 0, "schema": 1, "value": null }));
                };
                entry(value)?;
                let mut result = value.clone();
                result["present"] = json!(true);
                Ok(result)
            })();
            Ok(result)
        }).map_err(|_| UNAVAILABLE.to_string())?
    }

    pub(crate) fn plugin_state_compare_exchange_strict(
        &self, provider: &str, id: &str, key: &str, expected: u64, schema: u32, value: Value,
    ) -> Result<Value, String> {
        target(provider, id, key)?;
        if schema == 0 || !within_limit(&value, MAX_ENTRY_BYTES) {
            return Err("插件状态版本或大小无效".into());
        }
        let db = self.database().ok_or(UNAVAILABLE)?;
        let guard = self.guard();
        let result = db.plugin_state_transaction(|conn| {
            let mut record = record(conn, provider, id)?;
            let current = container(&record)?;
            let revision = current.and_then(|state| state.get(key)).map(entry).transpose()?
                .map(|(revision, _)| revision).unwrap_or(0);
            if revision != expected { return Ok(json!({ "updated": false, "revision": revision })); }
            let mut state = current.cloned().unwrap_or_default();
            if !state.contains_key(key) && state.len() >= MAX_ENTRIES {
                return Err("账号插件状态条数超过上限".to_string());
            }
            let revision = revision.checked_add(1).ok_or("账号插件状态版本已耗尽")?;
            state.insert(key.to_string(), json!({ "revision": revision, "schema": schema, "value": value }));
            if !within_limit(&state, MAX_STATE_BYTES) { return Err("账号插件状态总量超过 2 MiB".into()); }
            record.set("pluginState", Value::Object(state));
            sql::update_in_place(conn, &record).map_err(|_| UNAVAILABLE)?;
            let saved = self::record(conn, provider, id)?;
            if saved.get("pluginState") != record.get("pluginState") { return Err(UNAVAILABLE.into()); }
            Ok(json!({ "updated": true, "revision": revision }))
        });
        drop(guard);
        match result {
            crate::server::db::Durable::Committed(value) => Ok(value),
            crate::server::db::Durable::Rejected(error) => Err(error),
            _ => Err(UNAVAILABLE.into()),
        }
    }
}

struct Budget { used: usize, limit: usize }
impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.used = self.used.checked_add(bytes.len()).filter(|used| *used <= self.limit)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "state limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

fn within_limit(value: &impl serde::Serialize, limit: usize) -> bool {
    serde_json::to_writer(Budget { used: 0, limit }, value).is_ok()
}
