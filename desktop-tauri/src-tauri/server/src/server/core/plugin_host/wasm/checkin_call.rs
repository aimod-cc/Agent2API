//! 签到等待端的真实生命周期来源；仅记录事实，不授予账号、网络或提交权限。
//! 后台只持轻量信号与弱执行体身份，等待端Drop不被后台引用延长，也不取消旧普通HTTP。

mod entry;
pub(super) mod egress;
pub(super) mod doubao_daily;
pub(super) mod persistence;
mod result;
mod result_snapshot;
pub(in crate::server::core::plugin_host) use entry::Pending;
pub(super) use entry::{Entered, Prepared};
pub(super) use result::{PersistenceStatus, RecordedReply, RemoteStatus, ResultConsumer, ResultReceipt};
pub(super) use result_snapshot::{bind_result_snapshot, BoundResultSnapshot, ResultSnapshotError};
pub(in crate::server::core) use doubao_daily::{ObservationView, OfficialDailyFact};

pub(in crate::server::core::plugin_host) enum CallResult {
    Ordinary(Value),
    Unknown(persistence::Settlement),
}

use std::future::Future;
use std::sync::{atomic::Ordering, Arc, Mutex, Weak};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{context::{Context, Shared}, runtime::Runtime};
use super::super::Host;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EndReason {
    WaiterDropped,
    WaiterReturned,
    EnsureFailed,
    PreparationFailed,
    ExecutionDropped,
    ExecutionReturned,
    ExecutionFailed,
    Trap,
    ResourceLimit,
    Timeout,
    HostClosed,
    RuntimeReplaced,
    RuntimeCancelled,
    Unavailable,
    InvalidContext,
}

#[derive(Clone, Copy)]
enum WaiterEnd { Returned, Dropped }

#[derive(Default)]
struct State {
    waiter: Option<WaiterEnd>,
    execution: Option<EndReason>,
    reason: Option<EndReason>,
    bound: bool,
    entered: bool,
    context_entered: bool,
    result: Option<result::ResultOwner>,
    result_snapshot: result_snapshot::BindingState,
    daily_egress_issued: bool,
}

struct Signal {
    state: Mutex<State>,
    ended: CancellationToken,
}

impl Signal {
    fn close(&self, reason: EndReason) {
        self.state.lock().unwrap_or_else(|error| error.into_inner()).reason.get_or_insert(reason);
        self.ended.cancel();
    }

    fn execution_ended(&self, reason: EndReason, reply: Option<&Result<Value, proto::WireError>>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.execution.is_none() {
            // 原执行守卫是唯一写者；等待者Drop只记等待事实，绝不提前封存未知结果。
            if let Some(owner) = &mut state.result { owner.seal(reason, reply); }
            state.execution = Some(reason);
        }
        state.reason.get_or_insert(reason);
        drop(state);
        self.ended.cancel();
    }
}

/// 唯一等待所有者在返回future构造前建立；不实现Clone，后台从不持有此对象。
#[must_use]
pub(in crate::server::core::plugin_host) struct Waiter {
    signal: Arc<Signal>,
    returned: bool,
}

impl Waiter {
    pub(in crate::server::core::plugin_host) fn new(
        provider: &str, account: &str, timeout: Duration,
    ) -> (Self, Pending) {
        let signal = Arc::new(Signal { state: Mutex::new(State::default()), ended: CancellationToken::new() });
        // 尚未选择执行体；Pending不是可供Context消费的可信来源，非WASM分支直接丢弃。
        let pending = Pending::new(signal.clone(), provider, account, timeout);
        (Self { signal, returned: false }, pending)
    }

