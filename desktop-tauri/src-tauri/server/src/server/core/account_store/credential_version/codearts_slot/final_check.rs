//! 原Pending的同步最终核对对象。业务回调外持，原行/投影/异常只随对象在Call门外析构。
//! 只确认同一Pending和一次内存CAS的事实；不是HTTP已发送、响应或保存许可。

use std::{
    any::Any,
    panic::{catch_unwind, AssertUnwindSafe},
};

use zeroize::Zeroizing;

use super::super::{cached_key, AccountStore, Error, StorageFailure};
use super::{slots, PendingSlot, SlotFailure};
use crate::server::db::{CodeArtsStorageError, CodeArtsTransaction, CredentialSource};

pub(crate) enum PendingFinalError {
    AccountBusy,
    AccountPoisoned,
    StoreUnavailable,
    Read(Error),
    Slot(SlotFailure),
    Transaction(CodeArtsStorageError),
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingCommitFact {
    NotAttempted,
    Confirmed,
    Unconfirmed,
}

pub(crate) enum PendingFinalCallback {
    Returned(bool),
    Unwound,
}

pub(crate) enum PendingFinalState {
    NotEntered {
        error: PendingFinalError,
        commit: PendingCommitFact,
        storage_closed: bool,
    },
    Entered {
        result: PendingFinalCallback,
        commit: PendingCommitFact,
        storage_closed: bool,
    },
}

/// 不实现Clone/Debug/serde。必须离开原Call门后才into_state/Drop，不能锁内格式化panic。
#[must_use]
pub(crate) struct PendingFinalCheck {
    state: PendingFinalState,
    retained: slots::RetainedRows,
    deferred_unwind: [Option<Box<dyn Any + Send>>; 2],
}

impl PendingFinalCheck {
    pub(crate) fn into_state(self) -> PendingFinalState {
        let Self {
            state,
            retained,
            deferred_unwind,
        } = self;
        drop(retained);
        drop(deferred_unwind);
        state
    }
}

/// 封闭组合只在PendingSlot内装配；原AccountStore helper仅借原来源和已选缓存key。
/// 无Clone/serde/公开构造，不能拆出替代账号/库或从JSON申请终检。
pub(in crate::server::core::account_store) struct PendingFinalRead<'a> {
    owner: &'a PendingSlot,
    key: Zeroizing<[u8; 32]>,
}

impl PendingFinalRead<'_> {
    pub(in crate::server::core::account_store) fn with_original_store<R>(
        &self,
        read: impl FnOnce(
            &AccountStore,
            &CredentialSource,
            &crate::server::core::account_store::sql::credential_version::Stamp,
            &slots::PendingWrite,
            &[u8; 32],
        ) -> R,
    ) -> R {
        read(
            &self.owner.original.store,
            &self.owner.original.source,
            &self.owner.original.inner,
            &self.owner.write,
            &self.key,
        )
    }
}

impl PendingSlot {
    /// 借原Pending终检，不移出原stamp/操作、不重取Ready/账号/槽、不恢复发送机会。
    /// consume仅原Call已持内存门的一次布尔CAS；不得HTTP/await/spawn/反向锁或记录凭据。
    /// 原Call必须先单向消费终检机会；本入口自身不枚举/缓存其它Call，也不提供重试。
    pub(crate) fn try_final_check(&self, consume: &mut impl FnMut() -> bool) -> PendingFinalCheck {
        let mut retained = slots::RetainedRows::new();
        let key = match cached_key() {
            Ok(key) => key,
            Err(error) => {
                return PendingFinalCheck {
                    state: PendingFinalState::NotEntered {
                        error: PendingFinalError::Read(error),
                        commit: PendingCommitFact::NotAttempted,
                        storage_closed: true,
                    },
                    retained,
                    deferred_unwind: [None, None],
                }
            }
        };
        let read = PendingFinalRead { owner: self, key };
        let mut commit = None;
        let mut entered = false;
        let mut returned = None;
        let mut sql_error = None;
        let mut action_unwind = None;
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            AccountStore::try_codearts_pending_final(
                &read,
                &mut commit,
                &mut entered,
                &mut returned,
                &mut sql_error,
                &mut retained,
                consume,
                &mut action_unwind,
            )
        }));
        let mut outer_unwind = None;
        let (error, storage_closed) = match attempt {
            Ok(Err(error)) => (Some(error), true),
            Ok(Ok(CodeArtsTransaction::NotEntered {
                error,
                storage_closed,
            })) => (Some(PendingFinalError::Transaction(error)), storage_closed),
            Ok(Ok(CodeArtsTransaction::Rejected { storage_closed, .. })) => (
                Some(match sql_error {
                    Some(error) => PendingFinalError::Slot(error),
                    None => PendingFinalError::Interrupted,
                }),
                storage_closed,
            ),
            Ok(Ok(CodeArtsTransaction::CommitUnconfirmed { storage_closed })) => (
                Some(PendingFinalError::Read(Error::Storage(
                    StorageFailure::CommitUnconfirmed,
                ))),
                storage_closed,
            ),
            Ok(Ok(CodeArtsTransaction::Committed { storage_closed, .. })) => (None, storage_closed),
            Err(payload) => {
                outer_unwind = Some(payload);
                (Some(PendingFinalError::Interrupted), false)
            }
        };
        let commit = match commit {
            None => PendingCommitFact::NotAttempted,
            Some(true) => PendingCommitFact::Confirmed,
            Some(false) => PendingCommitFact::Unconfirmed,
        };
        let state = if entered {
            PendingFinalState::Entered {
                result: match returned {
                    Some(value) => PendingFinalCallback::Returned(value),
                    None => PendingFinalCallback::Unwound,
                },
                commit,
                storage_closed,
            }
        } else {
            PendingFinalState::NotEntered {
                error: error.unwrap_or(PendingFinalError::Interrupted),
                commit,
                storage_closed,
            }
        };
        PendingFinalCheck {
            state,
            retained,
            deferred_unwind: [action_unwind, outer_unwind],
        }
    }
}
