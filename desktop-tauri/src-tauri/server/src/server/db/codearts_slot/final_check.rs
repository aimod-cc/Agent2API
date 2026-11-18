//! 原CodeArts来源的非等待FULL/IMMEDIATE最终事务。只借本次原连接，不读账号或签发许可。
//! 回调和结果均为固定unit/bool；COMMIT事实外持，收尾/panic不能把已提交改成未进入。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    TryLockError,
};
use std::time::Duration;

use rusqlite::Connection;

use super::{control, synchronous, wal, CodeArtsStorageError, CodeArtsTransaction};
use crate::server::db::{CredentialSource, Db};

impl Db {
    /// 原账号try锁由固定存储helper持有；只try原Db锁，不等待、不换来源、不复用ROLLBACK门。
    /// commit=None未尝试，Some(false)已尝试但未确认，Some(true)COMMIT已确认且不再回退。
    /// action只做固定原Pending SQL核对及一次内存CAS；不得网络/await/spawn/反向取锁。
    pub(crate) fn try_codearts_final_transaction(
        &self,
        source: &CredentialSource,
        commit: &mut Option<bool>,
        action: &mut impl FnMut(&Connection) -> bool,
    ) -> CodeArtsTransaction<(), ()> {
        if commit.is_some() {
            return not_entered(CodeArtsStorageError::Unavailable, false);
        }
        let state = match self.inner.conn.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return not_entered(CodeArtsStorageError::Busy, false),
            Err(TryLockError::Poisoned(_)) => {
                return not_entered(CodeArtsStorageError::Unavailable, false)
            }
        };
        let conn: &Connection = &state;
        if self.inner.task_guard_unusable.load(Ordering::Acquire) {
            return not_entered(CodeArtsStorageError::Unavailable, conn.is_autocommit());
        }
        if !source.matches(self, &state) {
            return not_entered(CodeArtsStorageError::SourceChanged, conn.is_autocommit());
        }
        if !conn.is_autocommit() {
            self.inner
                .task_guard_unusable
                .store(true, Ordering::Release);
            return not_entered(CodeArtsStorageError::NestedTransaction, false);
        }
        if !wal(conn) {
            return not_entered(CodeArtsStorageError::JournalMode, true);
        }
        let Some(original_sync) = synchronous(conn) else {
            return not_entered(CodeArtsStorageError::Synchronous, true);
        };
        let Some(original_busy) = busy_millis(conn) else {
            return not_entered(CodeArtsStorageError::Unavailable, true);
        };
        let required_sync = original_sync.max(2);
        let mut scope = FinalScope {
            conn,
            unusable: &self.inner.task_guard_unusable,
            original_sync,
            original_busy,
            began: false,
            confirmed_commit: false,
            closed: false,
        };
        if conn.busy_timeout(Duration::ZERO).is_err() || busy_millis(conn) != Some(0) {
            return not_entered(CodeArtsStorageError::Unavailable, scope.finish());
        }
        if conn
            .pragma_update(None, "synchronous", required_sync)
            .is_err()
            || synchronous(conn) != Some(required_sync)
        {
            return not_entered(CodeArtsStorageError::Synchronous, scope.finish());
        }
        // BEGIN执行成功后先登记，再finalize；后续异常仍按真正已开始的事务处理。
        let mut begin = match conn.prepare("BEGIN IMMEDIATE") {
            Ok(begin) => begin,
            Err(_) => return not_entered(CodeArtsStorageError::Begin, scope.finish()),
        };
        let executed = begin.execute([]);
        if executed.is_ok() {
            scope.began = true;
        }
        let finalized = begin.finalize();
        if let Err(error) = executed {
            let failure = if matches!(
                error.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
            ) {
                CodeArtsStorageError::Busy
            } else {
                CodeArtsStorageError::Begin
            };
            return not_entered(failure, scope.finish());
        }
        if finalized.is_err() {
            return not_entered(CodeArtsStorageError::Begin, scope.finish());
        }
        if conn.is_autocommit() {
            return not_entered(CodeArtsStorageError::Begin, scope.finish());
        }
        let accepted = action(conn);
        // 固定同步动作不得自己结束事务/改设置；异常只保留未知事实，不再提交或补造资格。
        if conn.is_autocommit() || synchronous(conn) != Some(required_sync) || !wal(conn) {
            self.inner
                .task_guard_unusable
                .store(true, Ordering::Release);
            return CodeArtsTransaction::CommitUnconfirmed {
                storage_closed: scope.finish(),
            };
        }
        if !accepted {
            return CodeArtsTransaction::Rejected {
                error: (),
                storage_closed: scope.finish(),
            };
        }
        let mut statement = match conn.prepare("COMMIT") {
            Ok(statement) => statement,
            Err(_) => {
                self.inner
                    .task_guard_unusable
                    .store(true, Ordering::Release);
                return CodeArtsTransaction::CommitUnconfirmed {
                    storage_closed: scope.finish(),
                };
            }
        };
        // 在执行前占用尝试事实；不重试。execute成功+autocommit后先写外层旗标，
        // 再finalize和恢复设置，后两者失败/unwind不抹掉真实COMMIT成功。
        *commit = Some(false);
        let executed = statement.execute([]);
        let confirmed = executed.is_ok() && conn.is_autocommit();
        if confirmed {
            *commit = Some(true);
            scope.confirmed_commit = true;
        }
        let finalized = statement.finalize();
        if confirmed {
            if finalized.is_err() {
                self.inner
                    .task_guard_unusable
                    .store(true, Ordering::Release);
            }
            let closed = scope.finish();
            return CodeArtsTransaction::Committed {
                value: (),
                storage_closed: closed && finalized.is_ok(),
            };
        }
        self.inner
            .task_guard_unusable
            .store(true, Ordering::Release);
        CodeArtsTransaction::CommitUnconfirmed {
            storage_closed: scope.finish(),
        }
    }
}

