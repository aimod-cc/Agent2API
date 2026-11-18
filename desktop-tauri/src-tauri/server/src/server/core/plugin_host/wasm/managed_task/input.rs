//! 封闭宿主输入：当前调用批准仅从已保存的安装确认取得，绑定只由同源账号材料构造。
//! 点时来源不是Creating许可；绑定与登记使用同一调用门，最终资格仍须原账号唯一CAS。

use std::{sync::Arc, time::Duration};

use reqwest::header::HeaderValue;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{
    account_snapshot::{BindingAccount, SnapshotMaterial},
    approval_fence::{self, ApprovalEpoch, ApprovalGuard, FenceError},
    install_binding::{InstallBinding, InstallGuard},
    policy::CreateBody,
    status::NotSentReason,
    types::{self, ValidationError},
};

use super::super::conversation::{CallGate, TaskCallRef, TaskIdentity, TaskRuntime};

const MAX_UA_BYTES: usize = 1024;

/// 身份只能来自同源TaskCallRef与已重检账号回执；唯一构造封闭在PreparedInput::assemble。
/// 无公开构造器/serde，不接受自由identity/digest；摘要不替代原RowSource最终核对。
#[derive(PartialEq, Eq)]
pub(super) struct Binding {
    call: TaskIdentity,
    identity: String,
    credential_digest: [u8; 32],
    // 当前可信对话没有独立尝试ID；唯一构造固定为None，不用RPC编号猜造。
    attempt_id: Option<String>,
}

impl Binding {
    pub(super) fn provider(&self) -> &str { self.call.provider() }
    pub(super) fn same_call(&self, other: &Self) -> bool {
        self.call == other.call
    }
    pub(super) fn same_package(&self, other: &Self) -> bool {
        self.call.same_package(&other.call)
    }
    pub(super) fn same_account(&self, other: &Self) -> bool {
        self.call.same_account(&other.call)
    }
    pub(super) fn in_runtime(&self, runtime: &RuntimeBinding) -> bool {
        self.call.in_runtime(runtime)
    }
    pub(super) fn matches_call(&self, call: &TaskCallRef) -> bool {
        &self.call == call.identity()
    }

    pub(super) fn matches_account(
        &self,
        call: &TaskCallRef,
        identity: &str,
        credential_digest: &[u8; 32],
    ) -> bool {
        self.matches_call(call)
            && self.identity == identity
            && &self.credential_digest == credential_digest
    }

    pub(super) fn trace(&self) -> Trace {
        Trace {
            request_id: self.call.request().to_string(),
            attempt_id: self.attempt_id.clone().unwrap_or_default(),
            call_id: self.call.call_id(),
        }
    }
}

pub(super) type RuntimeBinding = TaskRuntime;

/// 无账号身份、凭据、任务编号或 URL 的有限关联标识，不接入主对话 TrafficCapture。
#[derive(Clone)]
pub(super) struct Trace {
    request_id: String,
    attempt_id: String,
    call_id: String,
}

impl Trace {
    pub(super) fn request_id(&self) -> &str {
        &self.request_id
    }
    pub(super) fn attempt_id(&self) -> &str {
        &self.attempt_id
    }
    pub(super) fn call_id(&self) -> &str {
        &self.call_id
    }
}

/// 只接受宿主已经选定的 UA；没有任意 HeaderMap、URL 或代理入口。
pub(super) struct FrozenUserAgent(HeaderValue);

impl FrozenUserAgent {
    pub(super) fn from_host_value(value: &str) -> Result<Self, ValidationError> {
        if value.trim().is_empty()
            || value.len() > MAX_UA_BYTES
            || !value.bytes().all(|byte| matches!(byte, 0x20..=0x7e))
        {
            return Err(ValidationError::new("冻结的 User-Agent 无效，未提交请求"));
        }
        HeaderValue::from_str(value)
            .map(Self)
            .map_err(|_| ValidationError::new("冻结的 User-Agent 无效，未提交请求"))
    }

    pub(super) fn header(&self) -> HeaderValue {
        self.0.clone()
    }
}

/// 期限只从账号首次准备时刻及原调用绝对期限推导，晚到的重检/登记不能延长。
/// 构造只接私有账号回执，不接受调用者传入now、deadline或另一份取消信号。
pub(super) struct CallWindow {
    prepare_deadline: Instant,
    call_deadline: Instant,
    cancellation: CancellationToken,
    call: TaskCallRef,
}

impl CallWindow {
    fn from_account(
        account: &BindingAccount,
        call: &TaskCallRef,
        gate: &CallGate<'_>,
    ) -> Result<Self, NotSentReason> {
        if !gate.matches(call) {
            return Err(NotSentReason::InvalidBinding);
        }
        let started = account.preparation_started();
        let now = Instant::now();
        if started > now {
            return Err(NotSentReason::InvalidBinding);
        }
        let life = types::bounded_lifetime(call.call_deadline().saturating_duration_since(started));
        let call_deadline = started + life;
        let prepare_deadline = call_deadline.min(started + types::PREPARE_TIMEOUT);
        if now >= call_deadline {
            return Err(NotSentReason::CallEnded);
        }
        if now >= prepare_deadline {
            return Err(NotSentReason::PreparationExpired);
        }
        Ok(Self {
            prepare_deadline,
            call_deadline,
            cancellation: call.ended_token(),
            call: call.clone(),
        })
    }

    pub(super) fn task_call(&self) -> &TaskCallRef {
        &self.call
    }

