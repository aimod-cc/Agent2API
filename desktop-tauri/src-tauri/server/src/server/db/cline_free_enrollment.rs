//! Free首次登记的原连接包装。只内部复用FULL底座，不复用CodeArts业务来源或槽。
//! 不增加连接/锁/代次，不接路径、JSON、账号ID或ABI构造的来源。

use std::sync::atomic::Ordering;

use super::codearts_slot::{full_transaction_with_fact, CodeArtsStorageError, CodeArtsTransaction};
use super::{CredentialSource, Db};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FreeEnrollmentStorageError {
    Unavailable,
    SourceChanged,
    NestedTransaction,
    JournalMode,
    Synchronous,
    Busy,
    Begin,
}

/// Db内部分阶段结果；业务层只交付确定未提交/未知/真实提交三态。
#[must_use]
pub(crate) enum FreeEnrollmentTransaction<T, E> {
    NotEntered {
        error: FreeEnrollmentStorageError,
        storage_closed: bool,
    },
    Rejected {
        error: E,
        storage_closed: bool,
    },
    CommitUnconfirmed {
        storage_closed: bool,
    },
    Committed {
        value: T,
        storage_closed: bool,
    },
}

impl Db {
    /// 调用者已持原AccountStore锁；action仅固定Free同库存储，不出网/await/重取锁。
    /// 本方法不捕获新来源，也不在来源失效后改用新epoch继续提交。
    pub(crate) fn cline_free_enrollment_transaction<T, E>(
        &self,
        source: &CredentialSource,
        action: impl FnOnce(&rusqlite::Connection) -> Result<T, E>,
    ) -> FreeEnrollmentTransaction<T, E> {
        let mut commit = None;
        self.cline_free_enrollment_transaction_with_fact(source, &mut commit, action)
    }

    pub(crate) fn cline_free_enrollment_transaction_with_fact<T, E>(
        &self,
        source: &CredentialSource,
        commit: &mut Option<bool>,
        action: impl FnOnce(&rusqlite::Connection) -> Result<T, E>,
    ) -> FreeEnrollmentTransaction<T, E> {
        if commit.is_some() { return unavailable(); }
        let state = match self.inner.conn.lock() {
            Ok(state) => state,
            Err(_) => return unavailable(),
        };
        if self.inner.task_guard_unusable.load(Ordering::Acquire) {
            return unavailable();
        }
        if !source.matches(self, &state) {
            return FreeEnrollmentTransaction::NotEntered {
                error: FreeEnrollmentStorageError::SourceChanged,
                storage_closed: state.is_autocommit(),
            };
        }
        // CodeArts类型只留在Db底层映射中；Free外层不取得其Stamp、槽或发送资格。
        match full_transaction_with_fact(&state, &self.inner.task_guard_unusable, commit, action) {
            CodeArtsTransaction::NotEntered {
                error,
                storage_closed,
            } => FreeEnrollmentTransaction::NotEntered {
                error: storage_error(error),
                storage_closed,
            },
            CodeArtsTransaction::Rejected {
                error,
                storage_closed,
            } => FreeEnrollmentTransaction::Rejected {
                error,
                storage_closed,
            },
            CodeArtsTransaction::CommitUnconfirmed { storage_closed } => {
                FreeEnrollmentTransaction::CommitUnconfirmed { storage_closed }
            }
            CodeArtsTransaction::Committed {
                value,
                storage_closed,
            } => FreeEnrollmentTransaction::Committed {
                value,
                storage_closed,
            },
        }
    }
}

fn unavailable<T, E>() -> FreeEnrollmentTransaction<T, E> {
    FreeEnrollmentTransaction::NotEntered {
        error: FreeEnrollmentStorageError::Unavailable,
        storage_closed: false,
    }
}

fn storage_error(error: CodeArtsStorageError) -> FreeEnrollmentStorageError {
    match error {
        CodeArtsStorageError::Unavailable => FreeEnrollmentStorageError::Unavailable,
        CodeArtsStorageError::SourceChanged => FreeEnrollmentStorageError::SourceChanged,
        CodeArtsStorageError::NestedTransaction => FreeEnrollmentStorageError::NestedTransaction,
        CodeArtsStorageError::JournalMode => FreeEnrollmentStorageError::JournalMode,
        CodeArtsStorageError::Synchronous => FreeEnrollmentStorageError::Synchronous,
        CodeArtsStorageError::Busy => FreeEnrollmentStorageError::Busy,
        CodeArtsStorageError::Begin => FreeEnrollmentStorageError::Begin,
    }
}
