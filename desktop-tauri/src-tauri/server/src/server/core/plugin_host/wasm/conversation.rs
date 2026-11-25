//! 真实对话的调用租约与执行结束信号；不授予任务、账号或网络权限。

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};

pub(super) mod ordinary_http;
mod catpaw_material;
pub(super) use catpaw_material::CatPawMaterial;
pub(in crate::server::core::plugin_host) use ordinary_http::{WorkBuddyExecution, WORKBUDDY_MAX_BODY};
pub(in crate::server::core::plugin_host::wasm) use ordinary_http::HttpRejectionObserver;
mod source_ref;
mod task_call;
pub(super) use source_ref::{SourceGate, SourceRef};
pub(super) use task_call::{CallGate, TaskCallRef, TaskIdentity, TaskRuntime};

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::super::{account_egress::{EgressError, EgressSelection}, Host};
use super::context::Shared;
use super::managed_task::{NotSentReason, Record};
use crate::server::core::proxies::chain::SelectedPath;
use crate::server::core::upstream::{
    replay_guard::{CallLease, PreparationRef, ReplayError},
    usage::RequestTelemetry,
};
use crate::server::errors::GatewayError;

#[derive(Clone, Copy)]
enum ExecutionEnd {
    Returned,
    Interrupted,
}

#[derive(Default)]
struct EndState {
    consumer: bool,
    execution: Option<ExecutionEnd>,
    guard_issued: bool,
    task_issued: bool,
    source_issued: bool,
    http_submission: bool,
    // Some 即已绑定过，Weak回收不能把一次调用还原成可绑定的新调用。
    record: Option<Weak<super::managed_task::Record>>,
}

struct Signal {
    state: Mutex<EndState>,
    ended: CancellationToken,
}

impl Signal {
    fn new() -> Self {
        Self {
            state: Mutex::new(EndState::default()),
            ended: CancellationToken::new(),
        }
    }

    fn execution_ended(&self, end: ExecutionEnd) {
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let recorded = *state.execution.get_or_insert(end);
            // 这里只知道执行返回或中断，不能从插件JSON推断任务成功或猜测陷阱原因。
            stop_record(
                &state,
                match recorded {
                    ExecutionEnd::Returned => super::managed_task::EndReason::Normal,
                    ExecutionEnd::Interrupted => {
                        super::managed_task::EndReason::ExecutionInterrupted
                    }
                },
            );
        }
        self.ended.cancel();
    }
}

fn stop_record(state: &EndState, reason: super::managed_task::EndReason) {
    if let Some(record) = state.record.as_ref().and_then(Weak::upgrade) {
        // 只写Record原子停止位与唤醒，不取得Table/ScopeGate/Record状态锁。
        record.request_stop(reason);
    }
}

/// 只在真实open_call成功后构造；消费后仅保留Signal/provider/account轻身份和关闭槽。
/// 强选择只存于不可回补槽，TaskCallRef仍只弱指向本对象，不延长CallLease寿命。
pub(super) struct ConversationEgress {
    selected: Mutex<Option<EgressSelection>>,
    signal: Weak<Signal>,
    provider: String,
    account: String,
}

/// 路径只在原Preparing门内take后产生；弱Record和弱来源不保留代理材料或调用租约。
/// 字段私有，无Clone/serde；仅供准备材料在异步冻结后再次核对同一消费来源。
pub(super) struct ConsumedEgress {
    source: Weak<ConversationEgress>,
    record: Weak<Record>,
}

impl ConversationEgress {
    fn matches(&self, signal: &Weak<Signal>, provider: &str, account: &str) -> bool {
        Weak::ptr_eq(&self.signal, signal) && self.provider == provider && self.account == account
    }

