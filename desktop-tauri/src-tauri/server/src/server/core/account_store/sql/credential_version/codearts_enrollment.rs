//! CodeArts首次登记只在原Db FULL/IMMEDIATE事务内完成；不从缺槽、账号ID或插件报告批准。
//! 原Input始终由外层持有；明文行及替换下来的字段保留到所有存储锁退出后清理。

mod identity;
mod record;

use rusqlite::{types::ValueRef, Connection};
use serde_json::{Map, Value};
use zeroize::Zeroize;

use super::{bounded, codearts_slot, storage, Conflict, Error, StorageFailure, Version};
use crate::server::core::account_store::{
    credential_version::{EnrollmentInput, EnrollmentStorageError as EnrollmentError},
    plugin_contract,
    state::StoredAccount,
};

const PROVIDER: &str = "codearts";
const MAX_ACCOUNTS: usize = 128;
const MAX_RETAINED_BYTES: i64 = 8 * 1024 * 1024;

/// 只能在外层创建，SQL借用它持有已读/已建明文，不在账号/Db锁内销毁用户业务内容。
pub(in crate::server::core::account_store) struct RetainedRows {
    records: Vec<StoredAccount>,
    discarded: Vec<Value>,
}

impl RetainedRows {
    pub(in crate::server::core::account_store) fn new() -> Self {
        Self {
            records: Vec::new(),
            discarded: Vec::new(),
        }
    }
    fn retain(&mut self, record: StoredAccount) -> usize {
        let index = self.records.len();
        self.records.push(record);
        index
    }
    fn get(&self, index: usize) -> &StoredAccount {
        &self.records[index]
    }
    fn parts(&mut self, index: usize) -> (&mut StoredAccount, &mut Vec<Value>) {
        (&mut self.records[index], &mut self.discarded)
    }
}

