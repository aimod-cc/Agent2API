//! 原WorkBuddy账号的条件冷却；只写原账号及其提供商范围，不补建账号，不把内容摘要当出生代次。

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::server::config::error_policy::{CooldownScope, ErrorRule};
use crate::server::core::account_store::policy_cooldowns::cooldown_entry;
use crate::server::core::upstream::cancellation::CancelToken;

use super::{decode_data, encode_data};
use crate::server::core::account_store::state::StoredAccount;

const MAX_ROW: usize = 2 * 1024 * 1024;

pub(crate) struct Stamp {
    id: String,
    model: String,
    rowid: i64,
    source: [u8; 32],
    data_version: i64,
}

fn read(conn: &Connection, id: &str) -> Result<(i64, StoredAccount), String> {
    let row: Option<(i64, String)> = conn.query_row(
        "SELECT rowid,data FROM main.accounts WHERE id=?1 AND provider='workbuddy' AND octet_length(data)<=?2",
        params![id, MAX_ROW as i64], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(|_| unavailable())?;
    let (rowid, data) = row.ok_or_else(unavailable)?;
    if data.len() > MAX_ROW { return Err(unavailable()); }
    let record = decode_data(&data).map_err(|_| unavailable())?;
    if record.id() != id || record.get("provider").and_then(Value::as_str) != Some("workbuddy") {
        return Err(unavailable());
    }
    Ok((rowid, record))
}

fn source(record: &StoredAccount) -> [u8; 32] {
    // 只排除运营/统计字段；其余未知字段同样参与内容核对。仅存摘要，不保留凭据或代理。
    let mut hash = Sha256::new();
    for (key, value) in record.fields() {
        if matches!(key.as_str(), "name" | "nickname" | "nameCustom" | "updatedAt" | "priority" | "enabled"
            | "proxy" | "rateLimits" | "pluginPublic" | "pluginState" | "usageQuery" | "checkinAt" | "streakDays"
            | "upstreamCooldownUntil" | "upstreamBlockedReason" | "lowBalance" | "limiters" | "limiterGroups"
            | "maxConcurrent" | "maxConcurrentConfigured") || key.starts_with("pluginLedger.") { continue; }
        let encoded = value.to_string();
        hash.update((key.len() as u64).to_be_bytes());
        hash.update(key.as_bytes());
        hash.update((encoded.len() as u64).to_be_bytes());
        hash.update(encoded.as_bytes());
    }
    hash.finalize().into()
}

pub(crate) fn capture(conn: &Connection, id: &str, model: &str) -> Result<Stamp, String> {
    if id.is_empty() || id.len() > 512 || model.is_empty() || model.len() > 512
        || id.chars().chain(model.chars()).any(char::is_control) { return Err(unavailable()); }
    let data_version = conn.pragma_query_value(None, "data_version", |row| row.get(0)).map_err(|_| unavailable())?;
    let (rowid, record) = read(conn, id)?;
    Ok(Stamp { id: id.into(), model: model.into(), rowid, source: source(&record), data_version })
}

pub(crate) fn apply(
    conn: &mut Connection, stamp: Stamp, status: u16, code: Option<i64>, reset_at: i64,
    message: &str, rule: &ErrorRule, scope: CooldownScope, floor_ms: u64, deadline: Instant,
    cancel: Option<&CancelToken>, abandoned: &AtomicBool, unchanged: impl FnOnce(i64) -> bool,
) -> Result<Value, String> {
    check(deadline, cancel, abandoned)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_| unavailable())?;
    check(deadline, cancel, abandoned)?;
    let version: i64 = tx.pragma_query_value(None, "data_version", |row| row.get(0)).map_err(|_| unavailable())?;
    let (rowid, mut record) = read(&tx, &stamp.id)?;
    // 日志回绕/缺失或原行任何修改均保守拒绝；不是账号完整出生代次或备份恢复防ABA。
    if version != stamp.data_version || rowid != stamp.rowid || source(&record) != stamp.source
        || !unchanged(rowid) { return Err(unavailable()); }
    let now = crate::server::logging::now_ms();
    if now <= 0 || !matches!(status, 402 | 429) || reset_at <= now || reset_at > now.saturating_add(8 * 86_400_000)
        || reset_at > (1_i64 << 53) - 1 || chrono::DateTime::from_timestamp_millis(reset_at).is_none() {
        return Err("WorkBuddy未提供有效冷却窗口，未猜测恢复时刻".into());
    }
    let (field, key) = match scope {
        CooldownScope::Provider => ("", "@provider".to_string()),
        CooldownScope::Account => ("routingCooldowns", "@account".to_string()),
        _ => ("rateLimits", stamp.model.clone()),
    };
    let mut provider_state = if scope == CooldownScope::Provider {
        let encoded: Option<String> = tx.query_row("SELECT value FROM kv WHERE key='routingProviderCooldowns'", [],
            |row| row.get(0)).optional().map_err(|_| unavailable())?;
        encoded.and_then(|value| serde_json::from_str::<Value>(&value).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
    } else { Value::Null };
    let existing = if scope == CooldownScope::Provider { provider_state.get("workbuddy") } else { record.get(field) };
    let mut limits = match existing {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(limits)) if limits.len() <= 1024 => limits.clone(),
        _ => return Err(unavailable()),
    };
    limits.retain(|_, value| value["resetAt"].as_i64().is_some_and(|at| at.saturating_add(1_800_000) > now));
    if limits.len() >= 1024 && !limits.contains_key(&key) { return Err(unavailable()); }
    let entry = cooldown_entry(limits.get(&key), i32::from(status), code, Some(reset_at), message, rule, scope, floor_ms, now);
    limits.insert(key, entry.clone());
    if scope == CooldownScope::Provider {
        provider_state["workbuddy"] = Value::Object(limits);
        check(deadline, cancel, abandoned)?;
        tx.execute("INSERT INTO kv(key,value) VALUES ('routingProviderCooldowns',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [provider_state.to_string()]).map_err(|_| unavailable())?;
    } else {
        record.set(field, Value::Object(limits));
        let data = encode_data(&record).map_err(|_| unavailable())?;
        if data.len() > MAX_ROW { return Err(unavailable()); }
        check(deadline, cancel, abandoned)?;
        let changed = tx.execute("UPDATE main.accounts SET data=?1 WHERE id=?2 AND provider='workbuddy' AND rowid=?3",
            params![data, stamp.id, stamp.rowid]).map_err(|_| unavailable())?;
        if changed != 1 { return Err(unavailable()); }
    }
    check(deadline, cancel, abandoned)?;
    tx.commit().map_err(|_| unavailable())?;
    Ok(entry)
}

fn check(deadline: Instant, cancel: Option<&CancelToken>, abandoned: &AtomicBool) -> Result<(), String> {
    if Instant::now() >= deadline || cancel.is_some_and(CancelToken::is_cancelled) || abandoned.load(Ordering::Acquire) {
        Err("WorkBuddy冷却记录操作已取消或到限，未确认保存".into())
    } else { Ok(()) }
}
fn unavailable() -> String { "WorkBuddy原账号来源已变化或冷却记录存储不可用，未确认保存".into() }