    /// 最末选路槽只try一次；成功take即永久消费，后续失败、取消或Drop均不能放回。
    fn take_path(&self) -> Result<SelectedPath, NotSentReason> {
        let selected = self.selected.try_lock().map_err(|_| NotSentReason::RuntimeUnavailable)?
            .take().ok_or(NotSentReason::AlreadyPreparing)?;
        if !selected.matches_account(&self.provider, &self.account) {
            return Err(NotSentReason::InvalidBinding);
        }
        selected.into_path().map_err(|_| NotSentReason::RequestUnavailable)
    }
}

/// 唯一所有者只在 RPC 等待端或响应消费端移动，后台任务不能保留它。
#[must_use]
pub(in crate::server::core::plugin_host) struct ConversationOwner {
    lease: Option<CallLease>,
    signal: Arc<Signal>,
    output: Option<Arc<super::catpaw_turn::output::OutputSlot>>,
}

impl ConversationOwner {
    pub(in crate::server::core::plugin_host) fn open(
        host: &Arc<Host>,
        provider: &str,
        account: &str,
        telemetry: &RequestTelemetry,
        egress: Result<EgressSelection, EgressError>,
    ) -> Result<(Self, ConversationCall), GatewayError> {
        let request = telemetry.id();
        if host.options.script
            || host
                .options
                .wasm
                .as_ref()
                .is_none_or(|options| options.provider != provider)
            || provider.is_empty()
            || !bounded(provider, 128)
            || !bounded(account, 512)
            || !bounded(&request, 512)
        {
            return Err(GatewayError::with_status(
                403,
                "对话调用不属于已选定的 Wasm 执行体",
            ));
        }
        if telemetry.is_cancelled() {
            return Err(crate::server::core::upstream::cancellation::cancelled_error());
        }
        let key_source = telemetry.model_key_source().cloned();
        let lease = telemetry
            .replay_guard()
            .open_call()
            .map_err(|error| match error {
                ReplayError::Blocked => {
                    GatewayError::with_status(409, "本次请求已禁止重放，未开启新的对话调用")
                        .with_code("request_replay_blocked")
                }
                ReplayError::CallActive => {
                    GatewayError::with_status(409, "本次请求尚有未结束的对话调用")
                        .with_code("request_call_active")
                }
                _ => GatewayError::with_status(503, "对话调用租约不可用")
                    .with_code("request_call_unavailable"),
            })?;
        let signal = Arc::new(Signal::new());
        let output = if provider == "catpaw" {
            let output = super::catpaw_turn::output::OutputSlot::new();
            telemetry.install_catpaw_output(output.clone())
                .map_err(|_| GatewayError::with_status(409, "CatPaw 本次响应观察来源已占用").with_code("catpaw_output_source_consumed"))?;
            Some(output)
        } else { None };
        // 绑定到刚由真实CallLease建立的Signal，不以请求串/RPC号补造来源。
        // 这时仍在原scope；后续spawn只移动结果，不再读取task_local。
        let egress = egress.and_then(|selected| {
            if !selected.still_selected(provider, account, telemetry) {
                return Err(EgressError::DifferentSource);
            }
            Ok(Arc::new(ConversationEgress {
                selected: Mutex::new(Some(selected)), signal: Arc::downgrade(&signal),
                provider: provider.to_string(), account: account.to_string(),
            }))
        });
        let call = ConversationCall {
            preparation: lease.preparation(),
            key_source,
            output: output.clone(),
            signal: signal.clone(),
            egress,
            provider: provider.to_string(),
            account: account.to_string(),
            request,
            capacity_caller: crate::server::core::runtime_capacity::caller(),
            host: Arc::downgrade(host),
            shared: Weak::new(),
            generation: 0,
            rpc: 0,
        };
        Ok((
            Self {
                lease: Some(lease),
                signal,
                output,
            },
            call,
        ))
    }

    pub(in crate::server::core::plugin_host) fn select_output(&self) -> Result<(), GatewayError> {
        let Some(output) = &self.output else { return Ok(()); };
        let state = self.signal.state.lock().map_err(|_| output_unavailable())?;
        if state.consumer { return Err(output_unavailable()); }
        self.lease.as_ref().ok_or_else(output_unavailable)?.preparation().freeze()
            .map_err(|_| output_unavailable())?;
        output.select().map_err(|_| output_unavailable())
    }

