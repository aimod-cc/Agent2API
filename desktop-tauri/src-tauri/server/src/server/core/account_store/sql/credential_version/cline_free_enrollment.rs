//! 仅真正新Free账号的固定首次登记事务；存量/同身份槽均拒绝，不迁移旧资格。
//! 只查询本家有界行。完整材料、私有版本与独立首次Ready在原FULL事务内写入回读。

pub(in crate::server::core::account_store) mod refresh;
pub(in crate::server::core::account_store) mod reauthorize;
mod identity;
mod record;
mod slot;
mod storage;

use rusqlite::Connection;

use crate::server::core::account_store::{
    credential_version::{FreeEnrollmentError, FreeEnrollmentInput},
    plugin_contract,
    state::StoredAccount,
};

pub(in crate::server::core::account_store) use storage::RetainedRows;

const PROVIDER: &str = "cline-free";
const MAX_ACCOUNTS: usize = 128;
const MAX_RETAINED_BYTES: i64 = 8 * 1024 * 1024;
const PRIVATE_FIELD: &str = "_hostCredentialVersion";
const EPOCH_KEY: &str = "accountCredentialEpoch";

pub(in crate::server::core::account_store) fn commit(
    conn: &Connection,
    key: &[u8; 32],
    input: &FreeEnrollmentInput,
    retained: &mut RetainedRows,
) -> Result<String, FreeEnrollmentError> {
    if conn.is_autocommit() {
        return Err(FreeEnrollmentError::Storage);
    }
    storage::ensure_no_triggers(conn)?;
    identity::verify_input(input)?;
    require_unexpired(input)?;
    // 固定新ID与UID锚使用原全局合同哈希；不取插件注册表、原账号ID或任意provider输入。
    let id = plugin_contract::contract_id(PROVIDER, input.identity(), false);
    let identity_key = id
        .strip_prefix("cline-free-account-")
        .ok_or(FreeEnrollmentError::IdentityConflict)?;
    // 在任何版本或凭据写入之前先查双锚。已有槽不解密，不把旧Ready当第二次许可。
    slot::require_absent(conn, &id, identity_key)?;
    if storage::occupied(conn, &id)? {
        return Err(FreeEnrollmentError::ExistingAccount);
    }
    let ids = storage::account_ids(conn, retained)?;
    for existing in ids {
        let index = storage::load_existing(conn, key, existing, retained)?;
        identity::exclude_existing(index, input, retained)?;
    }
    let epoch = match storage::epoch(conn, retained)? {
        Some(epoch) => epoch,
        None => storage::establish_epoch(conn, retained)?,
    };
    let born = retained.retain_value(serde_json::Value::String(storage::random_id()?));
    let now = require_unexpired(input)?;
    let next = record::create(conn, &id, input, now, retained)?;
    record::establish_version(next, epoch, born, retained)?;
    record::verify(next, input, retained)?;
    record::insert(conn, key, next, retained)?;
    let readback = storage::load_account(conn, key, &id, retained)?;
    if retained.record(readback).fields() != retained.record(next).fields() {
        return Err(FreeEnrollmentError::Storage);
    }
    record::verify(readback, input, retained)?;
    // 完整最终行与version均已同事务回读；首次槽只INSERT、回读，不接Pending消费者。
    slot::insert_initial(conn, key, &id, identity_key, readback, input, retained)?;
    require_unexpired(input)?;
    // 候选id不含材料；只有外层真实COMMIT成功后才包装成功回执。
    Ok(id)
}

fn require_unexpired(input: &FreeEnrollmentInput) -> Result<i64, FreeEnrollmentError> {
    let now = now_ms()?;
    if input.expires_at_ms().is_some_and(|expiry| expiry <= now) {
        return Err(FreeEnrollmentError::Expired);
    }
    Ok(now)
}

fn now_ms() -> Result<i64, FreeEnrollmentError> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| FreeEnrollmentError::ClockUnavailable)?;
    i64::try_from(elapsed.as_millis()).map_err(|_| FreeEnrollmentError::ClockUnavailable)
}
