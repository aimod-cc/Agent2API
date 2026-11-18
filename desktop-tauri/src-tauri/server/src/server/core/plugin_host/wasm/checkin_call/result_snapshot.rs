//! 固定豆包真实执行结果到最初账号快照的封闭桥；不发送、不回查、不启动后台工作。
//! 绑定/消费在原Signal及原运行代次门内一次登记，持久CAS严格在这些门全部释放后执行。
//! 门消费后发生Runtime替换不能倒改已消费事实；CAS仅核对原账号/Db来源，不是Host/Db原子授权。
//! 官方保存另须原同空快照的封闭观测事实；NoReply只证明原守卫真实终态。
//! 该终态不证明曾发送、未发送、签到成功或可重发。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{atomic::Ordering, Arc};

use super::{CheckinCallRef, PersistenceStatus, RemoteStatus, ResultReceipt, Source, State};
use crate::server::core::account_store::credential_version::doubao_state::{
    SaveOutcome, SnapshotReceipt, StateError, StateFailure,
};

/// 状态只留在原Signal，不持账号材料/等待所有者/异常载荷，也不产生引用环。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum BindingState { #[default] Unbound, Bound, Consumed, Abandoned }

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::server::core::plugin_host::wasm) enum ResultSnapshotError {
    AccountMismatch,
    WrongProvider,
    Unavailable,
    NotEntered,
    AlreadyBound,
    ExecutionEnded,
    SourceExpired,
    SourceChanged,
    ReceiptMismatch,
    NoExecutionReceipt,
    AlreadyConsumed,
    UnsupportedConfirmation,
}

/// 按值持最初SnapshotReceipt与原私有Source Arc。无Clone/Debug/serde或JSON构造。
/// Drop只终止本桥资格，不清unknown记忆、不取消旧执行、不重签/重发。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct BoundResultSnapshot {
    source: Arc<Source>,
    snapshot: Option<SnapshotReceipt>,
    terminal: bool,
}

/// 真实Context入场后、插件执行前，由宿主装配同次最初快照；不能终态后补绑旧结果。
/// 唯一绑定还要求结果消费者尚未发放；装配顺序必须是bind -> take_result_consumer。
/// 调用者须先退出外部Signal/Host/Runtime/账号锁；本入口只持自己的短同步门，不查询账号。
/// account文字仅检查本家目标，唯一归属以原Source Arc和一次状态登记为准。
pub(in crate::server::core::plugin_host::wasm) fn bind_result_snapshot(
    call: &CheckinCallRef, snapshot: SnapshotReceipt,
) -> Result<BoundResultSnapshot, ResultSnapshotError> {
    if call.source.provider != "doubaowork" { return Err(ResultSnapshotError::WrongProvider); }
    if !snapshot.matches_doubao_account(&call.source.account) { return Err(ResultSnapshotError::AccountMismatch); }
    {
        // 输入snapshot由外层持有；下面任何锁拒绝/异常都先释放全部门，再析构输入。
        let mut state = call.source.signal.state.lock().map_err(|_| ResultSnapshotError::Unavailable)?;
        entered(&state)?;
        if state.result_snapshot != BindingState::Unbound { return Err(ResultSnapshotError::AlreadyBound); }
        if state.execution.is_some() { return Err(ResultSnapshotError::ExecutionEnded); }
        if !state.result.as_ref().is_some_and(|owner| owner.pending_for_snapshot(&call.source)) {
            return Err(ResultSnapshotError::Unavailable);
        }
        let host = call.source.host.upgrade().ok_or(ResultSnapshotError::Unavailable)?;
        let _life = host.lifecycle.try_lock().map_err(|_| ResultSnapshotError::Unavailable)?;
        let runtime = host.embedded.runtime.try_read().map_err(|_| ResultSnapshotError::Unavailable)?;
        runtime_current(call, &host, runtime.as_ref().map(Arc::as_ref), true)?;
        state.result_snapshot = BindingState::Bound;
    }
    Ok(BoundResultSnapshot { source: call.source.clone(), snapshot: Some(snapshot), terminal: false })
}