    pub(in crate::server::core::plugin_host) fn wait<F: Future>(mut self, future: F) -> impl Future<Output = F::Output> {
        async move {
            let output = future.await;
            let mut state = self.signal.state.lock().unwrap_or_else(|error| error.into_inner());
            state.waiter.get_or_insert(WaiterEnd::Returned);
            state.reason.get_or_insert(EndReason::WaiterReturned);
            drop(state);
            self.returned = true;
            self.signal.ended.cancel();
            output
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if !self.returned {
            let mut state = self.signal.state.lock().unwrap_or_else(|error| error.into_inner());
            state.waiter.get_or_insert(WaiterEnd::Dropped);
            state.reason.get_or_insert(EndReason::WaiterDropped);
            drop(state);
            self.signal.ended.cancel();
        }
    }
}

struct Source {
    signal: Arc<Signal>,
    host: Weak<Host>,
    runtime: Weak<Runtime>,
    shared: Weak<Shared>,
    runtime_cancel: CancellationToken,
    generation: u64,
    provider: String,
    account: String,
    deadline: Instant,
}

/// Clone仅保留轻量事实，不持等待所有者、强Host/Runtime/Shared或任何账号材料。
#[derive(Clone)]
pub(super) struct CheckinCallRef {
    source: Arc<Source>,
}

impl CheckinCallRef {
    /// 此期限在原等待future构造时固定；仅供后续受控阶段与Context期限取较早值，
    /// 本批不改旧runtime/HTTP的截止或因取消直接丢弃已发请求。
    pub(super) fn deadline(&self) -> Instant { self.source.deadline }

    /// 只有当前Context可取只读引用；观察结果不是发送授权，调用者仍须在使用时复核。
    pub(super) fn for_context(context: &Context) -> Result<Self, proto::WireError> {
        let call = context.checkin_call.as_ref().ok_or_else(invalid)?;
        call.observe_context(context)?;
        Ok(call.clone())
    }

    /// 等待方取消与执行正常返回分别保留，首个结束原因不能掩盖后来丢弃等待的事实。
    pub(super) fn waiter_dropped(&self) -> bool {
        let state = self.source.signal.state.lock().unwrap_or_else(|error| error.into_inner());
        matches!(state.waiter, Some(WaiterEnd::Dropped))
    }

    /// 仅复核来源；成功不等于准许发网，也不能缓存成之后的发送许可。
    pub(super) fn end_reason(&self) -> Option<EndReason> {
        let result = self.inspect();
        if let Some(reason) = result { self.source.signal.close(reason); }
        self.source.signal.state.lock().unwrap_or_else(|error| error.into_inner()).reason
    }

    fn inspect(&self) -> Option<EndReason> {
        let state = match self.source.signal.state.lock() {
            Ok(state) => state,
            Err(_) => return Some(EndReason::Unavailable),
        };
        if state.reason.is_some() { return state.reason; }
        if Instant::now() >= self.source.deadline { return Some(EndReason::Timeout); }
        let Some(host) = self.source.host.upgrade() else { return Some(EndReason::Unavailable) };
        // 锁序始终Signal→HostLifecycle→RuntimeTryRead；没有同步锁跨await。
        let Ok(_lifecycle) = host.lifecycle.lock() else { return Some(EndReason::Unavailable) };
        let Ok(runtime) = host.embedded.runtime.try_read() else { return Some(EndReason::Unavailable) };
        if host.closing.load(Ordering::Acquire) || super::super::runtime::admission_closed() {
            return Some(EndReason::HostClosed);
        }
        if !state.bound || !state.entered || !state.context_entered || !host.ready() {
            return Some(EndReason::Unavailable);
        }
        if !runtime.as_ref().is_some_and(|runtime| self.matches_runtime(runtime))
            || host.generation.load(Ordering::Acquire) != self.source.generation {
            return Some(EndReason::RuntimeReplaced);
        }
        if self.source.runtime_cancel.is_cancelled() { return Some(EndReason::RuntimeCancelled); }
        None
    }

    fn matches_runtime(&self, runtime: &Runtime) -> bool {
        std::ptr::eq(self.source.runtime.as_ptr(), runtime)
            && std::ptr::eq(self.source.shared.as_ptr(), Arc::as_ptr(&runtime.shared))
            && runtime.shared.options.provider == self.source.provider
    }