    pub(super) fn prepare_deadline(&self) -> Instant {
        self.prepare_deadline
    }
    pub(super) fn call_deadline(&self) -> Instant {
        self.call_deadline
    }
    pub(super) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

/// 真实批准的作用域；全局Epoch不能独立成为某包的授权。
/// 原InstallBinding锁定同次安装的完整声明字节、实际模块及运行回执，而非包名字符串。
/// 无Clone/serde；只在原调用门从已保存确认条目派生，不能替换安装对象或TaskCallRef。
pub(super) struct ScopedApproval {
    epoch: ApprovalEpoch,
    install: InstallBinding,
    call: TaskCallRef,
}

impl ScopedApproval {
    pub(super) fn for_call(call: &TaskCallRef, gate: &CallGate<'_>) -> Result<Self, FenceError> {
        let (epoch, install, call) = approval_fence::for_call(call, gate)?.into_parts();
        Ok(Self {
            epoch,
            install,
            call,
        })
    }
}

/// 只由封闭作用域批准装配；不把点时核对缓存成Creating许可。
pub(super) struct Finalization {
    binding: Arc<Binding>,
    approval: ScopedApproval,
}

impl Finalization {
    pub(super) fn matches(
        &self,
        binding: &Arc<Binding>,
        call: &TaskCallRef,
        gate: &CallGate<'_>,
    ) -> bool {
        Arc::ptr_eq(&self.binding, binding)
            && self.binding.matches_call(call)
            && self.approval.call.same_source(call)
            && gate.matches(call)
            && self.approval.install.belongs_to(call)
    }

    pub(super) fn try_approval(&self) -> Result<ApprovalGuard, FenceError> {
        approval_fence::try_hold(&self.approval.epoch, &self.approval.install)
    }

    pub(super) fn install(&self) -> &InstallBinding {
        &self.approval.install
    }

    pub(super) fn matches_install(&self, held: &InstallGuard<'_>) -> bool {
        held.matches(&self.approval.install)
    }
}

/// 已有真实账号材料与封闭批准；不含客户端，不在预留前消费选路。
/// 只允许真实私有类型装配，普通JSON/记录引用不能签发ScopedApproval或Binding。
pub(super) struct PreparedInput {
    finalization: Finalization,
    body: CreateBody,
    account: SnapshotMaterial,
}

/// 只有本模块能构造，账号模块据此交付封闭材料，不导出可自由填写的身份投影。
pub(super) struct AccountBindingUse(());

/// 借用当前构造门，不能逃出同一次with_gate或跨await；不是发送许可。
pub(super) struct GatedInput<'a, 'call> {
    gate: &'a mut CallGate<'call>,
    input: PreparedInput,
    window: CallWindow,
}

impl<'a, 'call> GatedInput<'a, 'call> {
    /// 仅runtime_slot的私有钥匙可拆，原gate随同原窗口/输入交付，不接受替代gate。
    pub(super) fn into_registration(
        self,
        _access: super::runtime_slot::RegistrationUse,
    ) -> (&'a mut CallGate<'call>, PreparedInput, CallWindow) {
        (self.gate, self.input, self.window)
    }
}

impl PreparedInput {
    /// 唯一Binding构造点。先从同门真实确认派生批准，再绑定已重检的原账号回执。
    /// 不接调用方提供的批准、identity/digest/attempt或新deadline，失败按值消耗材料。
    pub(super) fn assemble<'a, 'call>(
        call: &TaskCallRef,
        gate: &'a mut CallGate<'call>,
        account: SnapshotMaterial,
        body: CreateBody,
        _access: super::runtime_slot::RegistrationUse,
    ) -> Result<GatedInput<'a, 'call>, NotSentReason> {
        if !gate.matches(call) || !gate.can_bind() || !account.belongs_to(call) {
            return Err(NotSentReason::InvalidBinding);
        }
        let account = account.into_binding_account(call, gate, AccountBindingUse(()))?;
        let window = CallWindow::from_account(&account, call, gate)?;
        let approval = ScopedApproval::for_call(call, gate).map_err(|error| match error {
            FenceError::Busy | FenceError::Unavailable => NotSentReason::RuntimeUnavailable,
            FenceError::ConfirmationRequired | FenceError::Stale => NotSentReason::AdmissionClosed,
        })?;
        if !approval.call.same_source(call) || !approval.install.belongs_to(call) {
            return Err(NotSentReason::InvalidBinding);
        }
        let binding = Arc::new(Binding {
            call: call.identity().clone(),
            identity: account.identity().to_string(),
            credential_digest: *account.credential_digest(),
            attempt_id: None,
        });
        let account = account.into_account();
        if !account.matches_binding(&binding) {
            return Err(NotSentReason::InvalidBinding);
        }
        Ok(GatedInput {
            gate,
            input: Self {
                finalization: Finalization { binding, approval },
                body,
                account,
            },
            window,
        })
    }

    pub(super) fn binding(&self) -> &Arc<Binding> {
        &self.finalization.binding
    }

    pub(super) fn into_parts(self) -> (Finalization, CreateBody, SnapshotMaterial) {
        (self.finalization, self.body, self.account)
    }
}

pub(super) const DRAIN_TIMEOUT: Duration = Duration::from_secs(55);
// 独立于未结清配额：保留当次调用的一次机会和轻量终态，拒绝无限堆积调用对象。
pub(super) const MAX_INVOCATIONS: usize = crate::server::config::runtime_capacity::POOL_EXECUTIONS;
pub(super) const MAX_EXECUTORS: usize = 128;
