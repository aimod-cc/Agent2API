//! 最终账号读取只做一次账号try锁，再做一次Db try事务；不经过普通快照缓存。

use std::{any::Any, panic::{catch_unwind, AssertUnwindSafe}, sync::TryLockError};

use super::super::{plugin_accounts::{task_projection, TaskAccountProjection, TaskProjectionError}, sql};
use super::AccountStore;
use super::super::credential_version::{Error as RowReadError, RowFinalError, RowFinalRead};
use crate::server::db::TaskTransaction;

pub(crate) enum TaskStoreError {
    Busy,
    Poisoned,
    Unavailable,
    Read(sql::TaskReadError),
    Projection(TaskProjectionError),
}

impl AccountStore {
    pub(in crate::server::core::account_store) fn bound_credential_transaction(
        &self, source: &crate::server::db::CredentialSource, commit: &mut Option<bool>,
        action: &mut impl FnMut(&rusqlite::Connection) -> bool,
    ) -> Result<crate::server::db::CodeArtsTransaction<(), ()>, ()> {
        let _account = self.inner.lock.try_lock().map_err(|_| ())?;
        let db = self.inner.db.as_ref().ok_or(())?;
        Ok(db.try_codearts_final_transaction(source, commit, action))
    }

    /// 固定原Pending保存终检；所有候选、返回值与异常均由Call门外Check持有。
    /// 不导出自由账号/Db/SQL动作；consume只有原Call的一次布尔内存准入。
    pub(in crate::server::core::account_store) fn try_codearts_pending_completion(
        read: &super::super::credential_version::PendingCompletionRead<'_>,
        preparation: &mut Option<Result<sql::credential_version::codearts_slot::Replacement, super::super::credential_version::SlotFailure>>,
        commit: &mut Option<bool>,
        entered: &mut bool,
        returned: &mut Option<Result<super::super::credential_version::SlotSettlement, super::super::credential_version::SlotFailure>>,
        sql_error: &mut Option<super::super::credential_version::SlotFailure>,
        retained: &mut [sql::credential_version::codearts_slot::RetainedRows; 2],
        consume: &mut impl FnMut() -> bool,
        deferred_unwind: &mut Option<Box<dyn Any + Send>>,
    ) -> Result<crate::server::db::CodeArtsTransaction<(), ()>, super::super::credential_version::PendingFinalError> {
        use super::super::credential_version::{PendingFinalError, SlotFailure, SlotSettlement};
        use sql::credential_version::codearts_slot as slots;
        read.with_original_store(|store, source, stamp, pending, key| {
            let _account = match store.inner.lock.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::WouldBlock) => return Err(PendingFinalError::AccountBusy),
                Err(TryLockError::Poisoned(_)) => return Err(PendingFinalError::AccountPoisoned),
            };
            let db = store.inner.db.as_ref().ok_or(PendingFinalError::StoreUnavailable)?;
            Ok(db.try_codearts_final_transaction(source, commit, &mut |conn| {
                match catch_unwind(AssertUnwindSafe(|| {
                    slots::verify_pending_current(conn, key, stamp, pending, &mut retained[0])?;
                    // 消费回调已进入先记事实；拒绝、异常和随后COMMIT收尾都不能改为未进入。
                    *entered = true;
                    if !consume() {
                        *returned = Some(Err(SlotFailure::Interrupted));
                        return Ok::<bool, SlotFailure>(false);
                    }
                    let result = match preparation.as_ref() {
                        Some(Ok(replacement)) => slots::complete(conn, key, stamp, pending, replacement, &mut retained[1]),
                        Some(Err(_)) => match slots::block(conn, key, stamp, pending) {
                            Ok(()) => match preparation.take() {
                                Some(Err(reason)) => Ok(SlotSettlement::Blocked(reason)),
                                other => { *preparation = other; Err(SlotFailure::Interrupted) },
                            },
                            Err(error) => Err(error),
                        },
                        None => Err(SlotFailure::Interrupted),
                    };
                    let accepted = result.is_ok();
                    *returned = Some(result);
                    Ok(accepted)
                })) {
                    Ok(Ok(accepted)) => accepted,
                    Ok(Err(error)) => { *sql_error = Some(error); false },
                    Err(payload) => { *deferred_unwind = Some(payload); false },
                }
            }))
        })
    }

    /// 原Pending封闭组合的唯一try账号入口；不开放inner/guard，不接自由账号或SQL动作。
    /// 仅在原Call专用门内调用，所有事实、原行和异常载荷都由门外所有者持有。
    pub(in crate::server::core::account_store) fn try_codearts_pending_final(
        read: &super::super::credential_version::PendingFinalRead<'_>,
        commit: &mut Option<bool>,
        entered: &mut bool,
        returned: &mut Option<bool>,
        sql_error: &mut Option<super::super::credential_version::SlotFailure>,
        retained: &mut sql::credential_version::codearts_slot::RetainedRows,
        consume: &mut impl FnMut() -> bool,
        deferred_unwind: &mut Option<Box<dyn Any + Send>>,
    ) -> Result<crate::server::db::CodeArtsTransaction<(), ()>, super::super::credential_version::PendingFinalError> {
        use super::super::credential_version::{PendingFinalError, SlotFailure};
        read.with_original_store(|store, source, stamp, pending, key| {
            let _account = match store.inner.lock.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::WouldBlock) => return Err(PendingFinalError::AccountBusy),
                Err(TryLockError::Poisoned(_)) => return Err(PendingFinalError::AccountPoisoned),
            };
            let db = store.inner.db.as_ref().ok_or(PendingFinalError::StoreUnavailable)?;
            Ok(db.try_codearts_final_transaction(source, commit, &mut |conn| {
                match catch_unwind(AssertUnwindSafe(|| {
                    sql::credential_version::codearts_slot::verify_pending_current(conn, key, stamp, pending, retained)?;
                    // Entered先于CAS，CAS异常或COMMIT/设置恢复失败不能把它改成未进入。
                    *entered = true;
                    *returned = Some(consume());
                    Ok::<(), SlotFailure>(())
                })) {
                    Ok(Ok(())) => true,
                    Ok(Err(error)) => { *sql_error = Some(error); false },
                    Err(payload) => { *deferred_unwind = Some(payload); false },
                }
            }))
        })
    }

    /// 只接RowSource内部借用组合，原store在组合内选定；不接替代Db/连接/账号目标。
    pub(in crate::server::core::account_store) fn try_row_source_final(
        read: &RowFinalRead<'_>,
        entered: &mut bool,
        consume: &mut impl FnMut(),
        deferred_unwind: &mut Option<Box<dyn Any + Send>>,
    ) -> Result<TaskTransaction<Result<(), RowReadError>>, RowFinalError> {
        read.with_original_store(|store, source, stamp, key| {
            let _account = match store.inner.lock.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::WouldBlock) => return Err(RowFinalError::AccountBusy),
                Err(TryLockError::Poisoned(_)) => return Err(RowFinalError::AccountPoisoned),
            };
            let db = store.inner.db.as_ref().ok_or(RowFinalError::StoreUnavailable)?;
            Ok(db.try_task_transaction_from(source, |conn| {
                // 先把动作异常载荷移到外层槽；后续Db收尾再次unwind也不能在锁内析构它。
                match catch_unwind(AssertUnwindSafe(|| {
                    stamp.verify_current(conn, key)?;
                    *entered = true;
                    consume();
                    Ok(())
                })) {
                    Ok(result) => result,
                    Err(payload) => {
                        *deferred_unwind = Some(payload);
                        Err(RowReadError::Storage(super::super::credential_version::StorageFailure::Unavailable))
                    }
                }
            }))
        })
    }

    /// 内部存储机制，不授予创建资格；调用方必须用可信快照比较投影后才能消费。
    /// 回调仍在账号锁、Db锁、IMMEDIATE事务内，不可联网、await、spawn或反向取锁。
    pub(crate) fn try_task_final<T>(
        &self,
        id: &str,
        action: impl FnOnce(TaskAccountProjection) -> T,
    ) -> Result<TaskTransaction<Result<T, TaskStoreError>>, TaskStoreError> {
        let _account = match self.inner.lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Err(TaskStoreError::Busy),
            Err(TryLockError::Poisoned(_)) => return Err(TaskStoreError::Poisoned),
        };
        let db = self.inner.db.as_ref().ok_or(TaskStoreError::Unavailable)?;
        Ok(db.try_task_transaction(|conn| {
            let record = sql::load_task_final(conn, id).map_err(TaskStoreError::Read)?;
            let projection = task_projection(&record, id).map_err(TaskStoreError::Projection)?;
            // 原始记录释放在消费之前；之后只借用有界最小投影，不保留凭据对象。
            drop(record);
            if conn.is_autocommit() { return Err(TaskStoreError::Unavailable); }
            Ok(action(projection))
        }))
    }
}
