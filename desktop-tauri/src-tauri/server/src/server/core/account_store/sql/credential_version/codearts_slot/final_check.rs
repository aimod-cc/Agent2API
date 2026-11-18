//! 原Pending最终发送前的固定同事务复核。只读一次epoch/账号/槽，不占新槽或补版本。
//! 调用者持原FULL/IMMEDIATE连接；纯内存CAS只能在全部核对成功后由外层执行一次。

use rusqlite::Connection;

use super::super::{storage, Conflict, Error, Stamp, StorageFailure};
use super::{eligible, record, require_binding, PendingWrite, RetainedRows, SlotFailure, State};

pub(in crate::server::core::account_store) fn verify_pending_current(
    conn: &Connection,
    key: &[u8; 32],
    stamp: &Stamp,
    pending: &PendingWrite,
    retained: &mut RetainedRows,
) -> Result<(), SlotFailure> {
    if conn.is_autocommit() {
        return Err(Error::Storage(StorageFailure::Begin).into());
    }
    let epoch = storage::epoch(conn)?.ok_or(Error::Conflict(Conflict::DatabaseEpochChanged))?;
    if epoch != stamp.version.epoch {
        return Err(Error::Conflict(Conflict::DatabaseEpochChanged).into());
    }
    let account = record::load_account(conn, key, &stamp.id, retained)?;
    let version = storage::version(retained.account(account))?
        .ok_or(Error::Conflict(Conflict::UnestablishedVersion))?;
    if version != stamp.version {
        return Err(Error::Conflict(Conflict::VersionChanged).into());
    }
    eligible(retained.account(account))?;
    let projection = retained.keep_projection(account)?;
    if retained.projection(projection) != &stamp.projection {
        return Err(Error::Conflict(Conflict::AuthorizationChanged).into());
    }
    let slot = record::load(conn, key, &stamp.id)?;
    require_binding(&slot, stamp)?;
    if slot.generation != pending.generation
        || !matches!(&slot.state, State::Pending { operation } if operation == &pending.operation)
    {
        return Err(SlotFailure::ReceiptChanged);
    }
    Ok(())
}
