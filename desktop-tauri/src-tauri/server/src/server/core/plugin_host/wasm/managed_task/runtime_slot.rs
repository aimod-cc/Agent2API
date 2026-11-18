//! 每个真实Shared仅一份惰性运行代次登记；初始化失败或撤销后不能重新建立。
//! 同源批准/账号绑定与登记仅占一处调用门；封闭准入结果不spawn，执行入口仍未接通。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};

use super::super::conversation::{CallGate, TaskCallRef};
use super::{
    account_snapshot::SnapshotMaterial,
    executor,
    input::{GatedInput, PreparedInput},
    lifecycle::{Invocation, Observation},
    network::ObservationFailure,
    policy::CreateBody,
    registry::Executor,
    status::{NotSentReason, Snapshot},
};

#[derive(Default)]
pub(in crate::server::core::plugin_host::wasm) struct ExecutorSlot {
    closed: AtomicBool,
    value: OnceLock<Result<Arc<Executor>, NotSentReason>>,
    initialization_claimed: AtomicBool,
}

impl ExecutorSlot {
    /// 只接受仍被真实CallGate借用的Shared，不接自由运行身份或重建开关。
    fn get(&self, gate: &CallGate<'_>) -> Result<&Arc<Executor>, NotSentReason> {
        if !std::ptr::eq(self, &gate.shared().managed_tasks) {
            return Err(NotSentReason::InvalidBinding);
        }
        if self.closed.load(Ordering::Acquire) || gate.shared().cancel.is_cancelled() {
            self.revoke();
            return Err(NotSentReason::AdmissionClosed);
        }
        if self.value.get().is_none() {
            if self
                .initialization_claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                // 正常并发初始化由Host生命周期锁串行；缺值但已领取表示中断，不能重建。
                self.revoke();
                return Err(NotSentReason::AdmissionClosed);
            }
            let initialized = if self.closed.load(Ordering::Acquire) {
                Err(NotSentReason::AdmissionClosed)
            } else {
                Executor::from_call_gate(gate).map(Arc::new)
            };
            if self.value.set(initialized).is_err() {
                self.revoke();
                return Err(NotSentReason::AdmissionClosed);
            }
        }
        let Some(initialized) = self.value.get() else {
            self.revoke();
            return Err(NotSentReason::AdmissionClosed);
        };
        match initialized {
            Ok(executor) if !self.closed.load(Ordering::Acquire) && executor.active_for(gate) => {
                Ok(executor)
            }
            Ok(executor) => {
                executor.revoke();
                self.closed.store(true, Ordering::Release);
                Err(NotSentReason::AdmissionClosed)
            }
            Err(reason) => Err(*reason),
        }
    }

    pub(in crate::server::core::plugin_host::wasm) fn ensure(
        &self,
        gate: &CallGate<'_>,
    ) -> Result<(), NotSentReason> {
        self.get(gate).map(|_| ())
    }

    pub(in crate::server::core::plugin_host::wasm) fn close_for_shutdown(
        &self,
        reason: super::registry::shutdown::ShutdownReason,
        deadline: tokio::time::Instant,
    ) {
        // 主动关闭先标记一次原因，不能让Shared取消把HostShutdown变成RuntimeReplaced。
        if let Some(Ok(executor)) = self.value.get() {
            executor.close_for_shutdown(reason, deadline);
        }
        self.closed.store(true, Ordering::Release);
        // 并行初始化也必须看到闭锁；已经发布的scope仍沿用首因和最早截止。
        if let Some(Ok(executor)) = self.value.get() {
            executor.close_for_shutdown(reason, deadline);
        }
    }

    pub(in crate::server::core::plugin_host::wasm) fn revoke(&self) {
        // 有Scope时先通过同一ScopeGate撤销；再封闭惰性槽，先Creating或先关闭有明确裁决。
        if let Some(Ok(executor)) = self.value.get() {
            executor.revoke();
        }
        self.closed.store(true, Ordering::Release);
        // 并行初始化可能在第一次读取后发布；闭锁后再次撤销它，不能漏掉新Scope。
        if let Some(Ok(executor)) = self.value.get() {
            executor.revoke();
        }
    }
}

/// 只有本模块能拆GatedInput；门的借用不能由调用方替换或跨await。
pub(super) struct RegistrationUse(());

/// 字段私有，无Clone/serde；输入与原Invocation只有这一个所有者，不交付可换配二元组。
/// 登记不代表执行批准：原worker提交前仍重验批准/安装/账号并进行唯一CAS。
#[must_use]
pub(super) struct Admitted {
    // 未启动时先销毁输入，再由Invocation::drop登记原调用结束意图。
    input: PreparedInput,
    invocation: Invocation,
}

/// 启动后仍封闭持有同一个Invocation；不能把等待结果与责任所有者拆成临时二元组。
#[must_use]
pub(super) struct Started {
    state: StartState,
    invocation: Invocation,
}

enum StartState {
    Waiting(executor::CreationWait),
    Delivered(executor::Delivery),
    NotSent {
        reason: NotSentReason,
        replay_forbidden: bool,
    },
    WaitingConsumed,
}