impl Drop for RetainedRows {
    fn drop(&mut self) {
        for row in &mut self.records {
            wipe_fields(row.fields_mut());
        }
        for value in &mut self.discarded {
            wipe_value(value);
        }
    }
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

pub(in crate::server::core::account_store) fn commit(
    conn: &Connection,
    key: &[u8; 32],
    input: &EnrollmentInput,
    retained: &mut RetainedRows,
    new_login: bool,
    proxy: Option<&Value>,
) -> Result<String, EnrollmentError> {
    if conn.is_autocommit() {
        return Err(EnrollmentError::Storage);
    }
    storage::ensure_no_triggers(conn).map_err(error)?;
    if input.expires_at_ms() <= now_ms()? {
        return Err(EnrollmentError::Expired);
    }
    let ids = account_ids(conn)?;
    let mut target = None;
    for id in ids {
        let loaded = retained.retain(storage::load(conn, key, &id).map_err(error)?);
        if new_login {
            let (row, discarded) = retained.parts(loaded);
            for field in ["needsRelogin", "needsReloginReason"] {
                if let Some(value) = row.fields_mut().remove(field) { discarded.push(value); }
                for container in ["pluginSecret", "pluginPublic"] {
                    if let Some(fields) = row.fields_mut().get_mut(container).and_then(Value::as_object_mut) {
                        if let Some(value) = fields.remove(field) { discarded.push(value); }
                    }
                }
            }
        }
        let same = identity::matches(retained.get(loaded), input)?;
        if same {
            if target.replace(loaded).is_some() {
                return Err(EnrollmentError::AmbiguousIdentity);
            }
        }
    }
    let id = match target {
        Some(index) => retained.get(index).id().to_string(),
        None => plugin_contract::contract_id(PROVIDER, input.identity(), false),
    };
    // 检查在任何版本重建和凭据写入之前。坏、Pending、Blocked、旧epoch、Ready均不覆盖。
    if new_login {
        codearts_slot::remove_for_new_login(conn, &id).map_err(slot_error)?;
    } else {
        codearts_slot::require_initial_absent(conn, &id).map_err(slot_error)?;
    }
    let current_epoch = storage::epoch(conn).map_err(error)?;
    let previous_version = match target {
        Some(index) => {
            identity::verify_original(retained.get(index), input)?;
            let previous = storage::version(retained.get(index)).map_err(error)?;
            if previous
                .as_ref()
                .is_some_and(|version| !new_login && current_epoch.as_deref() != Some(version.epoch.as_str()))
            {
                return Err(EnrollmentError::RecoveryIsolated);
            }
            previous
        }
        None => {
            // 主键占用只返回布尔，不读取、解密或枚举任何其它provider的记录。
            if occupied(conn, &id)? {
                return Err(EnrollmentError::IdentityConflict);
            }
            None
        }
    };
    let epoch = match current_epoch {
        Some(epoch) => epoch,
        None => storage::establish_epoch(conn).map_err(error)?,
    };
    let version = match previous_version.filter(|_| !new_login) {
        Some(previous) => Version {
            revision: previous
                .revision
                .checked_add(1)
                .ok_or(EnrollmentError::VersionExhausted)?,
            ..previous
        },
        None => Version {
            epoch,
            born: storage::random_id().map_err(error)?,
            revision: 1,
        },
    };
    let now = now_ms()?;
    if input.expires_at_ms() <= now {
        return Err(EnrollmentError::Expired);
    }
    let next_record = match target {
        Some(index) => retained.get(index).clone(),
        None => record::new_account(conn, &id, input, now)?,
    };
    let next = retained.retain(next_record);
    {
        let (record, discarded) = retained.parts(next);
        record::apply(record, input, discarded, now)?;
        if new_login {
            if let Some(proxy) = proxy { record.set_proxy(proxy.clone()); }
        }
        record::set(
            record,
            crate::server::core::account_store::credential_version::PRIVATE_FIELD,
            serde_json::json!({ "schema":1, "dbEpoch":version.epoch, "born":version.born, "revision":version.revision }),
            discarded,
        );
        record::write(conn, key, record, target.is_some())?;
    }
    let readback = retained.retain(storage::load(conn, key, &id).map_err(error)?);
    if retained.get(readback).fields() != retained.get(next).fields()
        || storage::version(retained.get(readback))
            .map_err(error)?
            .as_ref()
            != Some(&version)
    {
        return Err(EnrollmentError::Storage);
    }
    record::verify_input(retained.get(readback), input)?;
    codearts_slot::establish_initial(conn, key, &id, &version).map_err(slot_error)?;
    // 原事务只返回无凭据的候选id；外层真实COMMIT成功后才包装提交回执。
    Ok(id)
}

fn account_ids(conn: &Connection) -> Result<Vec<String>, EnrollmentError> {
    let mut statement = conn
        .prepare(
            "SELECT CASE WHEN typeof(id)='text' AND octet_length(id)<=512 THEN id END,
                CASE WHEN typeof(data)='text' THEN octet_length(data) END
         FROM main.accounts WHERE provider='codearts' ORDER BY rowid LIMIT ?1",
        )
        .map_err(|_| EnrollmentError::Storage)?;
    let mut rows = statement
        .query([MAX_ACCOUNTS as i64 + 1])
        .map_err(|_| EnrollmentError::Storage)?;
    let mut out = Vec::new();
    let mut bytes = 0i64;
    while let Some(row) = rows.next().map_err(|_| EnrollmentError::Storage)? {
        if out.len() == MAX_ACCOUNTS {
            return Err(EnrollmentError::LimitExceeded);
        }
        let id = match row.get_ref(0).map_err(|_| EnrollmentError::Storage)? {
            ValueRef::Text(bytes) => {
                std::str::from_utf8(bytes).map_err(|_| EnrollmentError::InvalidRecord)?
            }
            _ => return Err(EnrollmentError::InvalidRecord),
        };
        super::check_id(id).map_err(error)?;
        let size = match row.get_ref(1).map_err(|_| EnrollmentError::Storage)? {
            ValueRef::Integer(size) if size > 0 && size <= bounded::MAX_ROW_BYTES as i64 => size,
            _ => return Err(EnrollmentError::InvalidRecord),
        };
        bytes = bytes
            .checked_add(size)
            .ok_or(EnrollmentError::LimitExceeded)?;
        if bytes > MAX_RETAINED_BYTES {
            return Err(EnrollmentError::LimitExceeded);
        }
        out.push(id.to_string());
    }
    drop(rows);
    statement.finalize().map_err(|_| EnrollmentError::Storage)?;
    Ok(out)
}

fn occupied(conn: &Connection, id: &str) -> Result<bool, EnrollmentError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.accounts WHERE id=?1)",
        [id],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|_| EnrollmentError::Storage)
}

fn now_ms() -> Result<i64, EnrollmentError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| EnrollmentError::Storage)?;
    i64::try_from(duration.as_millis()).map_err(|_| EnrollmentError::Storage)
}

pub(super) fn error(error: Error) -> EnrollmentError {
    match error {
        Error::LimitExceeded => EnrollmentError::LimitExceeded,
        Error::VersionExhausted => EnrollmentError::VersionExhausted,
        Error::Conflict(Conflict::DatabaseEpochChanged) => EnrollmentError::RecoveryIsolated,
        Error::Storage(StorageFailure::RandomUnavailable) => EnrollmentError::RandomUnavailable,
        Error::Storage(_) => EnrollmentError::Storage,
        Error::Conflict(_)
        | Error::InvalidInput
        | Error::InvalidRecord
        | Error::UnsupportedProvider => EnrollmentError::InvalidRecord,
    }
}
fn slot_error(error: codearts_slot::SlotFailure) -> EnrollmentError {
    match error {
        codearts_slot::SlotFailure::ReceiptChanged => EnrollmentError::ExistingSlot,
        codearts_slot::SlotFailure::Account(error) => self::error(error),
        _ => EnrollmentError::InvalidRecord,
    }
}
