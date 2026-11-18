//! CodeArts严格既有Ready与原Pending存储来源；无HTTP/ABI/插件或自动登记入口。
//! 普通ReadStamp不能构造ReadyRead：只读既有加密行、既有版本和同事务实际Ready槽。
//! 来源/候选与异常载荷只在所有存储锁退出后交付或释放；耐久提交不是exactly-once。

mod final_check;
mod completion;
pub(crate) use final_check::{PendingCommitFact, PendingFinalCallback, PendingFinalCheck, PendingFinalError, PendingFinalState};
pub(crate) use completion::{PendingCompletion, PendingCompletionCallback, PendingCompletionCheck, PendingCompletionState};
pub(in crate::server::core::account_store) use final_check::PendingFinalRead;
pub(in crate::server::core::account_store) use completion::PendingCompletionRead;

use std::{
    any::Any,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Weak},
};

use rusqlite::Connection;
use serde_json::Value;

use super::{cached_key, source_error, sql, AccountStore, Error, ReadStamp, StorageFailure};
use crate::server::db::{CodeArtsStorageError, CodeArtsTransaction, CredentialSource};
use sql::credential_version::codearts_slot::{self as slots, PendingWrite, ReadyGeneration};

pub(crate) use slots::{Settlement as SlotSettlement, SlotFailure};

/// 只有Committed(T)交付后续材料；已提交但收尾失败和未知提交不允许发送或补回Ready。
#[must_use]
pub(crate) enum SlotOutcome<T> {
    NotEntered {
        error: CodeArtsStorageError,
        storage_closed: bool,
    },
    Rejected {
        error: SlotFailure,
        storage_closed: bool,
    },
    CommitUnconfirmed {
        storage_closed: bool,
    },
    CommittedButUnusable,
    Committed(T),
}

/// 无Clone/Debug/serde；只由本次真实只读FULL/IMMEDIATE提交确认后构造。
/// 持原ReadStamp与原Ready随机代次，不接受普通读凭证、账号字符串或缺槽补资格。
#[must_use]
pub(crate) struct ReadyRead {
    original: ReadStamp,
    generation: ReadyGeneration,
    proxy: Value,
}

/// 无Clone/Debug/serde；按值持原读来源和实际Pending提交的唯一操作材料。
/// 本类型本身没有HTTP能力，也不能证明后续原调用仍有效或已发/未发。
#[must_use]
pub(crate) struct PendingSlot {
    original: ReadStamp,
    write: PendingWrite,
    binding: Arc<()>,
}

/// 原Pending的弱对象见证；不以账号、操作字符串或持久化JSON重建同源资格。
/// 无Clone/Debug/serde/公开构造器，移动Pending或转入Completion不改变原对象关系。
#[must_use]
pub(crate) struct PendingBinding {
    source: Weak<()>,
}

impl AccountStore {
    /// 固定本家，id仅是宿主选定目标；不读取其它账号、不建表/版本/epoch/槽、不重试。
    /// 若放入spawn_blocking，所有者必须等真实工作结束，等待超时不能恢复资格。
    pub(crate) fn read_codearts_ready(&self, id: &str) -> Result<SlotOutcome<ReadyRead>, Error> {
        sql::credential_version::check_target("codearts", id)?;
        let key = cached_key()?;
        let db = self
            .database()
            .ok_or(Error::Storage(StorageFailure::Unavailable))?;
        // 只捕获原Db私有进程来源；不读账号/kv。后续原FULL事务在同连接锁内再次复核。
        let source = db
            .capture_codearts_enrollment_source()
            .map_err(source_error)?;
        let outcome = original_transaction(self, &source, |conn, retained| {
            slots::read_ready(conn, &key, id, retained)
        });
        Ok(match outcome {
            CodeArtsTransaction::Committed {
                value,
                storage_closed: true,
            } => SlotOutcome::Committed(ReadyRead {
                original: ReadStamp {
                    store: self.clone(),
                    inner: value.stamp,
                    source,
                },
                generation: value.generation,
                proxy: value.proxy,
            }),
            CodeArtsTransaction::Committed {
                storage_closed: false,
                ..
            } => SlotOutcome::CommittedButUnusable,
            CodeArtsTransaction::NotEntered {
                error,
                storage_closed,
            } => SlotOutcome::NotEntered {
                error,
                storage_closed,
            },
            CodeArtsTransaction::Rejected {
                error,
                storage_closed,
            } => SlotOutcome::Rejected {
                error,
                storage_closed,
            },
            CodeArtsTransaction::CommitUnconfirmed { storage_closed } => {
                SlotOutcome::CommitUnconfirmed { storage_closed }
            }
        })
    }
}

