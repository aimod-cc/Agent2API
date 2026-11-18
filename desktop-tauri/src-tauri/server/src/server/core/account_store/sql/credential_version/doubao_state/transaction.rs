//! 封闭事务收尾：不重试BEGIN/COMMIT；只有COMMIT真实成功才交付值。
//! 此处不持Db字段；回滚仍未结束时由原commit_credential_source隔离连接再释放原锁。

use rusqlite::{Connection, DropBehavior, Transaction, TransactionBehavior};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{storage, Error, Outcome, StateError, StorageFailure};

/// 只记本次提交语句是否已尝试；Db来源借用复核的异常也不能把已进入提交误降为NotEntered。
/// 无法由JSON或自由bool恢复，借用由封闭read/save入口持有。
pub(in crate::server::core::account_store) struct CommitAttempt {
    entered: AtomicBool,
    attempted: AtomicBool,
}

impl CommitAttempt {
    pub(in crate::server::core::account_store) fn new() -> Self {
        Self { entered: AtomicBool::new(false), attempted: AtomicBool::new(false) }
    }
    pub(in crate::server::core::account_store) fn entered(&self) -> bool { self.entered.load(Ordering::Acquire) }
    pub(in crate::server::core::account_store) fn attempted(&self) -> bool { self.attempted.load(Ordering::Acquire) }
}

pub(super) fn begin<'a, T>(conn: &'a mut Connection, attempt: &CommitAttempt) -> Result<Transaction<'a>, Outcome<T>> {
    if !conn.is_autocommit() {
        return Err(Outcome::NotEntered {
            error: Error::Storage(StorageFailure::Begin).into(), storage_closed: false,
        });
    }
    let mut tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|error| Outcome::NotEntered {
        // BEGIN报错本身不是收尾证明；原Db借用另核对autocommit并隔离未收尾连接。
        error: storage::sql_error(error, StorageFailure::Begin).into(), storage_closed: false,
    })?;
    // 从创建起即禁隐式SQL收尾；action异常由外层捕获，原Db借用隔离未结束连接。
    // 不能等finish才设置，否则read/解析异常会让Drop在Db锁内另作回滚尝试。
    tx.set_drop_behavior(DropBehavior::Ignore);
    attempt.entered.store(true, Ordering::Release);
    Ok(tx)
}

pub(super) fn finish<T>(mut tx: Transaction<'_>, result: Result<T, StateError>, attempt: &CommitAttempt) -> Outcome<T> {
    // 收尾由下面唯一显式COMMIT/ROLLBACK负责；失败后不由Drop再隐式尝试一次。
    // 未结束的连接必须由原Db借用在释放锁前隔离，不能继续普通账号写入。
    tx.set_drop_behavior(DropBehavior::Ignore);
    let conn: &Connection = &tx;
    match result {
        Ok(value) => {
            attempt.attempted.store(true, Ordering::Release);
            // 如果动作意外结束了事务，则不能把“已无事务”视为正常提交。
            if conn.is_autocommit() {
                return Outcome::CommitUnknown {
                    error: Error::Storage(StorageFailure::CommitUnconfirmed).into(), storage_closed: true,
                };
            }
            let committed = control(conn, "COMMIT");
            if committed.is_ok() && conn.is_autocommit() {
                return Outcome::Committed(value);
            }
            // 不重发COMMIT，不返回candidate/revision。仍有事务时只做一次显式ROLLBACK。
            let storage_closed = close(conn);
            Outcome::CommitUnknown {
                error: Error::Storage(StorageFailure::CommitUnconfirmed).into(), storage_closed,
            }
        }
        Err(error) => {
            let storage_closed = close(conn);
            // 即使ROLLBACK未确认也保留“业务已拒绝”；storage_closed=false单独说明收尾未知。
            Outcome::Rejected { error, storage_closed }
        }
    }
}

fn close(conn: &Connection) -> bool {
    conn.is_autocommit() || control(conn, "ROLLBACK").is_ok() && conn.is_autocommit()
}

fn control(conn: &Connection, sql: &'static str) -> rusqlite::Result<()> {
    let mut statement = conn.prepare(sql)?;
    let executed = statement.execute([]);
    let finalized = statement.finalize();
    executed?;
    finalized
}
