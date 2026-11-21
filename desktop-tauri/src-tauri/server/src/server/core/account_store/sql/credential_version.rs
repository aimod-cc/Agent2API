//! 固定只读行来源的出生版本；普通CRUD只维护/失效版本，不从JSON签发来源。
//! MonkeyCode/豆包只有读取来源，外层另绑定Db进程来源。
//! 不使用任务层只读ROLLBACK入口。COMMIT错误独立报告，不能推断已/未保存。

mod bounded;
pub(in crate::server::core::account_store) mod doubao_state;
pub(super) mod ordinary;
mod row_projection;
pub(in crate::server::core::account_store) mod row_egress;
mod row_source;
mod storage;

pub(in crate::server::core::account_store) use row_source::{prepare as prepare_row, RowStamp};

use super::super::credential_version::RowProvider;

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

#[derive(PartialEq, Eq)]
pub(super) struct Version {
    pub(super) epoch: String,
    pub(super) born: String,
    pub(super) revision: u64,
}

pub(in crate::server::core::account_store) fn maintained_provider(provider: &str) -> bool {
    RowProvider::parse(provider).is_some()
}

fn check_id(id: &str) -> Result<(), Error> {
    if id.is_empty() || id.len() > 512 || id.trim() != id || id.chars().any(char::is_control) {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

fn finish<T>(tx: rusqlite::Transaction<'_>, result: Result<T, Error>) -> Result<T, Error> {
    match result {
        Ok(value) => {
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
