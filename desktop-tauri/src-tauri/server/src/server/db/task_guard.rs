//! 任务锁链末端的一次性try入口。只为只读复核保留IMMEDIATE顺序，不写账号或配置。
//! 当前Db只使用busy_timeout、不安装自定义busy handler；恢复进入前的确切毫秒值。
//! 不等待锁、不重试BEGIN、不日志；回调只能同步、纯内存，不能反向获取外层锁。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{atomic::{AtomicBool, Ordering}, TryLockError};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode};

use super::{CredentialSource, Db};

#[derive(Clone, Copy)]
pub(crate) enum TaskTransactionError {
    Busy,
    Poisoned,
    Unavailable,
    NestedTransaction,
    BusyTimeout,
    Begin,
    Close,
}

/// Completed只表示存储动作已进入，不代表业务消费回调已调用；上层必须单独记录。
/// unwind载荷只交回外层丢弃，不在账号/Db锁内格式化、记录或执行它的析构。
#[must_use]
pub(crate) enum TaskTransaction<T> {
    NotEntered(TaskTransactionError),
    Completed {
        outcome: std::thread::Result<T>,
        storage_closed: bool,
    },
}

impl Db {
    pub(crate) fn try_task_transaction<T>(
        &self,
        action: impl FnOnce(&Connection) -> T,
    ) -> TaskTransaction<T> {
        self.try_task_transaction_inner(None, action)
    }

    /// 原来源在同一次conn try锁内核对，失败不进入事务，也不回退无来源入口。
    pub(crate) fn try_task_transaction_from<T>(
        &self,
        source: &CredentialSource,
        action: impl FnOnce(&Connection) -> T,
    ) -> TaskTransaction<T> {
        self.try_task_transaction_inner(Some(source), action)
    }

    fn try_task_transaction_inner<T>(
        &self,
        source: Option<&CredentialSource>,
        action: impl FnOnce(&Connection) -> T,
    ) -> TaskTransaction<T> {
        let conn = match self.inner.conn.try_lock() {
            Ok(conn) => conn,
            Err(TryLockError::WouldBlock) => return TaskTransaction::NotEntered(TaskTransactionError::Busy),
            Err(TryLockError::Poisoned(_)) => return TaskTransaction::NotEntered(TaskTransactionError::Poisoned),
        };
        if self.inner.task_guard_unusable.load(Ordering::Acquire)
            || source.is_some_and(|source| !source.matches(self, &conn))
        {
            return TaskTransaction::NotEntered(TaskTransactionError::Unavailable);
        }
        if !conn.is_autocommit() {
            // 不嵌套、不收拾调用前就存在的事务，也不修改它的busy设置。
            return TaskTransaction::NotEntered(TaskTransactionError::NestedTransaction);
        }
        // 这是连接级PRAGMA，不读取schema或账号数据；设0之前先记住原值。
        let Some(original_ms) = busy_millis(&conn) else {
            return TaskTransaction::NotEntered(TaskTransactionError::BusyTimeout);
        };
        let mut scope = TaskScope {
            conn: &conn,
            unusable: &self.inner.task_guard_unusable,
            original_ms,
            begin_attempted: false,
            began: false,
            closed: false,
        };
        if conn.busy_timeout(Duration::ZERO).is_err() {
            return rejected(scope, TaskTransactionError::BusyTimeout);
        }
        // 在BEGIN之前登记清理责任，BEGIN错误和unwind也会恢复原设置。
        scope.begin_attempted = true;
        if let Err(error) = control_statement(&conn, "BEGIN IMMEDIATE") {
            let reason = match error.sqlite_error_code() {
                Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => TaskTransactionError::Busy,
                _ => TaskTransactionError::Begin,
            };
            return rejected(scope, reason);
        }
        scope.began = true;
        if conn.is_autocommit() {
            return rejected(scope, TaskTransactionError::Begin);
        }
        // 仅unwind构建能捕获；panic=abort、进程强杀和外部替换库文件不在保证内。
        let outcome = catch_unwind(AssertUnwindSafe(|| action(&conn)));
        let storage_closed = scope.finish();
        TaskTransaction::Completed { outcome, storage_closed }
    }
}

fn rejected<T>(scope: TaskScope<'_>, reason: TaskTransactionError) -> TaskTransaction<T> {
    let reason = if scope.finish() { reason } else { TaskTransactionError::Close };
    TaskTransaction::NotEntered(reason)
}

fn control_statement(conn: &Connection, sql: &'static str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let executed = stmt.execute([]);
    let finalized = stmt.finalize();
    executed?;
    finalized
}

fn busy_millis(conn: &Connection) -> Option<u64> {
    let mut stmt = conn.prepare("PRAGMA busy_timeout").ok()?;
    let value: i64 = stmt.query_one([], |row| row.get(0)).ok()?;
    stmt.finalize().ok()?;
    (0..=i64::from(i32::MAX)).contains(&value).then_some(value as u64)
}

struct TaskScope<'a> {
    conn: &'a Connection,
    unusable: &'a AtomicBool,
    original_ms: u64,
    begin_attempted: bool,
    began: bool,
    closed: bool,
}

/// 收尾中途unwind也不能把未确认的连接交给已经排队的普通CRUD。
struct Unconfirmed<'a> {
    unusable: &'a AtomicBool,
    confirmed: bool,
}

impl Drop for Unconfirmed<'_> {
    fn drop(&mut self) {
        if !self.confirmed {
            self.unusable.store(true, Ordering::Release);
        }
    }
}

impl TaskScope<'_> {
    fn finish(mut self) -> bool {
        self.close()
    }

    fn close(&mut self) -> bool {
        self.closed = true;
        let mut state = Unconfirmed { unusable: self.unusable, confirmed: false };
        // 只读事务统一ROLLBACK：不提交任何业务写入；同一连接锁一直保留到恢复结束。
        let transaction_ok = if self.begin_attempted && !self.conn.is_autocommit() {
            control_statement(self.conn, "ROLLBACK").is_ok()
        } else {
            // 已成功BEGIN的事务在动作结束前消失不能视为正常收尾。
            !self.began
        };
        // 即使ROLLBACK出错，也必须尝试恢复；没有sleep、循环或第二次业务动作。
        let restore_ok = self.conn.busy_timeout(Duration::from_millis(self.original_ms)).is_ok();
        let restored = busy_millis(self.conn) == Some(self.original_ms);
        state.confirmed = self.conn.is_autocommit() && restored;
        // 普通竞争/账号拒绝/业务拒绝/unwind不会隔离Db；只有未确认恢复的状态才隔离。
        transaction_ok && restore_ok && state.confirmed
    }
}

impl Drop for TaskScope<'_> {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.close();
        }
    }
}
