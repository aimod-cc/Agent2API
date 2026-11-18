//! 唯一Creating通道：原调用/记录门 -> 批准try -> 安装try -> 原账号最终事务 -> CAS。
//! 所有网络/请求材料只借用；FinalCheck及异常载荷必须离开整条锁链后处理。

use std::sync::Arc;

use tokio::time::Instant;

use super::{try_preparing, PreparingGuard};
use crate::server::core::plugin_host::wasm::{
    conversation::CallGate,
    managed_task::{
        account_snapshot::{FinalCallback, FinalCheck, FinalCheckError, FinalCheckState},
        approval_fence::FenceError,
        input::{Binding, Finalization},
        install_binding::BindingError,
        lifecycle::Record,
        network::{CreateWitness, PreparedCreate, Session},
        status::NotSentReason,
    },
};

/// 只由账号consume回调内的成功CAS产生；绑定确切请求，不可Clone/serde或重建。
pub(in crate::server::core::plugin_host::wasm::managed_task) struct CreatePermit {
    binding: Arc<Binding>,
    request: CreateWitness,
    deadline: Instant,
}

impl CreatePermit {
    pub(in crate::server::core::plugin_host::wasm::managed_task) fn binding(&self) -> &Binding {
        &self.binding
    }

    pub(in crate::server::core::plugin_host::wasm::managed_task) fn request(
        &self,
    ) -> &CreateWitness {
        &self.request
    }

    pub(in crate::server::core::plugin_host::wasm::managed_task) fn deadline(&self) -> Instant {
        self.deadline
    }
}

/// 构造点仅在本模块最终账号回调；实际借用原Guard和CallGate，不能带出同步锁链。
pub(in crate::server::core::plugin_host::wasm::managed_task) struct FinalConsume<'a, 'held, 'call> {
    preparing: &'a mut PreparingGuard<'held>,
    _gate: &'a CallGate<'call>,
}

impl FinalConsume<'_, '_, '_> {
    pub(in crate::server::core::plugin_host::wasm::managed_task) fn matches_record(
        &self,
        record: &Record,
    ) -> bool {
        std::ptr::eq(self.preparing.record.as_ref(), record)
    }

    fn commit(
        self,
        binding: Arc<Binding>,
        request: CreateWitness,
    ) -> Result<CreatePermit, NotSentReason> {
        let deadline = self.preparing.record.consume_submission(&self)?;
        self.preparing.state.create_deadline = Some(deadline);
        self.preparing.state.snapshot.submitted_now();
        Ok(CreatePermit {
            binding,
            request,
            deadline,
        })
    }
}

pub(in crate::server::core::plugin_host::wasm::managed_task) enum FinalizeFailure {
    NotSent(NotSentReason),
    // 已进入回调，但读取原CAS确认未消耗；整次worker仍终结，replay/准备机会不返还。
    EnteredWithoutSubmission(NotSentReason),
    SubmittedUnknown,
}

pub(in crate::server::core::plugin_host::wasm::managed_task) fn finalize(
    record: &Arc<Record>,
    finalization: Finalization,
    session: &Session,
    request: &PreparedCreate,
) -> Result<CreatePermit, FinalizeFailure> {
    // 只克隆轻身份，且先于任何最终门；请求/凭据/客户端由原worker持有。
    let witness = session
        .request_witness(request)
        .map_err(|reason| before_callback(record, reason))?;
    let binding = record.binding().clone();
    let call = record.task_call().clone();
    let checked: Result<FinalCheck<Result<CreatePermit, NotSentReason>>, NotSentReason> = call
        .with_gate(|gate| {
            let mut preparing = try_preparing(record, gate)?;
            if !preparing
                ._table
                .running
                .iter()
                .any(|running| Arc::ptr_eq(&running.record, record) && running.abort.is_some())
            {
                return Err(NotSentReason::RuntimeUnavailable);
            }
            if !finalization.matches(record.binding(), record.task_call(), gate) {
                return Err(NotSentReason::InvalidBinding);
            }
            session.check_prepared(record, gate, &preparing, request, &witness)?;
            // 上面的Preparing/出口核对同步原取消与Scope状态；之后不再读取token或重入外层锁。
            let _approval = finalization.try_approval().map_err(approval_error)?;
            finalization
                .install()
                .try_hold(gate, |installed| {
                    if !finalization.matches_install(installed) {
                        return Err(NotSentReason::InvalidBinding);
                    }
                    Ok(session.final_account().try_final_check(gate, || {
                        FinalConsume {
                            preparing: &mut preparing,
                            _gate: gate,
                        }
                        .commit(binding, witness)
                    }))
                })
                .map_err(install_error)?
        });
    // 包括deferred_unwind的自定义析构在内，都只在Signal/Host/Table/Record等已释放后运行。
    let checked = checked.map_err(|reason| before_callback(record, reason))?;
    drop(finalization);
    match checked.into_state() {
        FinalCheckState::NotEntered(error) => Err(before_callback(record, account_error(error))),
        FinalCheckState::Entered {
            result: FinalCallback::Returned(Ok(permit)),
            storage_closed: true,
        } if record.submitted() => Ok(permit),
        FinalCheckState::Entered { result, .. } => {
            let reason = match result {
                FinalCallback::Returned(Err(reason)) => reason,
                FinalCallback::Returned(Ok(permit)) => {
                    drop(permit);
                    NotSentReason::RuntimeUnavailable
                }
                FinalCallback::Unwound => NotSentReason::RuntimeUnavailable,
            };
            if record.submitted() {
                Err(FinalizeFailure::SubmittedUnknown)
            } else {
                Err(FinalizeFailure::EnteredWithoutSubmission(
                    record.preparing_error().unwrap_or(reason),
                ))
            }
        }
    }
}

fn before_callback(record: &Record, reason: NotSentReason) -> FinalizeFailure {
    if record.submitted() {
        FinalizeFailure::SubmittedUnknown
    } else {
        FinalizeFailure::NotSent(record.preparing_error().unwrap_or(reason))
    }
}

fn approval_error(error: FenceError) -> NotSentReason {
    match error {
        FenceError::Busy | FenceError::Unavailable => NotSentReason::RuntimeUnavailable,
        FenceError::ConfirmationRequired | FenceError::Stale => NotSentReason::AdmissionClosed,
    }
}

fn install_error(error: BindingError) -> NotSentReason {
    match error {
        BindingError::DifferentSource => NotSentReason::InvalidBinding,
        BindingError::Unavailable => NotSentReason::RuntimeUnavailable,
    }
}

fn account_error(error: FinalCheckError) -> NotSentReason {
    match error {
        FinalCheckError::CallEnded => NotSentReason::CallEnded,
        FinalCheckError::InvalidBinding | FinalCheckError::AccountChanged => {
            NotSentReason::InvalidBinding
        }
        FinalCheckError::InvalidMaterial | FinalCheckError::TooLarge => {
            NotSentReason::RequestUnavailable
        }
        FinalCheckError::Busy
        | FinalCheckError::Poisoned
        | FinalCheckError::KeyUnavailable
        | FinalCheckError::StorageUnavailable
        | FinalCheckError::StorageNotClosed
        | FinalCheckError::Interrupted => NotSentReason::RuntimeUnavailable,
    }
}