pub(super) enum StartView<'a> {
    NotSent {
        reason: NotSentReason,
        replay_forbidden: bool,
    },
    Delivery(&'a executor::Delivery),
    WaitingConsumed,
}

impl Admitted {
    /// 只在所有外层锁释放后按值启动；原Invocation直接移入Started，不在函数返回时drop。
    /// 输入不可取回或重试；登记阶段不调用这里、不spawn。
    pub(super) fn start(self) -> Started {
        let Self { input, invocation } = self;
        let state = match executor::start(&invocation, input) {
            executor::StartResult::Waiting(wait) => StartState::Waiting(wait),
            executor::StartResult::NotSent {
                reason,
                replay_forbidden,
            } => StartState::NotSent {
                reason,
                replay_forbidden,
            },
        };
        Started { state, invocation }
    }
}

impl Started {
    /// 结果和真实创建收据留在本所有者内，只借出只读视图；借用存在期间Invocation不能销毁。
    /// 等待取消只消耗原接收端并置结束意图，不新建等待端/worker或返还创建机会。
    pub(super) fn wait(&mut self) -> impl std::future::Future<Output = StartView<'_>> + '_ {
        // Future之外取走原等待端；即使从未poll就丢弃，也由CreationWait::drop置结束意图。
        let pending = if matches!(&self.state, StartState::Waiting(_)) {
            match std::mem::replace(&mut self.state, StartState::WaitingConsumed) {
                StartState::Waiting(wait) => Some(wait),
                _ => None,
            }
        } else {
            None
        };
        async move {
            if let Some(wait) = pending {
                self.state = StartState::Delivered(wait.wait().await);
            }
            match &self.state {
                StartState::NotSent {
                    reason,
                    replay_forbidden,
                } => StartView::NotSent {
                    reason: *reason,
                    replay_forbidden: *replay_forbidden,
                },
                StartState::Delivered(delivery) => StartView::Delivery(delivery),
                StartState::WaitingConsumed => {
                    self.invocation
                        .close(super::status::EndReason::ReceiverDropped);
                    StartView::WaitingConsumed
                }
                StartState::Waiting(_) => StartView::WaitingConsumed,
            }
        }
    }

    pub(super) fn belongs_to(&self, call: &TaskCallRef) -> bool {
        self.invocation.record().task_call().same_source(call)
    }

    pub(super) fn has_receipt(&self, call: &TaskCallRef) -> bool {
        self.belongs_to(call)
            && matches!(
                &self.state,
                StartState::Delivered(executor::Delivery::Created(_))
            )
    }

    /// 只能操作保留在Started内的本次真实收据；不接自由Binding、UUID或另一Invocation。
    pub(super) fn observe(&self, call: &TaskCallRef) -> Result<Observation, ObservationFailure> {
        if !self.belongs_to(call) {
            return Err(ObservationFailure::Unavailable);
        }
        let StartState::Delivered(executor::Delivery::Created(receipt)) = &self.state else {
            return Err(ObservationFailure::Unavailable);
        };
        receipt.observe(self.invocation.record().binding())
    }

    /// 只对原真实收据置结束意图；返回true不是停止受理，更不是执行环境已销毁。
    pub(super) fn close_receipt(&self, call: &TaskCallRef) -> bool {
        if !self.belongs_to(call) {
            return false;
        }
        let StartState::Delivered(executor::Delivery::Created(receipt)) = &self.state else {
            return false;
        };
        receipt.close(self.invocation.record().binding())
    }

    /// Context取消只写原任务意图，不abort/重建worker，不同步联网。
    pub(super) fn cancel(&self) {
        self.invocation
            .close(super::status::EndReason::CallerDropped);
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        self.invocation.snapshot()
    }
}

/// 无生产调用者：只按值消费已从原RowSource准备并重检的SnapshotMaterial。
/// 不接受store或新的期限，不具备重读/补签来源的能力；失败不返还账号材料或调用机会。
pub(super) fn admit(
    call: &TaskCallRef,
    account: SnapshotMaterial,
    body: CreateBody,
) -> Result<Admitted, NotSentReason> {
    // 回调只返回仍拥有整个输入与Invocation的Admitted；锁内从不spawn或调用start。
    call.with_gate(|gate| {
        register(PreparedInput::assemble(
            call,
            gate,
            account,
            body,
            RegistrationUse(()),
        )?)
    })
}

fn register(prepared: GatedInput<'_, '_>) -> Result<Admitted, NotSentReason> {
    let (gate, input, window) = prepared.into_registration(RegistrationUse(()));
    let call = window.task_call().clone();
    let executor = gate.shared().managed_tasks.get(gate)?.clone();
    // 与PreparedInput中原字段相同的Arc，而非新建内容相同的Binding；gate也是构造借来的原门。
    let invocation = executor.invocation(call, gate, input.binding().clone(), window)?;
    Ok(Admitted { input, invocation })
}

impl Drop for ExecutorSlot {
    fn drop(&mut self) {
        self.revoke();
    }
}
