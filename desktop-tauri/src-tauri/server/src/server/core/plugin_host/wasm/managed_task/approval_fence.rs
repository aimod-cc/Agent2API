//! 批准绑定本次真实对话或明确保存的安装确认；配置读取不能恢复旧调用。
//! 最末批准检查不反取配置、账号或 Host，随后核对原运行来源及账号事务。

use std::sync::{Arc, Mutex, MutexGuard, TryLockError, Weak};

use super::{
    super::conversation::{CallGate, TaskCallRef},
    install_binding::InstallBinding,
    permission_install::InstallationScope,
    permissions::{Confirmation, ConfirmationIdentity},
};
use crate::server::config::plugin_task_grants::SavedConfirmation;

const MAX_CONFIRMED: usize = 256;

struct Confirmed {
    scope: Arc<InstallationScope>,
    operation: Arc<ConfirmationIdentity>,
}

enum ApprovalSource {
    Confirmation(Weak<Confirmed>),
    Request(TaskCallRef),
}

pub(super) struct ApprovalEpoch {
    generation: u64,
    source: ApprovalSource,
}

struct State {
    generation: Option<u64>,
    failed: bool,
    confirmed: Vec<Arc<Confirmed>>,
}

static FENCE: Mutex<State> = Mutex::new(State {
    generation: None,
    failed: false,
    confirmed: Vec::new(),
});

pub(super) enum FenceError {
    Busy,
    Unavailable,
    ConfirmationRequired,
    Stale,
}

#[must_use]
pub(super) struct ApprovalGuard {
    _guard: MutexGuard<'static, State>,
}

/// 配置更新锁内取得的单次操作票据；不能重新读取当前代次为旧确认续命。
pub(in crate::server) struct PendingConfirmation {
    generation: u64,
    operation: Arc<ConfirmationIdentity>,
}

/// 只把已验证安装批准移到原调用；字段私有，不能自行拼Epoch和另一InstallBinding。
pub(super) struct CallApproval {
    epoch: ApprovalEpoch,
    install: InstallBinding,
    call: TaskCallRef,
}

impl CallApproval {
    pub(super) fn into_parts(self) -> (ApprovalEpoch, InstallBinding, TaskCallRef) {
        (self.epoch, self.install, self.call)
    }
}

fn try_lock() -> Result<MutexGuard<'static, State>, FenceError> {
    match FENCE.try_lock() {
        Ok(guard) if !guard.failed => Ok(guard),
        Ok(_) | Err(TryLockError::Poisoned(_)) => Err(FenceError::Unavailable),
        Err(TryLockError::WouldBlock) => Err(FenceError::Busy),
    }
}

/// 最末只检查原条目和安装身份；紧接着由原InstallBinding持安装门到CAS。
pub(super) fn try_hold(
    expected: &ApprovalEpoch,
    install: &InstallBinding,
) -> Result<ApprovalGuard, FenceError> {
    let guard = try_lock()?;
    if guard.generation != Some(expected.generation) {
        return Err(FenceError::Stale);
    }
    let expected = match &expected.source {
        ApprovalSource::Request(request) => {
            if !install.belongs_to(request) {
                return Err(FenceError::Stale);
            }
            return Ok(ApprovalGuard { _guard: guard });
        }
        ApprovalSource::Confirmation(confirmed) => confirmed,
    };
    let confirmed = guard
        .confirmed
        .iter()
        .find(|entry| std::ptr::eq(Arc::as_ptr(entry), expected.as_ptr()))
        .ok_or(FenceError::ConfirmationRequired)?;
    if !confirmed.scope.matches_binding(install) {
        return Err(FenceError::Stale);
    }
    Ok(ApprovalGuard { _guard: guard })
}

/// 原调用门内只读现成来源，不从当前包名构造新安装回执，不读取磁盘Grants。
pub(super) fn for_call(
    call: &TaskCallRef,
    gate: &CallGate<'_>,
) -> Result<CallApproval, FenceError> {
    let mut guard = try_lock()?;
    if guard.generation.is_none() {
        guard.generation = Some(1);
    }
    let generation = guard.generation.ok_or(FenceError::Unavailable)?;
    if gate
        .shared()
        .options
        .manifest
        .managed_task
        .as_ref()
        .is_some_and(|declaration| {
            declaration
                .matches_call(gate.provider(), super::policy::ENTRY)
                .unwrap_or(false)
        })
    {
        let install = InstallBinding::from_call_gate(call, gate).ok_or(FenceError::Stale)?;
        return Ok(CallApproval {
            epoch: ApprovalEpoch {
                generation,
                source: ApprovalSource::Request(call.clone()),
            },
            install,
            call: call.clone(),
        });
    }
    let entry = guard
        .confirmed
        .iter()
        .find(|entry| entry.scope.matches_call(call, gate))
        .ok_or(FenceError::ConfirmationRequired)?;
    let _install = entry.scope.try_hold().map_err(|_| FenceError::Stale)?;
    let install = InstallBinding::from_scope(&entry.scope, call, gate).ok_or(FenceError::Stale)?;
    Ok(CallApproval {
        epoch: ApprovalEpoch {
            generation,
            source: ApprovalSource::Confirmation(Arc::downgrade(entry)),
        },
        install,
        call: call.clone(),
    })
}

