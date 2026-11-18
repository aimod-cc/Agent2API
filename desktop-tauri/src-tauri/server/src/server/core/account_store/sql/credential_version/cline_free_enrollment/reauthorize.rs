use rusqlite::Connection;
use serde_json::{json, Value};

use super::{identity, record, slot, storage, FreeEnrollmentError as Error, FreeEnrollmentInput, RetainedRows};
use crate::server::core::{account_store::{plugin_contract, state::StoredAccount}, crypto};

pub(in crate::server::core::account_store) fn authorize(
    conn: &Connection,
    key: &[u8; 32],
    original: &StoredAccount,
    input: &FreeEnrollmentInput,
    retained: &mut RetainedRows,
) -> Result<(), Error> {
    if conn.is_autocommit() { return Err(Error::Storage); }
    storage::ensure_no_triggers(conn)?;
    identity::verify_input(input)?;
    super::require_unexpired(input)?;
    let id = original.id();
    let contract = plugin_contract::contract_id("cline-free", input.uid(), false);
    let identity_key = contract.strip_prefix("cline-free-account-").ok_or(Error::IdentityConflict)?;
    let row = storage::load_account(conn, key, id, retained)?;
    identity::verify_refresh(row, retained)?;
    if retained.record(row).is_desktop()
        || retained.record(row).get("pluginIdentity").and_then(Value::as_str) != Some(input.uid()) {
        return Err(Error::IdentityConflict);
    }
    for other in storage::account_ids(conn, retained)? {
        if retained.text(other)? == id { continue; }
        let other = storage::load_existing(conn, key, other, retained)?;
        identity::exclude_existing(other, input, retained)?;
    }
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='cline_free_refresh_slots')",
        [], |row| row.get(0),
    ).map_err(|_| Error::Storage)?;
    if exists != 0 {
        slot::validate_schema(conn)?;
        let occupied: i64 = conn.query_row(
            "SELECT COUNT(*) FROM main.cline_free_refresh_slots WHERE account_id=?1 OR identity_key=?2",
            rusqlite::params![id, identity_key], |row| row.get(0),
        ).map_err(|_| Error::Storage)?;
        if occupied > 1 { return Err(Error::IdentityConflict); }
        if occupied == 1 {
            let before = slot::readback(conn, key, id, identity_key, retained)?;
            let value = retained.value(before);
            if value["schema"] != 2 || value["provider"] != "cline-free" || value["uid"] != input.uid()
                || value["accountId"] != id || value["identityKey"] != identity_key
                || !matches!(value["state"]["kind"].as_str(), Some("ready" | "pending")) {
                return Err(Error::InvalidRecord);
            }
            // 原槽不存令牌；只与替换前的真实材料比，不能拿刚保存的新行证明换了凭据。
            let previous = identity::refresh_token(original)?.ok_or(Error::InvalidRecord)?;
            let incoming = input.secret()["refreshToken"].as_str().ok_or(Error::InvalidInput)?;
            if incoming == previous { return Err(Error::ExistingSlot); }
            if value["state"]["kind"] == "pending" {
                use sha2::{Digest, Sha256};
                let digest = value["state"]["refreshDigest"].as_str().ok_or(Error::ExistingSlot)?;
                if digest == format!("{:x}", Sha256::digest(incoming.as_bytes())) {
                    return Err(Error::ExistingSlot);
                }
            }
            if conn.execute(
                "DELETE FROM main.cline_free_refresh_slots WHERE account_id=?1 AND identity_key=?2",
                rusqlite::params![id, identity_key],
            ).map_err(|_| Error::Storage)? != 1 { return Err(Error::Storage); }
        }
    }
    let epoch = match storage::epoch(conn, retained)? {
        Some(epoch) => epoch,
        None => storage::establish_epoch(conn, retained)?,
    };
    let born = retained.retain_value(json!(storage::random_id()?));
    retained.set_record(row, "source", json!("plugin"));
    retained.set_record(row, "account", json!(input.uid()));
    retained.set_record(row, "uid", json!(input.uid()));
    record::establish_version(row, epoch, born, retained)?;
    let bytes = retained.encode_record(row)?;
    let encrypted = crypto::seal_with_key(retained.buffer(bytes), key).map_err(|_| Error::Storage)?;
    if encrypted.len() > storage::MAX_ROW_BYTES { return Err(Error::LimitExceeded); }
    if conn.execute("UPDATE main.accounts SET data=?2 WHERE id=?1 AND provider='cline-free'",
        rusqlite::params![id, encrypted]).map_err(|_| Error::Storage)? != 1 { return Err(Error::Storage); }
    let readback = storage::load_account(conn, key, id, retained)?;
    if retained.record(readback).fields() != retained.record(row).fields() { return Err(Error::Storage); }
    identity::verify_refresh(readback, retained)?;
    slot::insert_initial(conn, key, id, identity_key, readback, input, retained)?;
    super::require_unexpired(input)?;
    Ok(())
}