    pub(in crate::server::core::plugin_host) fn close(&mut self) {
        if let Some(output) = &self.output { output.close_attempt(); }
        if self.lease.is_none() {
            return;
        }
        {
            let mut state = self
                .signal
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // 与Preparing/Creating共用Signal门，先撤销唯一租约，再登记消费终态。
            if let Some(lease) = self.lease.take() {
                lease.close();
            }
            state.consumer = true;
            stop_record(&state, super::managed_task::EndReason::CallerDropped);
        }
        self.signal.ended.cancel();
    }
}

impl Drop for ConversationOwner {
    fn drop(&mut self) {
        self.close();
    }
}

/// 只由真实 RPC 参数建立；请求关联串只用于关联，不作为批准或账号身份凭证。
/// 不实现 Clone/Serialize/Deserialize，实际宿主和运行实例均只保留弱引用。
pub(in crate::server::core::plugin_host) struct ConversationCall {
    preparation: PreparationRef,
    key_source: Option<Arc<crate::server::config::model_key_source::Source>>,
    output: Option<Arc<super::catpaw_turn::output::OutputSlot>>,
    // 锁顺序：Signal → Host生命周期 → Runtime只读 → Table → ScopeGate → Record → replay。
    signal: Arc<Signal>,
    egress: Result<Arc<ConversationEgress>, EgressError>,
    provider: String,
    account: String,
    request: String,
    capacity_caller: String,
    host: Weak<Host>,
    shared: Weak<Shared>,
    generation: u64,
    rpc: i64,
}

impl ConversationCall {
    pub(super) fn request_id(&self) -> &str { &self.request }

    pub(super) fn capacity_caller(&self) -> &str { &self.capacity_caller }

    pub(super) fn ended_token(&self) -> CancellationToken { self.signal.ended.clone() }

    pub(super) fn selected_path(&self, shared: &Arc<Shared>, account: &str) -> Result<SelectedPath, proto::WireError> {
        self.validate_context(shared, account)?;
        let egress = self.egress.as_ref().map_err(|_| unavailable())?;
        let selected = egress.selected.try_lock().map_err(|_| unavailable())?;
        selected.as_ref().ok_or_else(unavailable)?.selected_path().map_err(|_| unavailable())
    }

    pub(super) fn execution_guard(&self) -> Result<ExecutionGuard, proto::WireError> {
        let mut state = self.signal.state.lock().map_err(|_| unavailable())?;
        if state.guard_issued || state.consumer || state.execution.is_some() {
            return Err(unavailable());
        }
        state.guard_issued = true;
        Ok(ExecutionGuard {
            signal: self.signal.clone(),
            finished: false,
        })
    }

    pub(super) fn bind(
        mut self,
        host: &Arc<Host>,
        shared: &Arc<Shared>,
        rpc: i64,
    ) -> Result<Self, proto::WireError> {
        if self.rpc != 0
            || rpc <= 0
            || !self
                .host
                .upgrade()
                .is_some_and(|selected| Arc::ptr_eq(&selected, host))
            || shared.options.provider != self.provider
        {
            return Err(proto::WireError::with_code(
                "Wasm 对话执行体绑定无效",
                "forbidden",
            ));
        }
        let state = self.signal.state.lock().map_err(|_| unavailable())?;
        if !state.guard_issued || state.consumer || state.execution.is_some() {
            return Err(unavailable());
        }
        let lifecycle = host.lifecycle.lock().map_err(|_| unavailable())?;
        if !host.ready()
            || shared.cancel.is_cancelled()
            || !host.embedded.runtime.try_read().is_ok_and(|runtime| {
                runtime
                    .as_ref()
                    .is_some_and(|runtime| Arc::ptr_eq(&runtime.shared, shared))
            })
        {
            return Err(unavailable());
        }
        self.shared = Arc::downgrade(shared);
        self.generation = host.generation.load(Ordering::Acquire);
        self.rpc = rpc;
        drop(lifecycle);
        drop(state);
        Ok(self)
    }