impl ReadyRead {
    /// 仅借原读对象给同宿主纯协议校验；借用不是发送许可，不能拆出另一份ReadStamp。
    pub(crate) fn borrow_stamp(&self) -> &ReadStamp {
        &self.original
    }

    pub(crate) fn original_proxy(&self) -> &Value {
        &self.proxy
    }

    /// 原Ready代次、行版本、epoch与完整授权全部同原事务复核后一次占用Pending。
    /// 方法按值消费，不因拒绝/未知/收尾失败返回旧ReadyRead或换来源再试。
    pub(crate) fn consume_pending(self) -> Result<SlotOutcome<PendingSlot>, Error> {
        let key = cached_key()?;
        let outcome = original_transaction(
            &self.original.store,
            &self.original.source,
            |conn, retained| {
                slots::begin(conn, &key, &self.original.inner, &self.generation, retained)
            },
        );
        Ok(match outcome {
            CodeArtsTransaction::Committed {
                value,
                storage_closed: true,
            } => SlotOutcome::Committed(PendingSlot {
                original: self.original,
                write: value,
                binding: Arc::new(()),
            }),
            CodeArtsTransaction::Committed {
                storage_closed: false,
                ..
            } => SlotOutcome::CommittedButUnusable,
            CodeArtsTransaction::NotEntered {
                error,
                storage_closed,
            } => SlotOutcome::NotEntered {
                error,
                storage_closed,
            },
            CodeArtsTransaction::Rejected {
                error,
                storage_closed,
            } => SlotOutcome::Rejected {
                error,
                storage_closed,
            },
            CodeArtsTransaction::CommitUnconfirmed { storage_closed } => {
                SlotOutcome::CommitUnconfirmed { storage_closed }
            }
        })
    }
}

impl PendingSlot {
    pub(crate) fn binding(&self) -> PendingBinding {
        PendingBinding { source: Arc::downgrade(&self.binding) }
    }

    pub(crate) fn account(&self) -> &str {
        self.original.account()
    }

    /// 按值消费原操作；仅固定五项新材料候选，原调用/真实响应所有者仍由未来worker核对。
    /// 纯候选预检在账号/Db锁外；坏候选只凭原Pending写Blocked，不创建下一Ready。
    pub(crate) fn complete(
        self,
        credentials: &Value,
    ) -> Result<CodeArtsTransaction<SlotSettlement, SlotFailure>, Error> {
        let replacement = match slots::prepare_replacement(credentials) {
            Ok(replacement) => replacement,
            Err(error) => return Ok(blocked_result(self.block()?, error)),
        };
        let key = cached_key()?;
        Ok(original_transaction(
            &self.original.store,
            &self.original.source,
            |conn, retained| {
                slots::complete(
                    conn,
                    &key,
                    &self.original.inner,
                    &self.write,
                    &replacement,
                    retained,
                )
            },
        ))
    }

    /// 放弃/网络未知仅凭同一次真实Pending收据封锁；丢弃本对象保持耐久Pending。
    /// 未成功收尾也不恢复Ready，不按账号ID/超时猜测原操作或从新库重取来源。
    pub(crate) fn block(self) -> Result<CodeArtsTransaction<(), SlotFailure>, Error> {
        let key = cached_key()?;
        Ok(original_transaction(
            &self.original.store,
            &self.original.source,
            |conn, _| slots::block(conn, &key, &self.original.inner, &self.write),
        ))
    }
}