pub(super) fn installation_confirmed(scope: &InstallationScope) -> bool {
    let Ok(guard) = try_lock() else {
        return false;
    };
    let Some(entry) = guard
        .confirmed
        .iter()
        .find(|entry| entry.scope.same_install(scope))
    else {
        return false;
    };
    let current = entry.scope.try_hold().is_ok();
    current
}

/// 撤销原代次并绑定本次确认；只接受私有管理材料，不接受bool或摘要。
pub(in crate::server) fn begin_confirmation(
    confirmation: &Confirmation,
) -> Result<PendingConfirmation, &'static str> {
    let operation = confirmation.operation().clone();
    let mut guard = FENCE.lock().map_err(|_| "受控任务审批状态不可用")?;
    if guard.failed {
        return Err("受控任务审批状态不可用");
    }
    let generation = match guard.generation.unwrap_or(0).checked_add(1) {
        Some(generation) => generation,
        None => {
            guard.failed = true;
            let removed = std::mem::take(&mut guard.confirmed);
            drop(guard);
            drop(removed);
            return Err("受控任务审批代次不可用");
        }
    };
    guard.generation = Some(generation);
    // 本次只替换目标安装；其它条目保留真实确认来源，但旧调用Epoch同样失效。
    // reload/显式撤销仍走invalidate清空，绝不从磁盘记录恢复这里的条目。
    let removed = guard
        .confirmed
        .iter()
        .position(|entry| entry.scope.pack() == confirmation.pack())
        .map(|index| guard.confirmed.remove(index));
    let full = guard.confirmed.len() >= MAX_CONFIRMED;
    drop(guard);
    drop(removed);
    if full {
        return Err("受控任务审批数量已达上限");
    }
    Ok(PendingConfirmation {
        generation,
        operation,
    })
}

/// 只有配置层真实保存并发布后能构造SavedConfirmation；消费后不返还或补造票据。
pub(in crate::server) fn publish_confirmation(saved: SavedConfirmation) -> bool {
    let (confirmation, pending) = saved.into_parts();
    if !Arc::ptr_eq(confirmation.operation(), &pending.operation) {
        return false;
    }
    let pending_generation = pending.generation;
    let entry = Arc::new(Confirmed {
        scope: confirmation.scope().clone(),
        operation: pending.operation,
    });
    let scope = entry.scope.clone();
    // Host/Runtime先持有；审批门内不再读取Host。安装门始终在审批门之后。
    confirmation
        .with_current_memory(|| {
            let Ok(mut guard) = try_lock() else {
                return false;
            };
            if guard.generation != Some(pending_generation)
                || guard.confirmed.len() >= MAX_CONFIRMED
                || guard
                    .confirmed
                    .iter()
                    .any(|current| current.scope.pack() == entry.scope.pack())
                || !Arc::ptr_eq(&entry.operation, confirmation.operation())
            {
                return false;
            }
            let Ok(_install) = scope.try_hold() else {
                return false;
            };
            guard.confirmed.push(entry);
            true
        })
        .unwrap_or(false)
}

/// 配置变动或重载只关闭旧条目；同内容也不能恢复先前确认。
pub(crate) fn invalidate() {
    let Ok(mut guard) = FENCE.lock() else {
        return;
    };
    if guard.failed {
        return;
    }
    guard.generation = match guard.generation.unwrap_or(0).checked_add(1) {
        Some(generation) => Some(generation),
        None => {
            guard.failed = true;
            None
        }
    };
    let removed = std::mem::take(&mut guard.confirmed);
    drop(guard);
    drop(removed);
}

pub(crate) fn fail_closed() {
    let Ok(mut guard) = FENCE.lock() else {
        return;
    };
    guard.failed = true;
    let removed = std::mem::take(&mut guard.confirmed);
    drop(guard);
    drop(removed);
}
