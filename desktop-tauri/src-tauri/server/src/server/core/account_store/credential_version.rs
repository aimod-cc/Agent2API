//! 固定只读行来源共用的宿主私有出生版本；不含凭据写入或发送许可。
//! 普通SQL维护版本；本进程恢复/同步/旧导入单向撤销来源。外部写入不在此合同内。

pub(crate) mod doubao_state;

use std::{any::Any, panic::{catch_unwind, AssertUnwindSafe}};

use serde_json::{Map, Value};
use zeroize::Zeroizing;

use super::{sql, AccountStore};
use crate::server::core::crypto::{self, SecretScope};
use crate::server::db::{CredentialSource, CredentialSourceError, TaskTransaction, TaskTransactionError};

pub(crate) use sql::credential_version::{Conflict, Error, StorageFailure};

/// 只存在于加密账号行的顶层；插件secret或公开字段中的同名项不是版本凭据。
pub(super) const PRIVATE_FIELD: &str = "_hostCredentialVersion";
pub(super) const EPOCH_KEY: &str = "accountCredentialEpoch";

pub(super) fn private_field(key: &str) -> bool {
    key == PRIVATE_FIELD
}

pub(super) fn strip_private(fields: &mut Map<String, Value>) {
    fields.remove(PRIVATE_FIELD);
}

/// 只读来源的固定集合，不包含CodeArts凭据写入资格，也不接任意provider JSON。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowProvider {
    MonkeyCode,
    MonkeyCodeIntl,
    DoubaoWork,
    CatPaw,
}

impl RowProvider {
    pub(super) fn parse(provider: &str) -> Option<Self> {
        match provider {
            "monkeycode" => Some(Self::MonkeyCode),
            "monkeycode-intl" => Some(Self::MonkeyCodeIntl),
            "doubaowork" => Some(Self::DoubaoWork),
            "catpaw" => Some(Self::CatPaw),
            _ => None,
        }
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::MonkeyCode => "monkeycode",
            Self::MonkeyCodeIntl => "monkeycode-intl",
            Self::DoubaoWork => "doubaowork",
            Self::CatPaw => "catpaw",
        }
    }
}

/// 仅剥离固定三家旧导入记录的顶层宿主标记；不签发来源，不改任何业务字段。
pub(crate) fn strip_imported_row_version(record: &mut super::state::StoredAccount) {
    if sql::credential_version::maintained_provider(&record.provider()) {
        record.remove(PRIVATE_FIELD);
    }
}

/// 私有行来源始终持原AccountStore和同次Db来源；无Clone/Debug/serde或拆分配对入口。
#[must_use]
pub(crate) struct RowSource {
    store: AccountStore,
    inner: sql::credential_version::RowStamp,
    source: CredentialSource,
}

pub(crate) enum RowFinalError {
    AccountBusy,
    AccountPoisoned,
    StoreUnavailable,
    Transaction(TaskTransactionError),
    Read(Error),
    Interrupted,
}

pub(crate) enum RowFinalCallback<R> {
    Returned(R),
    Unwound,
}

/// Entered不能被存储收尾失败改成未进入；消费状态仍由原消费者的唯一CAS裁决。
pub(crate) enum RowFinalState<R> {
    NotEntered(RowFinalError),
    Entered { result: RowFinalCallback<R>, storage_closed: bool },
}

/// 只在全部外层锁释放后拆开或丢弃；异常载荷不格式化，也不视为批准。
#[must_use]
pub(crate) struct RowFinalCheck<R> {
    state: RowFinalState<R>,
    deferred_unwind: [Option<Box<dyn Any + Send>>; 2],
}

impl<R> RowFinalCheck<R> {
    fn not_entered(error: RowFinalError) -> Self {
        Self { state: RowFinalState::NotEntered(error), deferred_unwind: [None, None] }
    }

    pub(crate) fn into_state(self) -> RowFinalState<R> {
        let Self { state, deferred_unwind } = self;
        drop(deferred_unwind);
        state
    }
}

/// 字段仅RowSource终检能填写；不导出store/Db/账号，桥不能换配另一份来源。
pub(super) struct RowFinalRead<'a> {
    owner: &'a RowSource,
    key: Zeroizing<[u8; 32]>,
}

impl RowFinalRead<'_> {
    pub(super) fn with_original_store<R>(
        &self,
        read: impl FnOnce(&AccountStore, &CredentialSource, &sql::credential_version::RowStamp, &[u8; 32]) -> R,
    ) -> R {
        read(&self.owner.store, &self.owner.source, &self.owner.inner, &self.key)
    }
}