fn blocked_result(
    outcome: CodeArtsTransaction<(), SlotFailure>,
    reason: SlotFailure,
) -> CodeArtsTransaction<SlotSettlement, SlotFailure> {
    match outcome {
        CodeArtsTransaction::Committed { storage_closed, .. } => CodeArtsTransaction::Committed {
            value: SlotSettlement::Blocked(reason),
            storage_closed,
        },
        CodeArtsTransaction::NotEntered {
            error,
            storage_closed,
        } => CodeArtsTransaction::NotEntered {
            error,
            storage_closed,
        },
        CodeArtsTransaction::Rejected {
            error,
            storage_closed,
        } => CodeArtsTransaction::Rejected {
            error,
            storage_closed,
        },
        CodeArtsTransaction::CommitUnconfirmed { storage_closed } => {
            CodeArtsTransaction::CommitUnconfirmed { storage_closed }
        }
    }
}

/// T/原行/替换字段/异常载荷在Db动作外持有；Db只提交unit，提交失败不能在外层锁内丢T。
/// action仅是本文件固定同步SQL借用，捕获不持可外发材料、联网闭包或任意存储来源。
fn original_transaction<T>(
    store: &AccountStore,
    source: &CredentialSource,
    action: impl FnOnce(&Connection, &mut slots::RetainedRows) -> Result<T, SlotFailure>,
) -> CodeArtsTransaction<T, SlotFailure> {
    let mut retained = slots::RetainedRows::new();
    let mut candidate = None;
    let mut action = Some(action);
    let mut entered = false;
    let mut commit = None;
    let mut action_unwind: Option<Box<dyn Any + Send>> = None;
    let attempt = catch_unwind(AssertUnwindSafe(|| {
        let _guard = store.guard();
        let Some(db) = store.database() else {
            return CodeArtsTransaction::NotEntered {
                error: CodeArtsStorageError::Unavailable,
                storage_closed: false,
            };
        };
        db.codearts_slot_transaction_with_fact(source, &mut commit, |conn| {
            entered = true;
            let result = catch_unwind(AssertUnwindSafe(|| {
                let action = action.take().ok_or(SlotFailure::Interrupted)?;
                action(conn, &mut retained)
            }));
            match result {
                Ok(Ok(value)) => {
                    candidate = Some(value);
                    Ok(())
                }
                Ok(Err(error)) => Err(error),
                Err(payload) => {
                    action_unwind = Some(payload);
                    Err(SlotFailure::Interrupted)
                }
            }
        })
    }));
    let mut outer_unwind = None;
    let outcome = match attempt {
        Ok(CodeArtsTransaction::Committed { storage_closed, .. }) => match candidate.take() {
            Some(value) => CodeArtsTransaction::Committed {
                value,
                storage_closed,
            },
            None => CodeArtsTransaction::CommitUnconfirmed { storage_closed },
        },
        Ok(CodeArtsTransaction::NotEntered {
            error,
            storage_closed,
        }) => CodeArtsTransaction::NotEntered {
            error,
            storage_closed,
        },
        Ok(CodeArtsTransaction::Rejected {
            error,
            storage_closed,
        }) => CodeArtsTransaction::Rejected {
            error,
            storage_closed,
        },
        Ok(CodeArtsTransaction::CommitUnconfirmed { storage_closed }) => {
            CodeArtsTransaction::CommitUnconfirmed { storage_closed }
        }
        Err(payload) => {
            outer_unwind = Some(payload);
            if commit == Some(true) {
                match candidate.take() {
                    Some(value) => CodeArtsTransaction::Committed { value, storage_closed: false },
                    None => CodeArtsTransaction::CommitUnconfirmed { storage_closed: false },
                }
            } else if entered {
                CodeArtsTransaction::CommitUnconfirmed {
                    storage_closed: false,
                }
            } else {
                CodeArtsTransaction::NotEntered {
                    error: CodeArtsStorageError::Unavailable,
                    storage_closed: false,
                }
            }
        }
    };
    // 包括COMMIT/设置恢复异常：所有外层存储锁已退出，才处理未交付值和异常析构。
    drop(candidate);
    drop(action);
    drop(retained);
    drop([action_unwind, outer_unwind]);
    outcome
}
