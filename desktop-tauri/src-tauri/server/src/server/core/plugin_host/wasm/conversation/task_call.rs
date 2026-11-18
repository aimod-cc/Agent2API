//! 单次任务引用只从已绑定对话签发；借用门不能脱离同步闭包或延长调用租约。

use std::marker::PhantomData;
use std::sync::{Arc, Weak};

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::super::{
    context::{Context, Shared},
    managed_task::{NotSentReason, Record},
    runtime::Runtime,
};
use super::{bounded, ConsumedEgress, ConversationCall, ConversationEgress, EndState, Signal};
use crate::server::core::plugin_host::{self as host, account_egress::EgressError, packages::provenance::InstallLease, Host};
use crate::server::core::proxies::chain::SelectedPath;
use crate::server::core::upstream::replay_guard::PreparationRef;

/// 运行身份以实际Shared弱引用和宿主代次比较，不以调用方提供的字符串比较。
#[derive(Clone)]
pub(in crate::server::core::plugin_host::wasm) struct TaskRuntime {
    host: Weak<Host>,
    shared: Weak<Shared>,
    generation: u64,
}

impl PartialEq for TaskRuntime {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
            && Weak::ptr_eq(&self.host, &other.host)
            && Weak::ptr_eq(&self.shared, &other.shared)
    }
}
impl Eq for TaskRuntime {}

impl TaskRuntime {
    /// 只比较宿主已绑定的弱身份，不升级弱引用、不查当前Host，也不创建运行身份。
    fn matches_loaded(&self, host: &Arc<Host>, runtime: &Arc<Runtime>, generation: u64) -> bool {
        self.generation == generation
            && std::ptr::eq(self.host.as_ptr(), Arc::as_ptr(host))
            && std::ptr::eq(self.shared.as_ptr(), Arc::as_ptr(&runtime.shared))
    }

    /// 仅供宿主排空按真实弱引用身份筛选旧代次，不从包名/JSON重建身份。
    pub(in crate::server::core::plugin_host::wasm) fn belongs_to_host(&self, host: &Host) -> bool {
        std::ptr::eq(self.host.as_ptr(), host)
    }

    pub(in crate::server::core::plugin_host::wasm) fn shutdown_host(&self) -> Option<Arc<Host>> {
        self.host.upgrade()
    }

    pub(in crate::server::core::plugin_host::wasm) fn shutdown_shared(&self) -> Option<Arc<Shared>> {
        self.shared.upgrade()
    }

    pub(in crate::server::core::plugin_host::wasm) fn admission_open(&self) -> bool {
        !host::runtime::admission_closed()
            && self.host.upgrade().is_some_and(|host| host.ready())
            && self.shared.upgrade().is_some_and(|shared| !shared.cancel.is_cancelled())
    }
}

/// 只承载已由宿主绑定的调用身份，不代表批准或凭据真实性。
#[derive(Clone)]
pub(in crate::server::core::plugin_host::wasm) struct TaskIdentity {
    runtime: TaskRuntime,
    signal: Weak<Signal>,
    package: String,
    provider: String,
    account: String,
    request: String,
    rpc: i64,
}

impl PartialEq for TaskIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.runtime == other.runtime
            && Weak::ptr_eq(&self.signal, &other.signal)
            && self.package == other.package
            && self.provider == other.provider
            && self.account == other.account
            && self.request == other.request
            && self.rpc == other.rpc
    }
}
impl Eq for TaskIdentity {}

impl TaskIdentity {
    /// 包/provider仅取自已提交Lease；回执还须匹配原Host/Shared/generation，不能靠字符串补贴。
    fn matches_install(
        &self,
        lease: &InstallLease,
        host: &Arc<Host>,
        runtime: &Arc<Runtime>,
        generation: u64,
    ) -> bool {
        let artifact = lease.artifact();
        self.package == artifact.pack()
            && artifact.wasm_provider() == Some(self.provider.as_str())
            && self.runtime.matches_loaded(host, runtime, generation)
            && lease.started_runtime().is_some_and(|started| {
                started.matches_call(&self.runtime.host, &self.runtime.shared, self.runtime.generation)
                    && started.matches(host, runtime, generation)
            })
    }