fn not_entered(error: CodeArtsStorageError, storage_closed: bool) -> CodeArtsTransaction<(), ()> {
    CodeArtsTransaction::NotEntered {
        error,
        storage_closed,
    }
}

fn busy_millis(conn: &Connection) -> Option<u64> {
    let mut statement = conn.prepare("PRAGMA busy_timeout").ok()?;
    let value: i64 = statement.query_one([], |row| row.get(0)).ok()?;
    statement.finalize().ok()?;
    (0..=i64::from(i32::MAX))
        .contains(&value)
        .then_some(value as u64)
}

struct FinalScope<'a> {
    conn: &'a Connection,
    unusable: &'a AtomicBool,
    original_sync: i64,
    original_busy: u64,
    began: bool,
    confirmed_commit: bool,
    closed: bool,
}

struct RestoreGuard<'a> {
    unusable: &'a AtomicBool,
    confirmed: bool,
}
impl Drop for RestoreGuard<'_> {
    fn drop(&mut self) {
        if !self.confirmed {
            self.unusable.store(true, Ordering::Release);
        }
    }
}

impl FinalScope<'_> {
    fn finish(mut self) -> bool {
        self.close()
    }

    fn close(&mut self) -> bool {
        self.closed = true;
        let mut guard = RestoreGuard {
            unusable: self.unusable,
            confirmed: false,
        };
        let transaction_ok = if self.confirmed_commit {
            self.conn.is_autocommit()
        } else if !self.conn.is_autocommit() {
            control(self.conn, "ROLLBACK").is_ok() && self.conn.is_autocommit()
        } else {
            // action/外部异常令已BEGIN事务自行消失不能算正常回滚或正常提交。
            !self.began
        };
        let sync_ok = self
            .conn
            .pragma_update(None, "synchronous", self.original_sync)
            .is_ok();
        let busy_ok = self
            .conn
            .busy_timeout(Duration::from_millis(self.original_busy))
            .is_ok();
        guard.confirmed = transaction_ok
            && sync_ok
            && busy_ok
            && self.conn.is_autocommit()
            && synchronous(self.conn) == Some(self.original_sync)
            && busy_millis(self.conn) == Some(self.original_busy)
            && wal(self.conn);
        guard.confirmed
    }
}

impl Drop for FinalScope<'_> {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.close();
        }
    }
}
