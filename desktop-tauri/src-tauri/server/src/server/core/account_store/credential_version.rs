//! 固定三家共用宿主私有出生版本；CodeArts独占凭据写入，两家只读来源无发送权限。
//! 普通SQL维护版本；本进程恢复/同步/旧导入单向撤销来源。外部写入不在此合同内。

// 封闭开发基础：不导出发送许可或首次纳管入口，也不改变现有 RowSource 接口。
mod codearts_slot;
pub(crate) use codearts_slot::{ReadyRead, PendingSlot, PendingBinding, SlotOutcome, SlotFailure, SlotSettlement};
pub(crate) use codearts_slot::{PendingFinalCheck, PendingFinalState, PendingFinalCallback, PendingCommitFact, PendingFinalError};
pub(crate) use codearts_slot::{PendingCompletion, PendingCompletionCheck, PendingCompletionState, PendingCompletionCallback};
pub(in crate::server::core::account_store) use codearts_slot::{PendingFinalRead, PendingCompletionRead};
mod cline_free_refresh;
pub(crate) use cline_free_refresh::{FreeRefreshOrigin, FreeRefreshPending, FreeRefreshFinal};
mod cline_free_input;
mod cline_free_enrollment;
pub(crate) use cline_free_input::{FreeEnrollmentInput, FreeInputError};
pub(crate) use cline_free_enrollment::{FreeEnrollmentOrigin, FreeEnrollmentCommitOutcome, FreeEnrollmentError};
mod codearts_enrollment_input;
mod codearts_enrollment;
pub(crate) use codearts_enrollment_input::EnrollmentInput;
pub(crate) use codearts_enrollment::{EnrollmentOrigin, EnrollmentCommitOutcome, EnrollmentError as EnrollmentStorageError};
pub(crate) mod doubao_state;

use std::{any::Any, panic::{catch_unwind, AssertUnwindSafe}};

use serde_json::{Map, Value};
use zeroize::Zeroizing;

use super::{sql, AccountStore};
use crate::server::core::crypto::{self, SecretScope};
use crate::server::db::{CredentialSource, CredentialSourceError, TaskTransaction, TaskTransactionError};

pub(crate) use sql::credential_version::{CommitReceipt, Conflict, Error, StorageFailure};

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
    if sql::credential_version::maintained_provider(&record.provider()) || record.provider() == "cline-free" {
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

/// 字段只能由真实事务返回；不实现Clone/Debug/serde，不按路径或JSON恢复。
/// 持有原AccountStore的克隆，提交入口不接受另一份store来替换它。
#[must_use]
pub(crate) struct ReadStamp {
    store: AccountStore,
    inner: sql::credential_version::Stamp,
    source: CredentialSource,
}

/// 仅是经过有界字段校验的候选改动，不代表身份、执行批准或上游成功。
/// 接口只允许固定CodeArts凭据字段和必要公开标量，不接受任意整条账号记录。
pub(crate) struct CredentialChange {
    inner: sql::credential_version::Change,
}

impl CredentialChange {
    pub(crate) fn new(credentials: &Value, public: &Value) -> Result<Self, Error> {
        sql::credential_version::Change::new(credentials, public).map(|inner| Self { inner })
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

    /// 显式建立尚未加入本合同的旧账号版本，或读取已有有效版本；不创建账号。
    /// 恢复出的有效旧版本若与当前kv不一致，仅在这次真实读取事务内重建目标行。
    /// 缺版本不是第0代：只有事务已写入并COMMIT后才返回ReadStamp。
    /// key仅取已选缓存，避免持账号/Db锁进入配置、文件或密钥加载路径。
    pub(crate) fn prepare_codearts_credential_version(
        &self,
        provider: &str,
        id: &str,
    ) -> Result<ReadStamp, Error> {
        sql::credential_version::check_target(provider, id)?;
        let key = cached_key()?;
        let _guard = self.guard();
        let db = self
            .database()
            .ok_or(Error::Storage(StorageFailure::Unavailable))?;
        // 账号锁 -> 实际Db连接锁 -> SQL事务；来源签发与读取提交处于同一个连接借用期。
        let (inner, source) = db
            .prepare_credential_source(|conn| {
                sql::credential_version::prepare(conn, &key, provider, id)
            })
            .map_err(source_error)??;
        Ok(ReadStamp {
            store: self.clone(),
            inner,
            source,
        })
    }
}

impl ReadStamp {
    pub(crate) fn provider(&self) -> &'static str {
        "codearts"
    }

    pub(crate) fn account(&self) -> &str {
        self.inner.account()
    }

    /// 只向宿主调用方借出这一次的授权材料；不会输出版本标识、代理或其他账号。
    /// 旧原生行使用固定CodeArts字段投影，已有pluginSecret则保留其全部未知字段。
    pub(crate) fn secret(&self) -> &Value {
        self.inner.secret()
    }

    /// 同步提交：只有返回Ok才确认COMMIT；没有返回不等于没有保存。
    /// 若上层放入spawn_blocking，必须自行持有任务直到真正结束，超时不能推断未保存。
    pub(crate) fn commit(self, change: CredentialChange) -> Result<CommitReceipt, Error> {
        self.write(change, false)
    }

    /// 独立的可信重新授权入口；同一账号保留born，即使材料未变也强制推进revision。
    /// 本批没有登录/插件JSON调用者，也没有新增账号或按ID重建许可的旁路。
    pub(crate) fn reauthorize(self, change: CredentialChange) -> Result<CommitReceipt, Error> {
        self.write(change, true)
    }

    fn write(self, change: CredentialChange, reauthorize: bool) -> Result<CommitReceipt, Error> {
        let key = cached_key()?;
        let _guard = self.store.guard();
        let db = self
            .store
            .database()
            .ok_or(Error::Storage(StorageFailure::Unavailable))?;
        db.commit_credential_source(&self.source, |conn| {
            sql::credential_version::commit(conn, &key, &self.inner, change.inner, reauthorize)
        })
        .map_err(source_error)?
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