    pub(in crate::server::core::plugin_host::wasm) fn provider(&self) -> &str { &self.provider }
    pub(in crate::server::core::plugin_host::wasm) fn account(&self) -> &str { &self.account }

    pub(in crate::server::core::plugin_host::wasm) fn same_package(&self, other: &Self) -> bool {
        self.package == other.package
    }
    pub(in crate::server::core::plugin_host::wasm) fn same_account(&self, other: &Self) -> bool {
        self.provider == other.provider && self.account == other.account
    }
    pub(in crate::server::core::plugin_host::wasm) fn in_runtime(
        &self,
        runtime: &TaskRuntime,
    ) -> bool {
        self.runtime == *runtime
    }
    pub(in crate::server::core::plugin_host::wasm) fn request(&self) -> &str {
        &self.request
    }
    pub(in crate::server::core::plugin_host::wasm) fn call_id(&self) -> String {
        format!("{}:{}", self.runtime.generation, self.rpc)
    }
}

struct TaskSource {
    identity: TaskIdentity,
    preparation: PreparationRef,
    signal: Arc<Signal>,
    // 只保留本次对话来源的弱身份，不把代理凭据加入轻量调用或终态记录。
    egress: Result<Weak<ConversationEgress>, EgressError>,
    call_deadline: Instant,
}

/// Clone只复制轻量Arc；没有Owner、CallLease、强Host/Shared或账号材料。
#[derive(Clone)]
pub(in crate::server::core::plugin_host::wasm) struct TaskCallRef {
    source: Arc<TaskSource>,
}

/// 字段私有；闭包结果不能携带本门，且不能将Signal反向锁进Table临界区。
pub(in crate::server::core::plugin_host::wasm) struct CallGate<'a> {
    source: &'a TaskSource,
    state: &'a mut EndState,
    shared: &'a Arc<Shared>,
    host: &'a Arc<Host>,
    loaded_runtime: &'a Arc<Runtime>,
    invariant: PhantomData<&'a mut &'a ()>,
    not_send: PhantomData<std::rc::Rc<()>>,
}

impl Context {
    pub(in crate::server::core::plugin_host::wasm) fn task_call_ref(
        &self,
    ) -> Result<TaskCallRef, NotSentReason> {
        self.conversation
            .as_ref()
            .ok_or(NotSentReason::InvalidBinding)?
            .task_ref(&self.shared, &self.account, self.call_deadline)
    }
}

impl ConversationCall {
    fn task_ref(
        &self,
        shared: &Arc<Shared>,
        account: &str,
        call_deadline: Instant,
    ) -> Result<TaskCallRef, NotSentReason> {
        let mut state = self
            .signal
            .state
            .lock()
            .map_err(|_| NotSentReason::RuntimeUnavailable)?;
        if !state.guard_issued
            || state.consumer
            || state.execution.is_some()
            || state.task_issued
            || state.source_issued
            || state.http_submission
            || state.record.is_some()
            || self.account.is_empty()
            || self.request.is_empty()
            || !matches!(self.provider.as_str(), "monkeycode" | "monkeycode-intl")
            || account != self.account
            || call_deadline <= Instant::now()
            || !self
                .shared
                .upgrade()
                .is_some_and(|bound| Arc::ptr_eq(&bound, shared))
        {
            return Err(NotSentReason::CallEnded);
        }
        self.with_current(|| Ok(()))
            .map_err(|_| NotSentReason::RuntimeUnavailable)?;
        let package = host::provider_pack(&self.provider)
            .filter(|value| bounded(value, 128) && !value.is_empty())
            .ok_or(NotSentReason::InvalidBinding)?;
        if !host::host_for_pack(&package).is_some_and(|current| {
            self.host
                .upgrade()
                .is_some_and(|bound| Arc::ptr_eq(&bound, &current))
        }) {
            return Err(NotSentReason::InvalidBinding);
        }
        let identity = TaskIdentity {
            runtime: TaskRuntime {
                host: self.host.clone(),
                shared: self.shared.clone(),
                generation: self.generation,
            },
            signal: Arc::downgrade(&self.signal),
            package,
            provider: self.provider.clone(),
            account: self.account.clone(),
            request: self.request.clone(),
            rpc: self.rpc,
        };
        let source = Arc::new(TaskSource {
            identity,
            preparation: self.preparation.clone(),
            signal: self.signal.clone(),
            egress: self.egress.as_ref().map(Arc::downgrade).map_err(|error| *error),
            call_deadline,
        });
        state.task_issued = true;
        Ok(TaskCallRef { source })
    }
}