impl BoundResultSnapshot {
    pub(super) fn pending_reserved(&self) -> bool {
        self.snapshot.as_ref().is_none_or(SnapshotReceipt::pending_reserved)
    }

    pub(super) fn pending_today(&self) -> bool {
        self.snapshot.as_ref().is_none_or(SnapshotReceipt::pending_today)
    }

    pub(super) fn snapshot_for_daily(&self, call: &CheckinCallRef) -> Result<&SnapshotReceipt, ResultSnapshotError> {
        if self.terminal || !Arc::ptr_eq(&self.source, &call.source) {
            return Err(ResultSnapshotError::SourceChanged);
        }
        self.snapshot.as_ref().ok_or(ResultSnapshotError::AlreadyConsumed)
    }

    pub(super) fn snapshot_for_daily_mut(&mut self, call: &CheckinCallRef) -> Result<&mut SnapshotReceipt, ResultSnapshotError> {
        if self.terminal || !Arc::ptr_eq(&self.source, &call.source) { return Err(ResultSnapshotError::SourceChanged); }
        self.snapshot.as_mut().ok_or(ResultSnapshotError::AlreadyConsumed)
    }

    /// 唯一保存入口；真实ResultReceipt与本桥同时按值消费，没有无回执保存或替代call参数。
    /// old Source/代次、未交付的执行记录、重复消费或官方确认状态都不能进入账号CAS。
    /// 调用方不得持任何外层生命周期/Signal/账号/Db锁；本桥只保证自身门退出后才开始CAS。
    pub(in crate::server::core::plugin_host::wasm) fn save_unknown(
        mut self, result: ResultReceipt,
    ) -> SaveOutcome {
        let admitted = self.consume(&result);
        if let Err(reason) = admitted { return refused(reason); }
        // consume内的Signal/HostLifecycle/Runtime门已经全部退出；不把它们带入账号/Db锁。
        let Some(snapshot) = self.snapshot.take() else { return refused(ResultSnapshotError::AlreadyConsumed); };
        // 最初snapshot在外面持有：即使CAS unwind，其凭据来源也不在上述外层门内析构。
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            let candidate = snapshot.unknown_candidate();
            snapshot.save_unknown(candidate)
        }));
        // 原result只持有有界事实；直到CAS完整结束或异常槽形成才按值释放。
        drop(result);
        match attempt {
            Ok(outcome) => outcome,
            Err(payload) => {
                // 捕获载荷只在全部生命周期、账号和Db锁已退出之后处理；不能锁内格式化/析构。
                drop(payload);
                SaveOutcome::CommitUnknown {
                    error: StateError::State(StateFailure::Unavailable), storage_closed: false,
                }
            }
        }
    }

    pub(super) fn save_official_fact(
        mut self, result: ResultReceipt, fact: super::OfficialDailyFact,
    ) -> SaveOutcome {
        if self.snapshot.as_ref().is_none_or(|snapshot| !fact.matches_snapshot(snapshot)) {
            return refused(ResultSnapshotError::ReceiptMismatch);
        }
        if let Err(reason) = self.consume(&result) { return refused(reason); }
        let Some(snapshot) = self.snapshot.take() else { return refused(ResultSnapshotError::AlreadyConsumed); };
        let outcome = catch_unwind(AssertUnwindSafe(|| snapshot.save_official(fact)));
        drop(result);
        match outcome {
            Ok(outcome) => outcome,
            Err(payload) => {
                drop(payload);
                SaveOutcome::CommitUnknown { error: StateError::State(StateFailure::Unavailable), storage_closed: false }
            }
        }
    }

    fn consume(&mut self, result: &ResultReceipt) -> Result<(), ResultSnapshotError> {
        let call = CheckinCallRef { source: self.source.clone() };
        if !result.belongs_to(&call) { return Err(ResultSnapshotError::ReceiptMismatch); }
        if result.remote() != RemoteStatus::Unknown { return Err(ResultSnapshotError::UnsupportedConfirmation); }
        if result.persistence() != PersistenceStatus::NotEntered { return Err(ResultSnapshotError::AlreadyConsumed); }
        let mut state = self.source.signal.state.lock().map_err(|_| ResultSnapshotError::Unavailable)?;
        entered(&state)?;
        if self.terminal || state.result_snapshot != BindingState::Bound { return Err(ResultSnapshotError::AlreadyConsumed); }
        if state.execution != Some(result.execution()) { return Err(ResultSnapshotError::NoExecutionReceipt); }
        if !state.result.as_ref().is_some_and(|owner| owner.delivered_for_snapshot(&self.source, result.execution())) {
            return Err(ResultSnapshotError::NoExecutionReceipt);
        }
        let host = self.source.host.upgrade().ok_or(ResultSnapshotError::Unavailable)?;
        let _life = host.lifecycle.try_lock().map_err(|_| ResultSnapshotError::Unavailable)?;
        let runtime = host.embedded.runtime.try_read().map_err(|_| ResultSnapshotError::Unavailable)?;
        // 真实Timeout/NoReply是未知终态，期限到期不把它改成未发送；仍拒绝已关闭或替换的运行代次。
        runtime_current(&call, &host, runtime.as_ref().map(Arc::as_ref), false)?;
        state.result_snapshot = BindingState::Consumed;
        self.terminal = true;
        Ok(())
    }
}

