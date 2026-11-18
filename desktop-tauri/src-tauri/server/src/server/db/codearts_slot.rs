//! CodeArts 封闭发送槽的原连接耐久事务。此模块本身不签发发送许可。
//! FULL 必须在 BEGIN 之前设置；只承诺 SQLite/文件系统/设备遵约范围，不是 exactly-once。
//! 官方依据：https://sqlite.org/pragma.html#pragma_synchronous 、https://sqlite.org/wal.html 。

mod final_check;

use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{Connection, ErrorCode};

use super::{CredentialSource, Db};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodeArtsStorageError {
    Unavailable,
    SourceChanged,
    NestedTransaction,
    JournalMode,
    Synchronous,
    Busy,
    Begin,
}

/// COMMIT 事实与连接收尾分开。Rejected 也不等于回滚已确认，须同时检查 storage_closed。
/// Committed + storage_closed=false 仍然是已提交，不能补偿回 Ready 或当作未写入重试。
#[must_use]
pub(crate) enum CodeArtsTransaction<T, E> {
    NotEntered {
        error: CodeArtsStorageError,
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
    /// 调用方已持原 AccountStore 锁；来源只核对原 Db 对象及私有 Arc，不重新 prepare。
    /// action 只能做同库、有界 SQL；不能出网、await、另起事务、重取外层锁或签发许可。
    pub(crate) fn codearts_slot_transaction<T, E>(
        &self,
        source: &CredentialSource,
        action: impl FnOnce(&Connection) -> Result<T, E>,
    ) -> CodeArtsTransaction<T, E> {
        let mut commit = None;
        self.codearts_slot_transaction_with_fact(source, &mut commit, action)
    }

    /// 只为原固定存储路径外持COMMIT事实；不是新来源或可重复使用的提交许可。
    /// commit须在调用方catch_unwind外持有，已非None时不进连接、不执行action、不改写事实。
    pub(crate) fn codearts_slot_transaction_with_fact<T, E>(
        &self,
        source: &CredentialSource,
        commit: &mut Option<bool>,
        action: impl FnOnce(&Connection) -> Result<T, E>,
    ) -> CodeArtsTransaction<T, E> {
        if commit.is_some() {
            return unavailable();
        }
        let state = match self.inner.conn.lock() {
            Ok(state) => state,
            Err(_) => return unavailable(),
        };
        if self.inner.task_guard_unusable.load(Ordering::Acquire) {
            return unavailable();
        }
        if !source.matches(self, &state) {
            return CodeArtsTransaction::NotEntered {
                error: CodeArtsStorageError::SourceChanged,
                storage_closed: state.is_autocommit(),
            };
        }
        let conn: &Connection = &state;
        full_transaction_with_fact(conn, &self.inner.task_guard_unusable, commit, action)
    }
}

/// 只供原连接已受锁的宿主存储/恢复链复用；本入口不接账号、许可或外部状态。
/// 恢复前轮换 epoch 与槽变更使用相同 FULL/IMMEDIATE/COMMIT/设置恢复语义。
pub(super) fn full_transaction<T, E>(
    conn: &Connection,
    unusable: &AtomicBool,
    action: impl FnOnce(&Connection) -> Result<T, E>,
) -> CodeArtsTransaction<T, E> {
    let mut commit = None;
    full_transaction_with_fact(conn, unusable, &mut commit, action)
}

/// 固定FULL事务的外持事实入口；只借原受锁连接，不增加任意SQL或来源构造能力。
/// None未执行COMMIT，Some(false)已尝试但未确认，Some(true)真实确认且永不回退。
pub(super) fn full_transaction_with_fact<T, E>(
    conn: &Connection,
    unusable: &AtomicBool,
    commit: &mut Option<bool>,
    action: impl FnOnce(&Connection) -> Result<T, E>,
) -> CodeArtsTransaction<T, E> {
    if commit.is_some() {
        return unavailable();
    }
    if !conn.is_autocommit() {
        // 进入前已有未结束事务，不接管、不隐式提交；同样隔离避免随后交普通CRUD。
        unusable.store(true, Ordering::Release);
        return CodeArtsTransaction::NotEntered {
            error: CodeArtsStorageError::NestedTransaction,
            storage_closed: false,
        };
    }
    // WAL+FULL 的提交语义不能套用到内存库、OFF、MEMORY 或回滚日志模式。
    if !wal(conn) {
        return CodeArtsTransaction::NotEntered {
            error: CodeArtsStorageError::JournalMode,
            storage_closed: true,
        };
    }
    let Some(original_sync) = synchronous(conn) else {
        return CodeArtsTransaction::NotEntered {
            error: CodeArtsStorageError::Synchronous,
            storage_closed: true,
        };
    };
    let required = original_sync.max(2); // 不把原有 EXTRA 降低；WAL 下至少 FULL。
    let mut scope = FullScope {
        conn,
        unusable,
        original_sync,
        began: false,
        confirmed_commit: false,
        closed: false,
    };
    if conn.pragma_update(None, "synchronous", required).is_err()
        || synchronous(conn) != Some(required)
    {
        return CodeArtsTransaction::NotEntered {
            error: CodeArtsStorageError::Synchronous,
            storage_closed: scope.finish(),
        };
    }
    let mut begin = match conn.prepare("BEGIN IMMEDIATE") {
        Ok(begin) => begin,
        Err(error) => {
            let error = match error.sqlite_error_code() {
                Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => CodeArtsStorageError::Busy,
                _ => CodeArtsStorageError::Begin,
            };
            return CodeArtsTransaction::NotEntered {
                error,
                storage_closed: scope.finish(),
            };
        }
    };
    let executed = begin.execute([]);
    if executed.is_ok() {
        scope.began = true;
    }
    let finalized = begin.finalize();
    if let Err(error) = executed {
        let error = match error.sqlite_error_code() {
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => CodeArtsStorageError::Busy,
            _ => CodeArtsStorageError::Begin,
        };
        return CodeArtsTransaction::NotEntered {
            error,
            storage_closed: scope.finish(),
        };
    }
    if finalized.is_err() {
        return CodeArtsTransaction::NotEntered {
            error: CodeArtsStorageError::Begin,
            storage_closed: scope.finish(),
        };
    }
    if conn.is_autocommit() {
        return CodeArtsTransaction::NotEntered {
            error: CodeArtsStorageError::Begin,
            storage_closed: scope.finish(),
        };
    }
    let result = action(conn);
    // SQL 动作不可自行结束事务；异常结束不能被包装成正常提交或正常拒绝。
    if conn.is_autocommit() || synchronous(conn) != Some(required) || !wal(conn) {
        unusable.store(true, Ordering::Release);
        return CodeArtsTransaction::CommitUnconfirmed {
            storage_closed: scope.finish(),
        };
    }
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            return CodeArtsTransaction::Rejected {
                error,
                storage_closed: scope.finish(),
            };
        }
    };
    // COMMIT只执行一次；execute成功且autocommit后先写外层事实，再finalize/恢复设置。
    let mut statement = match conn.prepare("COMMIT") {
        Ok(statement) => statement,
        Err(_) => {
            unusable.store(true, Ordering::Release);
            return CodeArtsTransaction::CommitUnconfirmed {
                storage_closed: scope.finish(),
            };
        }
    };
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
            unusable.store(true, Ordering::Release);
        }
        let storage_closed = scope.finish();
        return CodeArtsTransaction::Committed {
            value,
            storage_closed: storage_closed && finalized.is_ok(),
        };
    }
    unusable.store(true, Ordering::Release);
    CodeArtsTransaction::CommitUnconfirmed {
        storage_closed: scope.finish(),
    }
}

