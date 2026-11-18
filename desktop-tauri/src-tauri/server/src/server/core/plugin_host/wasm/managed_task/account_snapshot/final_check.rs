//! 只复核快照封闭持有的原RowSource；调用方已经持同源CallGate和原Record消费门。
//! 原账号try锁 -> 原Dbtry锁/私有代次 -> IMMEDIATE完整投影 -> 同步consume -> ROLLBACK。
//! 只保证原来源在消费瞬间仍有效，不保证网络原子、上游仅执行一次或外部库回滚可识别。

use tokio::time::Instant;

use super::{
    bounded_text, source_matches_call, AccountSnapshot, FinalAccount, SnapshotMaterial,
    MAX_ACCOUNT_BYTES,
};
use crate::server::core::account_store::credential_version::{
    Error, RowFinalCallback, RowFinalCheck, RowFinalError, RowFinalState, RowSource, StorageFailure,
};
use crate::server::core::plugin_host::wasm::conversation::{CallGate, TaskCallRef};
use crate::server::db::TaskTransactionError;

#[derive(Clone, Copy)]
pub(in crate::server::core::plugin_host::wasm::managed_task) enum FinalCheckError {
    CallEnded,
    InvalidBinding,
    AccountChanged,
    InvalidMaterial,
    TooLarge,
    Busy,
    Poisoned,
    KeyUnavailable,
    StorageUnavailable,
    StorageNotClosed,
    Interrupted,
}

impl FinalCheckError {
    pub(in crate::server::core::plugin_host::wasm::managed_task) fn message(self) -> &'static str {
        match self {
            Self::CallEnded => "调用期限已结束，未进入最终消费回调",
            Self::InvalidBinding => "最终账号复核不是同源调用，未进入消费回调",
            Self::AccountChanged => "任务账号已不可用、出生版本或授权内容已变化，未进入消费回调",
            Self::InvalidMaterial => "任务账号材料不符合最终复核规则，未进入消费回调",
            Self::TooLarge => "任务账号材料超过最终复核限制，未进入消费回调",
            Self::Busy => "最终账号复核通道忙，未等待或进入消费回调",
            Self::Poisoned => "最终账号复核锁状态异常，未进入消费回调",
            Self::KeyUnavailable => "账号解密缓存尚未就绪，未加载密钥或进入消费回调",
            Self::StorageUnavailable => "最终账号复核存储不可用，未进入消费回调",
            Self::StorageNotClosed => "最终复核未正常收尾，消费回调未进入且不授予执行资格",
            Self::Interrupted => "最终账号复核被中断，消费回调未进入",
        }
    }
}

/// Returned保留回调原结果，包括业务拒绝；Unwound不等于未消费，不能返还机会。
pub(in crate::server::core::plugin_host::wasm::managed_task) enum FinalCallback<R> {
    Returned(R),
    Unwound,
}

/// Entered的所有分支都保留“回调已经进入”事实，不能映射成NotSent或返还机会。
/// storage_closed只回答存储是否正常收尾，不撤销回调、CAS或它的真实返回值。
#[must_use]
pub(in crate::server::core::plugin_host::wasm::managed_task) enum FinalCheckState<R> {
    NotEntered(FinalCheckError),
    Entered {
        result: FinalCallback<R>,
        storage_closed: bool,
    },
}

/// 无Clone/Debug/serde。完整RowFinalCheck带出Signal/Host/Table/Scope/Record/安装锁。
/// 两份延后异常及回调返回值均保持原类型，不在外层锁内拆开或释放。
#[must_use]
pub(in crate::server::core::plugin_host::wasm::managed_task) struct FinalCheck<R> {
    inner: Result<RowFinalCheck<R>, FinalCheckError>,
}

impl<R> FinalCheck<R> {
    fn not_entered(error: FinalCheckError) -> Self {
        Self { inner: Err(error) }
    }

    pub(in crate::server::core::plugin_host::wasm::managed_task) fn into_state(
        self,
    ) -> FinalCheckState<R> {
        // 仅registry既有锁外点可拆；RowFinalCheck在此释放原两份异常载荷。
        match self.inner {
            Err(error) => FinalCheckState::NotEntered(error),
            Ok(checked) => match checked.into_state() {
                RowFinalState::NotEntered(error) => FinalCheckState::NotEntered(row_error(error)),
                RowFinalState::Entered {
                    result,
                    storage_closed,
                } => FinalCheckState::Entered {
                    result: match result {
                        RowFinalCallback::Returned(value) => FinalCallback::Returned(value),
                        RowFinalCallback::Unwound => FinalCallback::Unwound,
                    },
                    storage_closed,
                },
            },
        }
    }
}

