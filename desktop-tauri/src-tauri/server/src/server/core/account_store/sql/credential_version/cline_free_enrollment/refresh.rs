//! Free续期只处理原加密账号行与既有双锚槽；未知结果保留Pending，不补Ready。

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::{identity, slot, storage, FreeEnrollmentError as Error, RetainedRows, StoredAccount, PRIVATE_FIELD};

pub(in crate::server::core::account_store) struct Ready {
    pub(in crate::server::core::account_store) record: StoredAccount,
    slot: Value,
}

const AUTHORIZATION_FIELDS: &[&str] = &[
    "source", "pluginIdentity", "pluginIdentityContract", "pluginSecret", "account", "uid",
    "desktop", "userAgent", "proxy", "accessToken", "access_token", "token", "refreshToken", "refresh_token",
    "expiresAt", "auth", "metadata", "userInfo", "pool", "needsRelogin", "accountId", "account_id", "userId", "user_id",
];

pub(in crate::server::core::account_store::sql) fn authorization(record: &StoredAccount) -> Value {
    Value::Object(AUTHORIZATION_FIELDS.iter().filter_map(|name|
        record.get(name).map(|value| ((*name).to_string(), value.clone()))).collect::<Map<_, _>>())
}

pub(in crate::server::core::account_store) fn read(
    conn: &Connection, key: &[u8; 32], id: &str, retained: &mut RetainedRows,
) -> Result<Ready, Error> {
    storage::ensure_no_triggers(conn)?;
    let registered: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='cline_free_refresh_slots' AND type='table')",
        [], |row| row.get(0),
    ).map_err(|_| Error::Storage)?;
    if registered == 0 { return Err(Error::RefreshRegistrationMissing); }
    slot::validate_schema(conn)?;
    let index = storage::load_account(conn, key, id, retained)?;
    identity::verify_refresh(index, retained)?;
    let record = retained.record(index);
    let identity = record.get("pluginIdentity").and_then(Value::as_str).ok_or(Error::IdentityConflict)?;
    let contract = crate::server::core::account_store::plugin_contract::contract_id("cline-free", identity, false);
    let identity_key = contract.strip_prefix("cline-free-account-").ok_or(Error::IdentityConflict)?;
    if record.get("pluginPublic").and_then(|public| public.get("canRefresh")).and_then(Value::as_bool) != Some(true)
        || record.get("source").and_then(Value::as_str) != Some("plugin") {
        return Err(Error::IdentityConflict);
    }
    let identity = identity.to_string();
    let version = storage::version_fields(record.get(PRIVATE_FIELD).ok_or(Error::RecoveryIsolated)?)?.clone();
    let epoch = storage::epoch(conn, retained)?.ok_or(Error::RecoveryIsolated)?;
    if version["dbEpoch"].as_str() != Some(retained.text(epoch)?) { return Err(Error::RecoveryIsolated); }
    let original = slot::readback(conn, key, id, identity_key, retained)?;
    let value = retained.value(original);
    if value.as_object().is_none_or(|fields| fields.len() != 10)
        || value["provider"] != "cline-free" || value["accountId"] != id || value["uid"] != identity
        || value["identityKey"] != identity_key || value["schema"] != 2
        || ["dbEpoch", "born", "revision"].iter().any(|name| value[*name] != version[*name])
        || value["generation"].as_str().is_none_or(|value| !storage::random_value(value))
        || value["state"].as_object().is_none_or(|state| state.len() != 2)
        || value["state"]["kind"] != "ready"
        || !(value["state"]["settledOperation"].is_null()
            || value["state"]["settledOperation"].as_str().is_some_and(storage::random_value)) {
        return Err(Error::RecoveryIsolated);
    }
    Ok(Ready { record: retained.record(index).clone(), slot: value.clone() })
}

fn current(conn: &Connection, key: &[u8; 32], ready: &Ready, retained: &mut RetainedRows) -> Result<usize, Error> {
    storage::ensure_no_triggers(conn)?;
    slot::validate_schema(conn)?;
    let index = storage::load_account(conn, key, ready.record.id(), retained)?;
    identity::verify_refresh(index, retained)?;
    let record = retained.record(index);
    if record.get(PRIVATE_FIELD) != ready.record.get(PRIVATE_FIELD)
        || AUTHORIZATION_FIELDS.iter().any(|name| record.get(name) != ready.record.get(name)) {
        return Err(Error::SourceChanged);
    }
    let epoch = storage::epoch(conn, retained)?.ok_or(Error::RecoveryIsolated)?;
    if ready.slot["dbEpoch"].as_str() != Some(retained.text(epoch)?) { return Err(Error::RecoveryIsolated); }
    Ok(index)
}

fn pending_state(ready: &Ready, operation: &str) -> Result<Value, Error> {
    use sha2::{Digest, Sha256};
    let token = identity::refresh_token(&ready.record)?.ok_or(Error::InvalidRecord)?;
    Ok(json!({ "kind": "pending", "operation": operation,
        "refreshDigest": format!("{:x}", Sha256::digest(token.as_bytes())) }))
}

pub(in crate::server::core::account_store) fn begin(
    conn: &Connection, key: &[u8; 32], ready: &Ready, operation: &str, retained: &mut RetainedRows,
) -> Result<(), Error> {
    current(conn, key, ready, retained)?;
    let identity_key = ready.slot["identityKey"].as_str().ok_or(Error::InvalidRecord)?;
    let before = slot::readback(conn, key, ready.record.id(), identity_key, retained)?;
    if retained.value(before) != &ready.slot || !storage::random_value(operation) { return Err(Error::SourceChanged); }
    let mut next = ready.slot.clone();
    next["state"] = pending_state(ready, operation)?;
    write_slot(conn, key, ready, &next, retained)
}

