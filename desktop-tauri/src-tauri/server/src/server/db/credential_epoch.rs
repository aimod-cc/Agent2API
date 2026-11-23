//! 与真实Db连接一起加锁的进程内凭据来源。只用于撤销旧读凭证，不是账号授权。
//! 代次是私有Arc对象，不落盘、不按路径恢复；替换失败或SQL回滚都不能回退旧对象。
//! 已知应用内恢复另在替换前耐久推进账号epoch；不解密重写任何账号或发送槽。

mod recovery;
pub(crate) use recovery::AccountChangeError;

use std::ops::{Deref, DerefMut};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Weak,
};

use rusqlite::Connection;

use super::{Db, DbInner};

struct Epoch;

/// 现有conn锁同时保护连接与代次；普通SQL及任务短事务仍只借用原Connection。
pub(super) struct ConnectionState {
    connection: Connection,
    epoch: Arc<Epoch>,
}

impl ConnectionState {
    pub(super) fn new(connection: Connection) -> Self {
        Self {
            connection,
            epoch: Arc::new(Epoch),
        }
    }

    fn revoke(&mut self) {
        // 活跃读凭证仍强持旧Arc，所以对象地址不会在它存活时被重新使用。
        // 没有计数器溢出、时间戳猜测或需要无限保留的历史代次表。
        self.epoch = Arc::new(Epoch);
    }
}

impl Deref for ConnectionState {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}

impl DerefMut for ConnectionState {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}

/// 无Clone/Debug/serde；只能在实际Db连接锁内完成读取后取得，不能由JSON拼装。
#[must_use]
pub(crate) struct CredentialSource {
    owner: Weak<DbInner>,
    epoch: Arc<Epoch>,
}

impl CredentialSource {
    pub(in crate::server::db) fn matches(&self, db: &Db, state: &ConnectionState) -> bool {
        Weak::ptr_eq(&self.owner, &Arc::downgrade(&db.inner))
            && Arc::ptr_eq(&self.epoch, &state.epoch)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CredentialSourceError {
    Unavailable,
    Changed,
}

/// 封闭的实际连接借用，不接另一个Db/Connection配对。不能移出外层锁的借用期。
pub(crate) struct AccountChange<'a> {
    state: &'a mut ConnectionState,
    unusable: &'a AtomicBool,
}

impl AccountChange<'_> {
    /// 旧迁移循环每项开始前检查；隔离或未收尾后不得继续使用同次连接借用。
    pub(in crate::server::db) fn available(&self) -> bool {
        !self.unusable.load(Ordering::Acquire) && self.state.connection.is_autocommit()
    }

    /// 非账号迁移项仍使用原连接签名；只有账号项取得可以撤销旧读凭证的借用。
    pub(in crate::server::db) fn connection(&self) -> &Connection {
        &self.state.connection
    }

    /// 先撤销进程来源，再在原受锁连接耐久轮换epoch，确认提交后才执行业务变更。
    /// action自行完成事务/备份替换；Err/unwind和安全备份回滚均不恢复已撤销代次。
    /// 仅保留Result入口，隔离错误单独说明阶段，不把“不明提交”说成“没写入”。
    pub(crate) fn replace<T, E: AccountChangeError>(
        &mut self,
        action: impl FnOnce(&mut Connection) -> Result<T, E>,
    ) -> Result<T, E> {
        self.state.revoke();
        recovery::advance(&self.state.connection, self.unusable).map_err(E::isolation)?;
        let mut scope = ClosedBorrow {
            connection: &mut self.state.connection,
            unusable: self.unusable,
        };
        // scope先于原连接锁析构；action发生unwind也会执行未结束事务检查。
        action(scope.connection())
    }

    /// 仅处理本次恢复链自己的临时库；原epoch先耐久撤销，临时库另取独立新epoch。
    /// 两项均确认后才交原连接创建安全备份/覆盖，不能从外来JSON恢复发送许可。
    pub(in crate::server::db) fn restore<T>(
        &mut self,
        staged: &Connection,
        action: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        self.replace(|main| {
            recovery::isolate_staged(staged, main)?;
            action(main)
        })
    }
}