impl Drop for BoundResultSnapshot {
    fn drop(&mut self) {
        if self.terminal { return; }
        {
            let mut state = self.source.signal.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.result_snapshot == BindingState::Bound { state.result_snapshot = BindingState::Abandoned; }
        }
        // snapshot字段在本函数返回后才析构；不在原Signal门内丢弃账号来源或自定义异常。
    }
}

fn entered(state: &State) -> Result<(), ResultSnapshotError> {
    if !state.bound || !state.entered || !state.context_entered { return Err(ResultSnapshotError::NotEntered); }
    // 等待方离开/执行终态不是来源失效；已登记的宿主或Context失效不能随后恢复成可消费。
    if matches!(state.reason, Some(super::EndReason::HostClosed | super::EndReason::RuntimeReplaced
        | super::EndReason::RuntimeCancelled | super::EndReason::Unavailable | super::EndReason::InvalidContext
        | super::EndReason::EnsureFailed | super::EndReason::PreparationFailed)) {
        return Err(ResultSnapshotError::SourceChanged);
    }
    Ok(())
}

fn runtime_current(
    call: &CheckinCallRef, host: &super::super::super::Host,
    runtime: Option<&super::super::runtime::Runtime>, binding: bool,
) -> Result<(), ResultSnapshotError> {
    if !std::ptr::eq(call.source.host.as_ptr(), host) || !host.ready()
        || host.closing.load(Ordering::Acquire) || super::super::super::runtime::admission_closed() {
        return Err(ResultSnapshotError::SourceChanged);
    }
    if host.generation.load(Ordering::Acquire) != call.source.generation
        || !runtime.is_some_and(|runtime| call.matches_runtime(runtime)) {
        return Err(ResultSnapshotError::SourceChanged);
    }
    if call.source.runtime_cancel.is_cancelled() { return Err(ResultSnapshotError::SourceChanged); }
    if binding && tokio::time::Instant::now() >= call.source.deadline { return Err(ResultSnapshotError::SourceExpired); }
    Ok(())
}

fn refused(reason: ResultSnapshotError) -> SaveOutcome {
    let failure = match reason {
        ResultSnapshotError::AccountMismatch | ResultSnapshotError::WrongProvider => StateFailure::WrongProvider,
        ResultSnapshotError::ReceiptMismatch => StateFailure::CandidateChanged,
        ResultSnapshotError::SourceChanged | ResultSnapshotError::SourceExpired => StateFailure::SourceChanged,
        ResultSnapshotError::UnsupportedConfirmation => StateFailure::OfficialConfirmationUnavailable,
        _ => StateFailure::Unavailable,
    };
    // 只指结果桥没有调用存储；绝不表示签到未发送、未发生或可以重试。
    SaveOutcome::NotEntered { error: StateError::State(failure), storage_closed: true }
}