    pub(super) fn enter_context(&self, runtime: &Runtime, method: &str, params: &Value) -> Result<(), proto::WireError> {
        if method != proto::methods::gateway::CHECKIN_CLAIM || !self.matches_runtime(runtime)
            || params["provider"].as_str() != Some(self.source.provider.as_str())
            || params["account"].as_str() != Some(self.source.account.as_str()) {
            self.source.signal.close(EndReason::InvalidContext);
            return Err(invalid());
        }
        // 消费唯一Context装配机会；Clone只读引用不能拿去启动第二个WASM实例。
        let mut state = self.source.signal.state.lock().map_err(|_| invalid())?;
        if !state.bound || !state.entered || state.context_entered || state.execution.is_some() {
            state.reason.get_or_insert(EndReason::InvalidContext);
            drop(state);
            self.source.signal.ended.cancel();
            return Err(invalid());
        }
        // 只在真实原Context入场时创建豆包结果所有者；容量耗尽在插件执行前拒绝。
        // 此槽不含原凭据，不代表签到发送、账号终检或持久保存许可。
        let owner = result::ResultOwner::for_call(self)?;
        state.result = owner;
        state.context_entered = true;
        // 不以等待取消拒绝旧签到调用；后续受控准备/回查必须另读end_reason。
        Ok(())
    }

    pub(super) fn observe_context(&self, context: &Context) -> Result<(), proto::WireError> {
        if !std::ptr::eq(self.source.shared.as_ptr(), Arc::as_ptr(&context.shared))
            || context.shared.options.provider != self.source.provider || context.account != self.source.account {
            self.source.signal.close(EndReason::InvalidContext);
            return Err(invalid());
        }
        // 只登记失效事实，不将其变成旧HTTP调用的中断信号。
        self.end_reason();
        Ok(())
    }

    /// 等待的是终止事实，不公开可调用cancel的Token；无后台任务或轮询。
    pub(super) async fn ended(&self) -> EndReason {
        if let Some(reason) = self.end_reason() { return reason; }
        tokio::select! {
            biased;
            _ = self.source.signal.ended.cancelled() => {},
            _ = self.source.runtime_cancel.cancelled() => self.source.signal.close(EndReason::RuntimeCancelled),
            _ = tokio::time::sleep_until(self.source.deadline) => self.source.signal.close(EndReason::Timeout),
        }
        self.end_reason().unwrap_or(EndReason::Unavailable)
    }
}

/// 构造在ensure/spawn的future之外，未poll就销毁也能记录本次执行已中断。
pub(super) struct ExecutionGuard {
    signal: Arc<Signal>,
    finished: bool,
}

impl ExecutionGuard {
    fn new(signal: Arc<Signal>) -> Self { Self { signal, finished: false } }

    fn finish(&mut self, reason: EndReason) {
        self.complete(reason, None);
    }

    fn complete(&mut self, reason: EndReason, reply: Option<&Result<Value, proto::WireError>>) {
        if !self.finished {
            self.signal.execution_ended(reason, reply);
            self.finished = true;
        }
    }

    pub(super) fn runtime_result(&mut self, result: &Result<Value, proto::WireError>) {
        // 在插件JSON解码前记录真实运行结果，不从插件自报业务错误推断Trap/超时。
        let reason = match result {
            Ok(_) => EndReason::ExecutionReturned,
            Err(error) => match error.code.as_deref() {
                Some("timeout") => EndReason::Timeout,
                Some("plugin_trap") => EndReason::Trap,
                Some("plugin_resource_limit") => EndReason::ResourceLimit,
                Some("plugin_unavailable") => EndReason::RuntimeCancelled,
                _ => EndReason::ExecutionFailed,
            },
        };
        self.complete(reason, Some(result));
    }
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) { self.finish(EndReason::ExecutionDropped); }
}

pub(super) fn invalid() -> proto::WireError {
    proto::WireError::with_code("Wasm 签到调用与原执行体不一致", "checkin_call_invalid")
}