impl TaskCallRef {
    pub(in crate::server::core::plugin_host::wasm) fn package(&self) -> &str {
        &self.source.identity.package
    }

    pub(in crate::server::core::plugin_host::wasm) fn identity(&self) -> &TaskIdentity {
        &self.source.identity
    }
    pub(in crate::server::core::plugin_host::wasm) fn call_deadline(&self) -> Instant {
        self.source.call_deadline
    }
    pub(in crate::server::core::plugin_host::wasm) fn ended_token(&self) -> CancellationToken {
        self.source.signal.ended.clone()
    }
    pub(in crate::server::core::plugin_host::wasm) fn same_source(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
    }

    /// 管理/准备阶段只核对原调用与现成Loaded，不查注册表，不签发调用或安装回执。
    pub(in crate::server::core::plugin_host::wasm) fn matches_install(
        &self,
        lease: &InstallLease,
        host: &Arc<Host>,
        runtime: &Arc<Runtime>,
        generation: u64,
    ) -> bool {
        self.source.identity.matches_install(lease, host, runtime, generation)
    }

    pub(in crate::server::core::plugin_host::wasm) fn with_gate<T>(
        &self,
        action: impl for<'a> FnOnce(&mut CallGate<'a>) -> Result<T, NotSentReason>,
    ) -> Result<T, NotSentReason> {
        let mut state = self
            .source
            .signal
            .state
            .lock()
            .map_err(|_| NotSentReason::RuntimeUnavailable)?;
        if state.consumer
            || state.execution.is_some()
            || !state.guard_issued
            || !state.task_issued
            || Instant::now() >= self.source.call_deadline
        {
            return Err(NotSentReason::CallEnded);
        }
        let identity = &self.source.identity;
        let shared = identity
            .runtime
            .shared
            .upgrade()
            .ok_or(NotSentReason::RuntimeUnavailable)?;
        let Some(host) = identity.runtime.host.upgrade() else {
            shared.managed_tasks.revoke();
            return Err(NotSentReason::RuntimeUnavailable);
        };
        if !host::host_for_pack(&identity.package)
            .is_some_and(|current| Arc::ptr_eq(&current, &host))
            || !host::host_for_provider(&identity.provider)
                .is_some_and(|current| Arc::ptr_eq(&current, &host))
            || host::registry::is_switching(&identity.provider)
            || !host::serves_provider(&identity.provider)
        {
            shared.managed_tasks.revoke();
            return Err(NotSentReason::InvalidBinding);
        }
        let _lifecycle = host.lifecycle.lock().map_err(|_| {
            shared.managed_tasks.revoke();
            NotSentReason::RuntimeUnavailable
        })?;
        let runtime = host.embedded.runtime.try_read().map_err(|_| {
            shared.managed_tasks.revoke();
            NotSentReason::RuntimeUnavailable
        })?;
        if !host.ready()
            || host.generation.load(std::sync::atomic::Ordering::Acquire)
                != identity.runtime.generation
            || shared.cancel.is_cancelled()
            || shared.options.provider != identity.provider
            || !runtime
                .as_ref()
                .is_some_and(|runtime| Arc::ptr_eq(&runtime.shared, &shared))
        {
            shared.managed_tasks.revoke();
            return Err(NotSentReason::RuntimeUnavailable);
        }
        let Some(current_runtime) = runtime.as_ref() else {
            shared.managed_tasks.revoke();
            return Err(NotSentReason::RuntimeUnavailable);
        };
        let mut gate = CallGate {
            source: &self.source,
            state: &mut state,
            shared: &shared,
            host: &host,
            loaded_runtime: current_runtime,
            invariant: PhantomData,
            not_send: PhantomData,
        };
        shared.managed_tasks.ensure(&gate)?;
        action(&mut gate)
    }
}

