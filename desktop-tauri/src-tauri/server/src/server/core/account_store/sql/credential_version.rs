//! 固定三家的单行出生版本。普通CRUD只维护/失效版本，不从JSON签发来源。
//! CodeArts独占凭据提交；MonkeyCode/豆包只有读取来源，外层另绑定Db进程来源。
//! 不使用任务层只读ROLLBACK入口。COMMIT错误独立报告，不能推断已/未保存。

mod bounded;
pub(in crate::server::core::account_store) mod codearts_slot;
pub(in crate::server::core::account_store) mod codearts_enrollment;
pub(in crate::server::core::account_store) mod cline_free_enrollment;
pub(in crate::server::core::account_store) mod doubao_state;
pub(super) mod ordinary;
mod projection;
mod row_projection;
pub(in crate::server::core::account_store) mod row_egress;
mod row_source;
mod storage;

pub(in crate::server::core::account_store) use row_source::{prepare as prepare_row, RowStamp};

use rusqlite::{Connection, TransactionBehavior};
use serde_json::Value;

use super::super::{credential_version::RowProvider, state::StoredAccount};
use projection::Projection;

pub(in crate::server::core::account_store) use projection::Change;

#[derive(Debug)]
pub(crate) enum Error {
    InvalidInput,
    InvalidRecord,
    LimitExceeded,
    UnsupportedProvider,
    VersionExhausted,
    Conflict(Conflict),
    Storage(StorageFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Conflict {
    MissingAccount,
    ProviderChanged,
    DatabaseEpochChanged,
    HostSourceChanged,
    VersionChanged,
    AuthorizationChanged,
    UnestablishedVersion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StorageFailure {
    Unavailable,
    Busy,
    Begin,
    Read,
    Write,
    KeyUnavailable,
    Encryption,
    RandomUnavailable,
    CommitUnconfirmed,
    RollbackUnconfirmed,
}

/// 凭据改动已提交的事实；不携带token、账号私有标识或可再次使用的读凭证。
#[must_use]
pub(crate) struct CommitReceipt {
    revision_advanced: bool,
}

impl CommitReceipt {
    pub(crate) fn revision_advanced(&self) -> bool {
        self.revision_advanced
    }
}

/// 字段私有且不实现Clone/Debug/serde；只能由prepare的真实事务成功后构造。
pub(in crate::server::core::account_store) struct Stamp {
    id: String,
    version: Version,
    projection: Projection,
}

impl Stamp {
    pub(in crate::server::core::account_store) fn account(&self) -> &str {
        &self.id
    }

    pub(in crate::server::core::account_store) fn secret(&self) -> &Value {
        self.projection.secret()
    }
}

#[derive(PartialEq, Eq)]
struct Version {
    epoch: String,
    born: String,
    revision: u64,
}

pub(in crate::server::core::account_store) fn maintained_provider(provider: &str) -> bool {
    provider == projection::PROVIDER || provider == "cline-free" || RowProvider::parse(provider).is_some()
}

pub(in crate::server::core::account_store) fn check_target(
    provider: &str,
    id: &str,
) -> Result<(), Error> {
    if provider != projection::PROVIDER {
        return Err(Error::UnsupportedProvider);
    }
    check_id(id)
}

fn check_id(id: &str) -> Result<(), Error> {
    if id.is_empty() || id.len() > 512 || id.trim() != id || id.chars().any(char::is_control) {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

pub(in crate::server::core::account_store) fn prepare(
    conn: &mut Connection,
    key: &[u8; 32],
    provider: &str,
    id: &str,
) -> Result<Stamp, Error> {
    check_target(provider, id)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage::sql_error(error, StorageFailure::Begin))?;
    let result = (|| {
        let mut record = storage::load(&tx, key, id)?;
        let projection = Projection::read(&record)?;
        let existing = storage::version(&record)?;
        let epoch = storage::epoch(&tx)?;
        let (version, establish) = match (existing, epoch) {
            (Some(version), Some(epoch)) if version.epoch == epoch => (version, false),
            (previous, epoch) => {
                // 只处理真实读出的缺失/格式有效但不同代次；上面的解码/版本/kv错误均不吞。
                // 恢复后按需重建当前目标行，不解密重写全库，也不复用备份中的born。
                let epoch = match epoch {
                    Some(epoch) => epoch,
                    None => storage::establish_epoch(&tx)?,
                };
                let born = storage::random_id()?;
                if previous
                    .as_ref()
                    .is_some_and(|version| version.born == born)
                {
                    return Err(Error::Storage(StorageFailure::RandomUnavailable));
                }
                (
                    Version {
                        epoch,
                        born,
                        revision: 1,
                    },
                    true,
                )
            }
        };
        if establish {
            storage::set_version(&mut record, &version);
            storage::update_existing(&tx, key, &record)?;
            verify_written(&tx, key, id, &record)?;
        }
        Ok(Stamp {
            id: id.to_string(),
            version,
            projection,
        })
    })();
    finish(tx, result)
}

pub(in crate::server::core::account_store) fn commit(
    conn: &mut Connection,
    key: &[u8; 32],
    stamp: &Stamp,
    change: Change,
    reauthorize: bool,
) -> Result<CommitReceipt, Error> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage::sql_error(error, StorageFailure::Begin))?;
    let result = (|| {
        let epoch = storage::epoch(&tx)?.ok_or(Error::Conflict(Conflict::DatabaseEpochChanged))?;
        if epoch != stamp.version.epoch {
            return Err(Error::Conflict(Conflict::DatabaseEpochChanged));
        }
        let mut record = storage::load(&tx, key, &stamp.id)?;
        let current =
            storage::version(&record)?.ok_or(Error::Conflict(Conflict::UnestablishedVersion))?;
        if current != stamp.version {
            return Err(Error::Conflict(Conflict::VersionChanged));
        }
        let authorization = Projection::read(&record)?;
        if authorization != stamp.projection {
            return Err(Error::Conflict(Conflict::AuthorizationChanged));
        }
        // 以事务内最新原行作为底稿；调用者无法把旧备注、启停、优先级或代理传进来。
        change.apply(&mut record)?;
        let changed = Projection::read(&record)? != authorization;
        let advanced = changed || reauthorize;
        let revision = if advanced {
            current
                .revision
                .checked_add(1)
                .ok_or(Error::VersionExhausted)?
        } else {
            current.revision
        };
        storage::set_version(
            &mut record,
            &Version {
                revision,
                ..current
            },
        );
        record.set_updated_at(crate::server::logging::now_ms());
        storage::update_existing(&tx, key, &record)?;
        verify_written(&tx, key, &stamp.id, &record)?;
        Ok(CommitReceipt {
            revision_advanced: advanced,
        })
    })();
    finish(tx, result)
}

fn verify_written(
    conn: &Connection,
    key: &[u8; 32],
    id: &str,
    expected: &StoredAccount,
) -> Result<(), Error> {
    let read = storage::load(conn, key, id)?;
    if read.fields() != expected.fields() {
        return Err(Error::Storage(StorageFailure::Write));
    }
    Ok(())
}

fn finish<T>(tx: rusqlite::Transaction<'_>, result: Result<T, Error>) -> Result<T, Error> {
    match result {
        Ok(value) => {
            // 只有此处COMMIT返回Ok才放出快照或成功回执；上层停止等待无法替代这个事实。
            tx.commit()
                .map_err(|_| Error::Storage(StorageFailure::CommitUnconfirmed))?;
            Ok(value)
        }
        Err(error) => {
            tx.rollback()
                .map_err(|_| Error::Storage(StorageFailure::RollbackUnconfirmed))?;
            Err(error)
        }
    }
}