pub(in crate::server::core::account_store) fn verify_pending(
    conn: &Connection, key: &[u8; 32], ready: &Ready, operation: &str, retained: &mut RetainedRows,
) -> Result<usize, Error> {
    if !storage::random_value(operation) { return Err(Error::InvalidRecord); }
    let index = current(conn, key, ready, retained)?;
    let before = slot::readback(conn, key, ready.record.id(), ready.slot["identityKey"].as_str().ok_or(Error::InvalidRecord)?, retained)?;
    let mut pending = ready.slot.clone();
    pending["state"] = pending_state(ready, operation)?;
    if retained.value(before) != &pending { return Err(Error::SourceChanged); }
    Ok(index)
}

pub(in crate::server::core::account_store) fn complete(
    conn: &Connection, key: &[u8; 32], ready: &Ready, operation: &str, replacement: &Value, public: &Value,
    retained: &mut RetainedRows,
) -> Result<(), Error> {
    let index = verify_pending(conn, key, ready, operation, retained)?;
    if replacement["account"] != ready.slot["uid"] || !replacement.is_object() || !public.is_object() {
        return Err(Error::IdentityConflict);
    }
    let now = crate::server::logging::now_ms();
    if now <= 0 || replacement.get("expiresAt").is_some_and(|value|
        !value.is_null() && value.as_i64().is_none_or(|expiry| expiry <= now)) { return Err(Error::Expired); }
    let revision = ready.slot["revision"].as_u64().and_then(|value| value.checked_add(1)).ok_or(Error::Storage)?;
    let secret = retained.retain_value(ready.record.get("pluginSecret").cloned().ok_or(Error::InvalidRecord)?);
    for (name, value) in replacement.as_object().ok_or(Error::InvalidRecord)? {
        retained.set_value(secret, name, value.clone())?;
    }
    for (aliases, canonical) in [(&["access_token", "token"][..], "accessToken"), (&["refresh_token"][..], "refreshToken")] {
        for name in aliases {
            if retained.value(secret).get(*name).is_some() {
                retained.set_value(secret, name, replacement[canonical].clone())?;
            }
        }
    }
    retained.move_to_record(index, "pluginSecret", secret)?;
    let public_value = retained.retain_value(retained.record(index).get("pluginPublic").cloned().unwrap_or_else(|| json!({})));
    for (name, value) in public.as_object().ok_or(Error::InvalidRecord)? { retained.set_value(public_value, name, value.clone())?; }
    retained.move_to_record(index, "pluginPublic", public_value)?;
    for (aliases, canonical) in [(&["accessToken", "access_token", "token"][..], "accessToken"),
        (&["refreshToken", "refresh_token"][..], "refreshToken"), (&["expiresAt"][..], "expiresAt")] {
        for name in aliases {
            if ready.record.get(name).is_some() { retained.set_record(index, name, replacement[canonical].clone()); }
        }
    }
    let version = retained.retain_value(ready.record.get(PRIVATE_FIELD).cloned().ok_or(Error::RecoveryIsolated)?);
    retained.set_value(version, "revision", json!(revision))?;
    retained.move_to_record(index, PRIVATE_FIELD, version)?;
    retained.set_record(index, "updatedAt", json!(now));
    identity::verify_refresh(index, retained)?;
    let bytes = retained.encode_record(index)?;
    let encrypted = crate::server::core::crypto::seal_with_key(retained.buffer(bytes), key).map_err(|_| Error::Storage)?;
    if encrypted.len() > storage::MAX_ROW_BYTES { return Err(Error::LimitExceeded); }
    if conn.execute("UPDATE main.accounts SET data=?3 WHERE id=?1 AND provider=?2",
        rusqlite::params![ready.record.id(), "cline-free", encrypted]).map_err(|_| Error::Storage)? != 1 { return Err(Error::Storage); }
    let readback = storage::load_account(conn, key, ready.record.id(), retained)?;
    if retained.record(index).fields() != retained.record(readback).fields() { return Err(Error::Storage); }
    let mut next = ready.slot.clone();
    next["revision"] = json!(revision);
    let generation = storage::random_id()?;
    if next["generation"].as_str() == Some(&generation) { return Err(Error::RandomUnavailable); }
    next["generation"] = json!(generation);
    next["state"] = json!({"kind": "ready", "settledOperation": operation});
    write_slot(conn, key, ready, &next, retained)
}

fn write_slot(conn: &Connection, key: &[u8; 32], ready: &Ready, next: &Value, retained: &mut RetainedRows) -> Result<(), Error> {
    let value = retained.retain_value(next.clone());
    let bytes = retained.encode_value(value, 4096)?;
    let encrypted = crate::server::core::crypto::seal_with_key(retained.buffer(bytes), key).map_err(|_| Error::Storage)?;
    if conn.execute("UPDATE main.cline_free_refresh_slots SET payload=?3 WHERE account_id=?1 AND identity_key=?2",
        rusqlite::params![ready.record.id(), ready.slot["identityKey"].as_str().ok_or(Error::InvalidRecord)?, encrypted])
        .map_err(|_| Error::Storage)? != 1 { return Err(Error::Storage); }
    let readback = slot::readback(conn, key, ready.record.id(), ready.slot["identityKey"].as_str().ok_or(Error::InvalidRecord)?, retained)?;
    if retained.value(readback) != next { return Err(Error::Storage); }
    Ok(())
}

impl Drop for Ready {
    fn drop(&mut self) { storage::wipe_record(&mut self.record); }
}
