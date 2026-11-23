//! CatPaw 普通对话的私有来源；不授予账号或网络权限，也不延长原调用租约。

use std::marker::PhantomData;
use std::sync::{atomic::Ordering, Arc, Weak};

use agent2api_provider_protocol as proto;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{bounded, unavailable, CatPawMaterial, ConversationEgress, EndState, Signal};
use crate::server::core::plugin_host::wasm::context::{Context, Shared};
use crate::server::core::plugin_host::{self as host, account_egress::EgressError, Host};
use crate::server::core::proxies::chain::SelectedPath;
use crate::server::core::upstream::replay_guard::PreparationRef;

struct Source {
    signal: Arc<Signal>,
    host: Weak<Host>,
    shared: Weak<Shared>,
    generation: u64,
    rpc: i64,
    provider: String,
    account: String,
    session: String,
    client_stream: bool,
    deadline: Instant,
    call_cancel: CancellationToken,
    preparation: PreparationRef,
    egress: Result<Weak<ConversationEgress>, EgressError>,
    material: Arc<CatPawMaterial>,
    key_source: Option<Arc<crate::server::config::model_key_source::Source>>,
    output: Weak<super::super::catpaw_turn::output::OutputSlot>,
    capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
}

#[derive(Clone)]
pub(in crate::server::core::plugin_host::wasm) struct SourceRef {
    source: Arc<Source>,
}

pub(in crate::server::core::plugin_host::wasm) struct SourceGate<'a> {
    source: &'a Source,
    state: &'a EndState,
    invariant: PhantomData<&'a mut &'a ()>,
    not_send: PhantomData<std::rc::Rc<()>>,
}

impl Context {
    pub(in crate::server::core::plugin_host::wasm) fn source_ref(&self) -> Result<SourceRef, proto::WireError> {
        let call = self.conversation.as_ref().ok_or_else(unavailable)?;
        if call.provider != "catpaw"
            || call.account.is_empty()
            || call.account != self.account
            || self.shared.options.provider != "catpaw"
            || self.shared.cancel.is_cancelled()
            || self.sink.as_ref().is_none_or(|sink| sink.cancel.is_cancelled())
            || !bounded(&self.session, 1024)
            || self.call_deadline <= Instant::now()
            || !std::ptr::eq(call.shared.as_ptr(), Arc::as_ptr(&self.shared))
        {
            return Err(unavailable());
        }
        let key_source = call.key_source.clone();
        if key_source.as_ref().is_some_and(|source| source.with_current(|| ()).is_none()) { return Err(unavailable()); }
        let client_stream = self.client_stream.ok_or_else(unavailable)?;
        let material = self.catpaw_material.as_ref()
            .filter(|material| material.matches_call(call)).cloned().ok_or_else(unavailable)?;
        let mut state = call.signal.state.try_lock().map_err(|_| unavailable())?;
        if state.consumer || state.execution.is_some() || !state.guard_issued
            || state.task_issued || state.source_issued || state.http_submission || state.record.is_some() {
            return Err(unavailable());
        }
        call.with_current(|| Ok(())).map_err(|_| unavailable())?;
        let source = Arc::new(Source {
            signal: call.signal.clone(), host: call.host.clone(), shared: call.shared.clone(),
            generation: call.generation, rpc: call.rpc, provider: call.provider.clone(), account: call.account.clone(),
            session: self.session.clone(), client_stream, deadline: self.call_deadline,
            call_cancel: self.sink.as_ref().ok_or_else(unavailable)?.cancel.clone(),
            preparation: call.preparation.clone(),
            egress: call.egress.as_ref().map(Arc::downgrade).map_err(|error| *error),
            material, key_source,
            output: call.output.as_ref().map(Arc::downgrade).ok_or_else(unavailable)?,
            capture: self.trace.as_str().and_then(host::script::capture_for_trace),
        });
        state.source_issued = true;
        Ok(SourceRef { source })
    }
}

impl SourceRef {
    pub(in crate::server::core::plugin_host::wasm) fn provider(&self) -> &str { &self.source.provider }
    pub(in crate::server::core::plugin_host::wasm) fn material(&self) -> &CatPawMaterial { self.source.material.as_ref() }
    pub(in crate::server::core::plugin_host::wasm) fn account(&self) -> &str { &self.source.account }
    pub(in crate::server::core::plugin_host::wasm) fn client_session(&self) -> &str { &self.source.session }
    pub(in crate::server::core::plugin_host::wasm) fn client_stream(&self) -> bool { self.source.client_stream }
    pub(in crate::server::core::plugin_host::wasm) fn deadline(&self) -> Instant { self.source.deadline }
    pub(in crate::server::core::plugin_host::wasm) fn ended_token(&self) -> CancellationToken { self.source.signal.ended.clone() }
    pub(in crate::server::core::plugin_host::wasm) fn capture(&self)
        -> Option<Arc<crate::server::core::debug_traffic::TrafficCapture>> { self.source.capture.clone() }

    pub(in crate::server::core::plugin_host::wasm) fn output_slot(&self)
        -> Result<Arc<super::super::catpaw_turn::output::OutputSlot>, proto::WireError> {
        self.source.output.upgrade().ok_or_else(unavailable)
    }

