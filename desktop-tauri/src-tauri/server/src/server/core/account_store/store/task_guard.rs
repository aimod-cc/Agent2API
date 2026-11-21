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