impl Db {
    /// 新来源只伴随成功的真实读取返回；调用者负责有界SQL事务与COMMIT事实。
    /// 此对象本身不能构造ReadStamp，也没有重写账号或出网的能力。
    #[track_caller]
    pub(crate) fn prepare_credential_source<T, E>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T, E>,
    ) -> Result<Result<(T, CredentialSource), E>, CredentialSourceError> {
        let _reentrant_guard =
            super::ThreadLockGuard::enter(&self.inner).ok_or(CredentialSourceError::Unavailable)?;
        let mut state = self
            .inner
            .conn
            .lock()
            .map_err(|_| CredentialSourceError::Unavailable)?;
        if self.inner.task_guard_unusable.load(Ordering::Acquire) {
            return Err(CredentialSourceError::Unavailable);
        }
        let source = CredentialSource {
            owner: Arc::downgrade(&self.inner),
            epoch: state.epoch.clone(),
        };
        // 对象快照与SQL读取使用同一连接锁，不能在恢复之前取epoch、恢复之后签发记录。
        let result = {
            let mut scope = ClosedBorrow {
                connection: &mut state.connection,
                unusable: &self.inner.task_guard_unusable,
            };
            action(scope.connection())
        };
        if self.inner.task_guard_unusable.load(Ordering::Acquire) {
            // 只拒绝交付来源；action可能已写入，Unavailable不是“未保存”的回执。
            return Err(CredentialSourceError::Unavailable);
        }
        if !source.matches(self, &state) {
            return Err(CredentialSourceError::Changed);
        }
        Ok(result.map(|value| (value, source)))
    }

    /// 只接受原读取携带的来源；检查到SQL事务结束始终持同一conn锁，不换新来源补救。
    #[track_caller]
    pub(crate) fn commit_credential_source<T, E>(
        &self,
        source: &CredentialSource,
        action: impl FnOnce(&mut Connection) -> Result<T, E>,
    ) -> Result<Result<T, E>, CredentialSourceError> {
        let _reentrant_guard =
            super::ThreadLockGuard::enter(&self.inner).ok_or(CredentialSourceError::Unavailable)?;
        let mut state = self
            .inner
            .conn
            .lock()
            .map_err(|_| CredentialSourceError::Unavailable)?;
        if self.inner.task_guard_unusable.load(Ordering::Acquire) {
            return Err(CredentialSourceError::Unavailable);
        }
        if !source.matches(self, &state) {
            return Err(CredentialSourceError::Changed);
        }
        let result = {
            let mut scope = ClosedBorrow {
                connection: &mut state.connection,
                unusable: &self.inner.task_guard_unusable,
            };
            action(scope.connection())
        };
        // 同锁没有恢复可以插入；复核仍留在真实事务返回之后，而不是外层解锁后再检查。
        // 未结束事务由原锁内RAII隔离；保留action的原结果，不把已提交事实改为未写入。
        if !source.matches(self, &state) {
            return Err(CredentialSourceError::Changed);
        }
        Ok(result)
    }

    /// 恢复/云同步/旧账号导入的共同锁入口；不接自由Connection，不取得账号或配置锁。
    #[track_caller]
    pub(crate) fn with_account_change<T>(
        &self,
        action: impl FnOnce(&mut AccountChange<'_>) -> T,
    ) -> Option<T> {
        let _reentrant_guard = super::ThreadLockGuard::enter(&self.inner)?;
        let mut state = self.inner.conn.lock().ok()?;
        if self.inner.task_guard_unusable.load(Ordering::Acquire) {
            return None;
        }
        Some(action(&mut AccountChange {
            state: &mut state,
            unusable: &self.inner.task_guard_unusable,
        }))
    }
}

/// 必须在原连接锁释放前析构。只隔离、不隐式COMMIT/ROLLBACK，不改业务返回值。
struct ClosedBorrow<'a> {
    connection: &'a mut Connection,
    unusable: &'a AtomicBool,
}

impl ClosedBorrow<'_> {
    fn connection(&mut self) -> &mut Connection {
        &mut *self.connection
    }
}

impl Drop for ClosedBorrow<'_> {
    fn drop(&mut self) {
        if !self.connection.is_autocommit() {
            self.unusable.store(true, Ordering::Release);
        }
    }
}