    pub(in crate::server::core::plugin_host::wasm) fn belongs_to_output(
        &self, output: &super::super::catpaw_turn::output::OutputSlot,
    ) -> bool { std::ptr::eq(self.source.output.as_ptr(), output) }

    pub(in crate::server::core::plugin_host::wasm) fn same_source(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
    }

    pub(in crate::server::core::plugin_host::wasm) fn same_session(&self, other: &Self) -> bool {
        self.source.session == other.source.session
            && Weak::ptr_eq(&self.source.host, &other.source.host)
            && Weak::ptr_eq(&self.source.shared, &other.source.shared)
            && self.source.generation == other.source.generation
            && self.source.provider == other.source.provider
            && self.source.account == other.source.account
            && self.source.session == other.source.session
            && self.source.material.model() == other.source.material.model()
            && self.source.material.parameters() == other.source.material.parameters()
            && self.source.key_source.as_ref().zip(other.source.key_source.as_ref())
                .is_some_and(|(original, current)| original.same(current))
    }

    pub(in crate::server::core::plugin_host::wasm) fn has_client_identity(&self) -> bool {
        self.source.key_source.as_ref().is_some_and(|source| source.is_current())
    }

    pub(in crate::server::core::plugin_host::wasm) fn belongs_to_host(&self, host: &Host) -> bool {
        std::ptr::eq(self.source.host.as_ptr(), host)
    }

    pub(in crate::server::core::plugin_host::wasm) fn check_current(&self) -> Result<(), proto::WireError> {
        self.with_submission_gate(|_| Ok(()))
    }

    pub(in crate::server::core::plugin_host::wasm) fn take_egress(&self) -> Result<SelectedPath, proto::WireError> {
        self.with_submission_gate(|_| {
            let egress = self.source.egress.as_ref().map_err(|_| unavailable())?
                .upgrade().ok_or_else(unavailable)?;
            if !egress.matches(&Arc::downgrade(&self.source.signal), self.provider(), self.account()) {
                return Err(unavailable());
            }
            egress.take_path().map_err(|_| unavailable())
        })
    }

    /// 回调只持原调用门同步核对账号并转换状态；不可等待、联网或反向进入调用门。
    pub(in crate::server::core::plugin_host::wasm) fn with_submission_gate<R>(
        &self,
        action: impl for<'a> FnOnce(&SourceGate<'a>) -> Result<R, proto::WireError>,
    ) -> Result<R, proto::WireError> {
        match &self.source.key_source {
            Some(source) => source.with_current(|| self.with_submission_gate_inner(action)).ok_or_else(unavailable)?,
            None => self.with_submission_gate_inner(action),
        }
    }

    fn with_submission_gate_inner<R>(
        &self,
        action: impl for<'a> FnOnce(&SourceGate<'a>) -> Result<R, proto::WireError>,
    ) -> Result<R, proto::WireError> {
        let state = self.source.signal.state.lock().map_err(|_| unavailable())?;
        if !state.guard_issued || !state.source_issued || state.consumer || state.execution.is_some()
            || state.task_issued || state.record.is_some() || self.source.deadline <= Instant::now()
            || self.source.call_cancel.is_cancelled() {
            return Err(unavailable());
        }
        let host = self.source.host.upgrade().ok_or_else(unavailable)?;
        let shared = self.source.shared.upgrade().ok_or_else(unavailable)?;
        if self.source.rpc <= 0 || self.source.generation == 0
            || host::runtime::admission_closed()
            || host::registry::is_switching(self.provider())
            || !host::serves_provider(self.provider())
            || !host::host_for_provider(self.provider()).is_some_and(|current| Arc::ptr_eq(&current, &host)) {
            return Err(unavailable());
        }
        let _lifecycle = host.lifecycle.try_lock().map_err(|_| unavailable())?;
        let runtime = host.embedded.runtime.try_read().map_err(|_| unavailable())?;
        if !host.ready() || host.activity.retired() || host.generation.load(Ordering::Acquire) != self.source.generation
            || shared.cancel.is_cancelled() || shared.options.provider != self.provider()
            || !runtime.as_ref().is_some_and(|runtime| Arc::ptr_eq(&runtime.shared, &shared)) {
            return Err(unavailable());
        }
        action(&SourceGate { source: &self.source, state: &state, invariant: PhantomData, not_send: PhantomData })
    }
}

impl SourceRef {
    pub(in crate::server::core::plugin_host::wasm) fn confirm_unsubmitted_rejection(&self) {
        self.source.preparation.confirm_unsubmitted_http_rejection();
    }

    pub(in crate::server::core::plugin_host::wasm) fn lock_permanent_replay(&self) {
        self.source.preparation.lock_permanent_replay_block();
    }
}

impl SourceGate<'_> {
    pub(in crate::server::core::plugin_host::wasm) fn freeze_replay(&self) -> Result<(), proto::WireError> {
        if self.state.consumer || self.state.execution.is_some() || self.source.deadline <= Instant::now()
            || self.source.call_cancel.is_cancelled() {
            return Err(unavailable());
        }
        self.source.preparation.freeze().map_err(|_| unavailable())
    }
}