    pub(super) fn validate_params(
        &self,
        shared: &Arc<Shared>,
        params: &Value,
    ) -> Result<(), proto::WireError> {
        if params["provider"].as_str() != Some(self.provider.as_str())
            || params["account"].as_str() != Some(self.account.as_str())
        {
            return Err(proto::WireError::with_code(
                "Wasm 对话参数与可信调用不一致",
                "forbidden",
            ));
        }
        self.validate_context(shared, &self.account)
    }

    pub(super) fn validate_context(
        &self,
        shared: &Arc<Shared>,
        account: &str,
    ) -> Result<(), proto::WireError> {
        let state = self.signal.state.lock().map_err(|_| unavailable())?;
        if !state.guard_issued
            || state.consumer
            || state.execution.is_some()
            || account != self.account
            || !self
                .shared
                .upgrade()
                .is_some_and(|bound| Arc::ptr_eq(&bound, shared))
        {
            return Err(unavailable());
        }
        self.with_current(|| Ok(())).map_err(|_| unavailable())
    }

    fn with_current<T>(
        &self,
        action: impl FnOnce() -> Result<T, ReplayError>,
    ) -> Result<T, ReplayError> {
        let host = self.host.upgrade().ok_or(ReplayError::Revoked)?;
        let shared = self.shared.upgrade().ok_or(ReplayError::Revoked)?;
        if self.rpc <= 0
            || self.generation == 0
            || !bounded(&self.request, 512)
            || super::super::registry::is_switching(&self.provider)
            || !super::super::serves_provider(&self.provider)
            || !super::super::host_for_provider(&self.provider)
                .is_some_and(|current| Arc::ptr_eq(&current, &host))
        {
            return Err(ReplayError::Revoked);
        }
        let _lifecycle = host
            .lifecycle
            .lock()
            .map_err(|_| ReplayError::Unavailable)?;
        let runtime = host
            .embedded
            .runtime
            .try_read()
            .map_err(|_| ReplayError::Unavailable)?;
        if !host.ready()
            || host.generation.load(Ordering::Acquire) != self.generation
            || shared.cancel.is_cancelled()
            || shared.options.provider != self.provider
            || !runtime
                .as_ref()
                .is_some_and(|runtime| Arc::ptr_eq(&runtime.shared, &shared))
        {
            return Err(ReplayError::Revoked);
        }
        action()
    }

    pub(super) async fn ended(&self) {
        self.signal.ended.cancelled().await;
    }
}

/// 在任务移交前创建；未来从未被 poll、排队取消、陷阱或提前返回也登记执行终态。
/// 仅保存结束信号，不持有调用租约、账号材料或执行体。
pub(super) struct ExecutionGuard {
    signal: Arc<Signal>,
    finished: bool,
}

impl ExecutionGuard {
    pub(super) fn matches(&self, call: &ConversationCall) -> bool {
        !self.finished && Arc::ptr_eq(&self.signal, &call.signal)
    }

    pub(super) fn finish(&mut self, returned: bool) {
        if !self.finished {
            self.signal.execution_ended(if returned {
                ExecutionEnd::Returned
            } else {
                ExecutionEnd::Interrupted
            });
            self.finished = true;
        }
    }
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        self.finish(false);
    }
}

fn output_unavailable() -> GatewayError {
    GatewayError::with_status(409, "CatPaw 原响应观察来源未确认，未切换调用或重放本次响应")
        .with_code("catpaw_output_unconfirmed")
}

fn bounded(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.chars().any(char::is_control)
}

fn unavailable() -> proto::WireError {
    proto::WireError::with_code("Wasm 对话调用已结束或执行体已切换", "request_call_revoked")
}
