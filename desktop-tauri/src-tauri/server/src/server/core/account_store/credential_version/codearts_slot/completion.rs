//! 原CodeArts Pending的封闭保存终检；候选预检在原Call门外，SQL只走固定原来源。
//! 一次消费、SQL返回、真实COMMIT和连接收尾分开记录；任何异常都不恢复Ready资格。

use std::{
    any::Any,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Mutex, Weak},
};

use serde_json::Value;
use zeroize::Zeroizing;

use super::super::{cached_key, AccountStore, Error, StorageFailure};
use super::{
    slots, PendingBinding, PendingCommitFact, PendingFinalError, PendingSlot, SlotFailure,
    SlotSettlement,
};
use crate::server::db::{CodeArtsStorageError, CodeArtsTransaction, CredentialSource};

pub(crate) enum PendingCompletionCallback {
    Returned(Result<SlotSettlement, SlotFailure>),
    Unwound,
}

pub(crate) enum PendingCompletionState {
    NotEntered {
        error: PendingFinalError,
        commit: PendingCommitFact,
        storage_closed: bool,
    },
    Entered {
        result: PendingCompletionCallback,
        commit: PendingCommitFact,
        storage_closed: bool,
    },
}

/// 无Clone/Debug/serde；原Pending不离开本对象，候选和已选缓存key不能另配账号来源。
#[must_use]
pub(crate) struct PendingCompletion {
    owner: PendingSlot,
    // 门前取走这一槽即单向占用本对象，忙/拒绝/未知/异常之后也不放回。
    preparation: Mutex<Option<Result<slots::Replacement, SlotFailure>>>,
    key: Zeroizing<[u8; 32]>,
}

/// 全部原Call/Host/Runtime/账号/Db门退出后才into_state或Drop。
/// 校验底稿与写入底稿分开有界保留，避免额外verify挤满原complete的三条行预算。
#[must_use]
pub(crate) struct PendingCompletionCheck {
    state: PendingCompletionState,
    retained: [slots::RetainedRows; 2],
    preparation: Option<Result<slots::Replacement, SlotFailure>>,
    deferred_unwind: [Option<Box<dyn Any + Send>>; 2],
}

impl PendingCompletionCheck {
    pub(crate) fn facts(&self) -> (PendingCommitFact, bool) {
        match &self.state {
            PendingCompletionState::NotEntered { commit, storage_closed, .. }
                | PendingCompletionState::Entered { commit, storage_closed, .. } => (*commit, *storage_closed),
        }
    }

    pub(crate) fn into_state(self) -> PendingCompletionState {
        let Self {
            state,
            retained,
            preparation,
            deferred_unwind,
        } = self;
        drop(retained);
        drop(preparation);
        drop(deferred_unwind);
        state
    }

    fn not_entered(error: PendingFinalError) -> Self {
        Self {
            state: PendingCompletionState::NotEntered {
                error,
                commit: PendingCommitFact::NotAttempted,
                storage_closed: true,
            },
            retained: [slots::RetainedRows::new(), slots::RetainedRows::new()],
            preparation: None,
            deferred_unwind: [None, None],
        }
    }
}

/// 只能从PendingCompletion借用；不导出原store/Db/key或自由SQL执行入口。
pub(in crate::server::core::account_store) struct PendingCompletionRead<'a> {
    owner: &'a PendingCompletion,
}

impl PendingCompletionRead<'_> {
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
            &self.owner.owner.original.store,
            &self.owner.owner.original.source,
            &self.owner.owner.original.inner,
            &self.owner.owner.write,
            &self.owner.key,
        )
    }
}

impl PendingSlot {
    /// 仅锁外候选预检；坏候选仍由原Pending持有，后续只允许固定Blocked提交。
    /// cached_key失败不恢复原槽、重取账号或构造另一份Pending。
    pub(crate) fn prepare_completion(
        self,
        credentials: &Value,
    ) -> Result<PendingCompletion, Error> {
        let preparation = slots::prepare_replacement(credentials);
        let key = cached_key()?;
        Ok(PendingCompletion {
            owner: self,
            preparation: Mutex::new(Some(preparation)),
            key,
        })
    }
}

impl PendingCompletion {
    pub(crate) fn matches_binding(&self, binding: &PendingBinding) -> bool {
        Weak::ptr_eq(&binding.source, &Arc::downgrade(&self.owner.binding))
    }

    pub(crate) fn account(&self) -> &str {
        self.owner.account()
    }

    /// 仅原Call专用保存门；consume只能同步布尔准入，不得SQL/网络/await/spawn/反向锁。
    /// 终检槽在任何存储动作前一次取走；未进入或提交未确认均不得重试本对象。
    pub(crate) fn try_commit(&self, consume: &mut impl FnMut() -> bool) -> PendingCompletionCheck {
        let preparation = {
            let mut slot = match self.preparation.try_lock() {
                Ok(slot) => slot,
                Err(_) => {
                    return PendingCompletionCheck::not_entered(PendingFinalError::Interrupted)
                }
            };
            slot.take()
        };
        let Some(preparation) = preparation else {
            return PendingCompletionCheck::not_entered(PendingFinalError::Slot(
                SlotFailure::ReceiptChanged,
            ));
        };
        let mut preparation = Some(preparation);
        let read = PendingCompletionRead { owner: self };
        let mut retained = [slots::RetainedRows::new(), slots::RetainedRows::new()];
        let mut commit = None;
        let mut entered = false;
        let mut returned = None;
        let mut sql_error = None;
        let mut action_unwind = None;
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            AccountStore::try_codearts_pending_completion(
                &read,
                &mut preparation,
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
                Some(
                    sql_error
                        .map(PendingFinalError::Slot)
                        .unwrap_or(PendingFinalError::Interrupted),
                ),
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
        // COMMIT事实由Db在execute成功且autocommit时外写；finalize/恢复设置/unwind不抹掉。
        let commit = match commit {
            None => PendingCommitFact::NotAttempted,
            Some(true) => PendingCommitFact::Confirmed,
            Some(false) => PendingCommitFact::Unconfirmed,
        };
        let state = if entered {
            PendingCompletionState::Entered {
                result: returned
                    .map(PendingCompletionCallback::Returned)
                    .unwrap_or(PendingCompletionCallback::Unwound),
                commit,
                storage_closed,
            }
        } else {
            PendingCompletionState::NotEntered {
                error: error.unwrap_or(PendingFinalError::Transaction(
                    CodeArtsStorageError::Unavailable,
                )),
                commit,
                storage_closed,
            }
        };
        PendingCompletionCheck {
            state,
            retained,
            preparation,
            deferred_unwind: [action_unwind, outer_unwind],
        }
    }
}