impl<'a> CallGate<'a> {
    pub(in crate::server::core::plugin_host::wasm) fn provider(&self) -> &str {
        &self.source.identity.provider
    }
    pub(in crate::server::core::plugin_host::wasm) fn runtime(&self) -> &TaskRuntime {
        &self.source.identity.runtime
    }
    pub(in crate::server::core::plugin_host::wasm) fn shared(&self) -> &Shared {
        self.shared
    }
    pub(in crate::server::core::plugin_host) fn source_digest(&self) -> &[u8; 32] {
        self.shared.options.source_digest()
    }
    pub(in crate::server::core::plugin_host) fn module_digest(&self) -> &str {
        self.loaded_runtime.module_digest()
    }
    pub(in crate::server::core::plugin_host::wasm) fn matches(&self, call: &TaskCallRef) -> bool {
        std::ptr::eq(self.source, call.source.as_ref())
    }
    /// 必须持真实调用门及Preparing记录门；最后才try选路槽，不从槽锁反向取前置锁。
    pub(in crate::server::core::plugin_host::wasm) fn take_egress(
        &self,
        preparing: &super::super::managed_task::PreparingGuard<'_>,
    ) -> Result<(SelectedPath, ConsumedEgress), NotSentReason> {
        let record = preparing.record();
        if !preparing.matches(self) || !self.owns_record(record) {
            return Err(NotSentReason::InvalidBinding);
        }
        let weak = self.source.egress.as_ref().map_err(|_| NotSentReason::RequestUnavailable)?;
        let source = weak.upgrade().ok_or(NotSentReason::RuntimeUnavailable)?;
        let identity = &self.source.identity;
        if !source.matches(&identity.signal, &identity.provider, &identity.account) {
            return Err(NotSentReason::InvalidBinding);
        }
        let path = source.take_path()?;
        Ok((path, ConsumedEgress { source: weak.clone(), record: Arc::downgrade(record) }))
    }

    /// 只比较消费时保存的弱对象身份；不升级来源，不读取已经消耗的代理槽。
    pub(in crate::server::core::plugin_host::wasm) fn matches_consumed_egress(
        &self,
        record: &Arc<Record>,
        consumed: &ConsumedEgress,
    ) -> bool {
        self.owns_record(record)
            && Weak::ptr_eq(&consumed.record, &Arc::downgrade(record))
            && self.source.egress.as_ref().is_ok_and(|source| Weak::ptr_eq(source, &consumed.source))
    }
    /// 本门已借用当前Host/Runtime锁；仅比较原调用与已提交来源，不重复取Host锁。
    pub(in crate::server::core::plugin_host::wasm) fn matches_install(
        &self,
        call: &TaskCallRef,
        lease: &InstallLease,
    ) -> bool {
        self.matches(call)
            && self.source.identity.matches_install(
                lease, self.host, self.loaded_runtime, self.source.identity.runtime.generation,
            )
    }
    pub(in crate::server::core::plugin_host::wasm) fn can_bind(&self) -> bool {
        self.state.record.is_none()
    }
    pub(in crate::server::core::plugin_host::wasm) fn bind_record(
        &mut self,
        record: &Arc<Record>,
    ) -> Result<(), NotSentReason> {
        if !self.can_bind() || !self.matches(record.task_call()) {
            return Err(NotSentReason::AlreadySubmitted);
        }
        self.state.record = Some(Arc::downgrade(record));
        Ok(())
    }
    pub(in crate::server::core::plugin_host::wasm) fn owns_record(
        &self,
        record: &Arc<Record>,
    ) -> bool {
        self.matches(record.task_call())
            && self
                .state
                .record
                .as_ref()
                .is_some_and(|registered| Weak::ptr_eq(registered, &Arc::downgrade(record)))
    }
    pub(in crate::server::core::plugin_host::wasm) fn freeze(
        &self,
        record: &Arc<Record>,
    ) -> Result<(), NotSentReason> {
        if !self.owns_record(record) {
            return Err(NotSentReason::InvalidBinding);
        }
        if let Some(reason) = record.preparing_error() {
            return Err(reason);
        }
        self.source
            .preparation
            .freeze()
            .map_err(|_| NotSentReason::CallEnded)
    }
}