impl RowSource {
    /// 同步终检只借用原来源；读取和比较成功后才在原账号/连接/IMMEDIATE门内消费。
    /// 不重新prepare、不回退无来源事务；闭包不可联网、await、spawn或反向取锁。
    pub(crate) fn try_final_check<R>(&self, consume: impl FnOnce() -> R) -> RowFinalCheck<R> {
        let key = match cached_key() {
            Ok(key) => key,
            Err(error) => return RowFinalCheck::not_entered(RowFinalError::Read(error)),
        };
        let read = RowFinalRead { owner: self, key };
        let mut entered = false;
        let mut returned = None;
        // 未进入时，调用闭包及其捕获只在所有存储锁退出后析构，不把它移进Db动作。
        let mut consume = Some(consume);
        // R在Db动作外持有；ROLLBACK或设置恢复unwind也不能丢失已返回结果。
        let mut action_unwind = None;
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            AccountStore::try_row_source_final(&read, &mut entered, &mut || {
                if let Some(consume) = consume.take() {
                    returned = Some(consume());
                }
            }, &mut action_unwind)
        }));
        let (failure, storage_closed, deferred_unwind) = match attempt {
            Err(payload) => (Some(RowFinalError::Interrupted), false, Some(payload)),
            Ok(Err(error)) => (Some(error), true, None),
            Ok(Ok(TaskTransaction::NotEntered(error))) => {
                let closed = !matches!(error, TaskTransactionError::Close);
                (Some(RowFinalError::Transaction(error)), closed, None)
            }
            Ok(Ok(TaskTransaction::Completed { outcome, storage_closed })) => {
                let (failure, payload) = match outcome {
                    Err(payload) => (Some(RowFinalError::Interrupted), Some(payload)),
                    Ok(Err(error)) => (Some(RowFinalError::Read(error)), None),
                    Ok(Ok(())) => (None, None),
                };
                (failure, storage_closed, payload)
            }
        };
        let state = if entered {
            RowFinalState::Entered {
                result: match returned {
                    Some(result) => RowFinalCallback::Returned(result),
                    None => RowFinalCallback::Unwound,
                },
                storage_closed,
            }
        } else {
            RowFinalState::NotEntered(if storage_closed {
                if action_unwind.is_some() { Some(RowFinalError::Interrupted) } else { failure }
                    .unwrap_or(RowFinalError::Interrupted)
            } else {
                RowFinalError::Transaction(TaskTransactionError::Close)
            })
        };
        RowFinalCheck { state, deferred_unwind: [action_unwind, deferred_unwind] }
    }

    pub(crate) fn provider(&self) -> RowProvider {
        self.inner.provider()
    }
    pub(crate) fn account(&self) -> &str {
        self.inner.account()
    }
    pub(crate) fn secret(&self) -> &Value {
        self.inner.secret()
    }
    pub(crate) fn identity(&self) -> &str {
        self.inner.identity()
    }
    pub(crate) fn user_agent(&self) -> Option<&str> {
        self.inner.user_agent()
    }

    pub(crate) fn original_doubao_proxy(&self) -> Result<Value, Error> {
        if self.provider() != RowProvider::DoubaoWork { return Err(Error::UnsupportedProvider); }
        let key = cached_key()?;
        let _guard = self.store.guard();
        let db = self.store.database().ok_or(Error::Storage(StorageFailure::Unavailable))?;
        db.commit_credential_source(&self.source, |conn| {
            sql::credential_version::row_egress::doubao_proxy(conn, &key, &self.inner)
        }).map_err(source_error)?
    }
}

impl AccountStore {
    /// 只读来源的唯一构造：原账号锁/Db连接锁内读取或建立版本，提交确认后才交付。
    /// 不创建账号，不返回bool批准；任务/签到消费者仍未接入。
    pub(crate) fn prepare_row_source(
        &self,
        provider: RowProvider,
        id: &str,
    ) -> Result<RowSource, Error> {
        self.prepare_row_source_inner(provider, id, false)
    }

    pub(crate) fn prepare_maintenance_row_source(&self, provider: RowProvider, id: &str) -> Result<RowSource, Error> {
        if provider != RowProvider::DoubaoWork { return Err(Error::UnsupportedProvider); }
        self.prepare_row_source_inner(provider, id, true)
    }

    fn prepare_row_source_inner(&self, provider: RowProvider, id: &str, account_maintenance: bool) -> Result<RowSource, Error> {
        let key = cached_key()?;
        let _guard = self.guard();
        let db = self
            .database()
            .ok_or(Error::Storage(StorageFailure::Unavailable))?;
        let (inner, source) = db
            .prepare_credential_source(|conn| {
                sql::credential_version::prepare_row(conn, &key, provider, id, account_maintenance)
            })
            .map_err(source_error)??;
        Ok(RowSource {
            store: self.clone(),
            inner,
            source,
        })
    }
}

fn source_error(error: CredentialSourceError) -> Error {
    match error {
        CredentialSourceError::Unavailable => Error::Storage(StorageFailure::Unavailable),
        CredentialSourceError::Changed => Error::Conflict(Conflict::HostSourceChanged),
    }
}

fn cached_key() -> Result<Zeroizing<[u8; 32]>, Error> {
    crypto::try_cached_key(SecretScope::Accounts)
        .map(Zeroizing::new)
        .ok_or(Error::Storage(StorageFailure::KeyUnavailable))
}