impl AccountSnapshot {
    /// 只借用首次真实来源；点时重检失效后没有材料，不能取得第二份来源来继续本次操作。
    pub(in crate::server::core::plugin_host::wasm::managed_task) fn try_final_check<R>(
        &self,
        gate: &CallGate<'_>,
        consume: impl FnOnce() -> R,
    ) -> FinalCheck<R> {
        let Some(previous) = self.material.as_ref() else {
            return FinalCheck::not_entered(FinalCheckError::AccountChanged);
        };
        check(&previous.source, &self.call, gate, consume)
    }
}

impl SnapshotMaterial {
    /// consume只做已持Record的同步唯一CAS/状态赋值，不查token/注册表、不再取锁。
    pub(in crate::server::core::plugin_host::wasm::managed_task) fn try_final_check<R>(
        &self,
        gate: &CallGate<'_>,
        consume: impl FnOnce() -> R,
    ) -> FinalCheck<R> {
        check(&self.material.source, &self.call, gate, consume)
    }
}

impl FinalAccount {
    pub(in crate::server::core::plugin_host::wasm::managed_task) fn try_final_check<R>(
        &self,
        gate: &CallGate<'_>,
        consume: impl FnOnce() -> R,
    ) -> FinalCheck<R> {
        check(&self.source, &self.call, gate, consume)
    }
}

fn check<R>(
    source: &RowSource,
    call: &TaskCallRef,
    gate: &CallGate<'_>,
    consume: impl FnOnce() -> R,
) -> FinalCheck<R> {
    if !gate.matches(call)
        || !source_matches_call(source, call)
        || !bounded_text(call.identity().account(), MAX_ACCOUNT_BYTES)
    {
        return FinalCheck::not_entered(FinalCheckError::InvalidBinding);
    }
    if Instant::now() >= call.call_deadline() {
        return FinalCheck::not_entered(FinalCheckError::CallEnded);
    }
    // 不在RowSource里面再包装可失败前置检查，避免把已进入事实折回NotEntered。
    // 原Record的唯一CAS自己核对期限/结束意图；此处不查取消token，不做网络或回调装配。
    FinalCheck {
        inner: Ok(source.try_final_check(consume)),
    }
}

fn row_error(error: RowFinalError) -> FinalCheckError {
    match error {
        RowFinalError::AccountBusy => FinalCheckError::Busy,
        RowFinalError::AccountPoisoned => FinalCheckError::Poisoned,
        RowFinalError::StoreUnavailable => FinalCheckError::StorageUnavailable,
        RowFinalError::Transaction(error) => transaction_error(error),
        RowFinalError::Read(error) => match error {
            Error::InvalidInput | Error::UnsupportedProvider => FinalCheckError::InvalidBinding,
            Error::InvalidRecord => FinalCheckError::InvalidMaterial,
            Error::LimitExceeded => FinalCheckError::TooLarge,
            Error::VersionExhausted | Error::Conflict(_) => FinalCheckError::AccountChanged,
            Error::Storage(StorageFailure::Busy) => FinalCheckError::Busy,
            Error::Storage(StorageFailure::KeyUnavailable) => FinalCheckError::KeyUnavailable,
            Error::Storage(StorageFailure::RollbackUnconfirmed) => {
                FinalCheckError::StorageNotClosed
            }
            Error::Storage(_) => FinalCheckError::StorageUnavailable,
        },
        RowFinalError::Interrupted => FinalCheckError::Interrupted,
    }
}

fn transaction_error(error: TaskTransactionError) -> FinalCheckError {
    match error {
        TaskTransactionError::Busy => FinalCheckError::Busy,
        TaskTransactionError::Poisoned => FinalCheckError::Poisoned,
        TaskTransactionError::Close => FinalCheckError::StorageNotClosed,
        TaskTransactionError::Unavailable
        | TaskTransactionError::NestedTransaction
        | TaskTransactionError::BusyTimeout
        | TaskTransactionError::Begin => FinalCheckError::StorageUnavailable,
    }
}