fn unavailable<T, E>() -> CodeArtsTransaction<T, E> {
    CodeArtsTransaction::NotEntered {
        error: CodeArtsStorageError::Unavailable,
        storage_closed: false,
    }
}

fn control(conn: &Connection, sql: &'static str) -> rusqlite::Result<()> {
    let mut statement = conn.prepare(sql)?;
    let executed = statement.execute([]);
    let finalized = statement.finalize();
    executed?;
    finalized
}

fn synchronous(conn: &Connection) -> Option<i64> {
    let mut statement = conn.prepare("PRAGMA main.synchronous").ok()?;
    let value: i64 = statement.query_one([], |row| row.get(0)).ok()?;
    statement.finalize().ok()?;
    (0..=3).contains(&value).then_some(value)
}

fn wal(conn: &Connection) -> bool {
    let Ok(mut statement) = conn.prepare("PRAGMA main.journal_mode") else {
        return false;
    };
    let value: rusqlite::Result<String> = statement.query_one([], |row| row.get(0));
    statement.finalize().is_ok() && value.is_ok_and(|value| value == "wal")
}

struct FullScope<'a> {
    conn: &'a Connection,
    unusable: &'a AtomicBool,
    original_sync: i64,
    began: bool,
    confirmed_commit: bool,
    closed: bool,
}

/// 收尾 panic 也不能把半截事务或被改过的设置交给排队的普通存储调用。
struct Closing<'a> {
    unusable: &'a AtomicBool,
    confirmed: bool,
}

impl Drop for Closing<'_> {
    fn drop(&mut self) {
        if !self.confirmed {
            self.unusable.store(true, Ordering::Release);
        }
    }
}

impl FullScope<'_> {
    fn finish(mut self) -> bool {
        self.close()
    }

    fn close(&mut self) -> bool {
        self.closed = true;
        let mut closing = Closing {
            unusable: self.unusable,
            confirmed: false,
        };
        let transaction_ok = if self.confirmed_commit {
            self.conn.is_autocommit()
        } else if !self.conn.is_autocommit() {
            control(self.conn, "ROLLBACK").is_ok() && self.conn.is_autocommit()
        } else {
            // BEGIN后的事务若自行消失，不能当作回滚或提交已确认。
            !self.began
        };
        let restore_ok = self
            .conn
            .pragma_update(None, "synchronous", self.original_sync)
            .is_ok();
        closing.confirmed = transaction_ok
            && restore_ok
            && self.conn.is_autocommit()
            && synchronous(self.conn) == Some(self.original_sync);
        closing.confirmed
    }
}

impl Drop for FullScope<'_> {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.close();
        }
    }
}
