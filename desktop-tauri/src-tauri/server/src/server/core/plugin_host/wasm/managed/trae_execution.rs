use std::sync::{atomic::Ordering, Arc};

use super::super::{context::Context, runtime::Runtime};
use super::{current, unavailable, Active};
use crate::server::core::login::managed::{Call, TraeCallSource};
use crate::server::core::plugin_host::{activity::UsageLease, Host};
use crate::server::errors::GatewayError;

pub(crate) struct TraeExecution {
    original: TraeCallSource,
    host: Arc<Host>,
    runtime: Arc<Runtime>,
    generation: u64,
    _active: Active,
    _usage: Arc<UsageLease>,
}

fn context_runtime(
    context: &Context,
    call: &Call,
) -> Result<(Arc<Host>, Arc<Runtime>, u64), GatewayError> {
    call.validate("trae")?;
    if context.shared.options.provider != "trae"
        || call.method_index() != 0
        || context.flow != call.flow()
        || context
            .managed
            .as_ref()
            .is_none_or(|original| original.flow() != call.flow())
    {
        return Err(unavailable());
    }
    let host = super::super::super::host_for_provider("trae").ok_or_else(unavailable)?;
    let (runtime, generation) = {
        let _lifecycle = host.lifecycle.try_lock().map_err(|_| unavailable())?;
        let slot = host
            .embedded
            .runtime
            .try_read()
            .map_err(|_| unavailable())?;
        let runtime = slot.as_ref().ok_or_else(unavailable)?;
        let generation = host.generation.load(Ordering::Acquire);
        if !current(&host, "trae", generation)
            || super::super::super::runtime::admission_closed()
            || !Arc::ptr_eq(&context.shared, &runtime.shared)
            || runtime.shared.cancel.is_cancelled()
            || !runtime.shared.options.allows("login.managed.v1")
            || !runtime.shared.options.allows("login.loopback.v1")
            || runtime
                .shared
                .options
                .manifest
                .managed_login
                .as_ref()
                .is_none_or(|declaration| !declaration.methods.contains(&0))
        {
            return Err(unavailable());
        }
        (runtime.clone(), generation)
    };
    call.runtime_matches(Arc::as_ptr(&runtime.shared) as usize, generation)?;
    Ok((host, runtime, generation))
}

pub(in crate::server::core::plugin_host::wasm) fn validate_context(
    context: &Context,
    call: &Call,
) -> Result<(), GatewayError> {
    context_runtime(context, call).map(|_| ())
}

impl TraeExecution {
    pub(super) fn capture(context: &Context, call: &Call) -> Result<Arc<Self>, GatewayError> {
        let original = call.trae_call_source()?;
        if context
            .managed
            .as_ref()
            .is_none_or(|call| !original.matches(call))
        {
            return Err(unavailable());
        }
        let (host, runtime, generation) = context_runtime(context, call)?;
        let usage = host.activity.acquire().ok_or_else(unavailable)?;
        {
            let _lifecycle = host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let slot = host
                .embedded
                .runtime
                .try_read()
                .map_err(|_| unavailable())?;
            if !current(&host, "trae", generation)
                || super::super::super::runtime::admission_closed()
                || runtime.shared.cancel.is_cancelled()
                || !slot
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &runtime))
            {
                return Err(unavailable());
            }
            host.embedded.active.fetch_add(1, Ordering::AcqRel);
        }
        let active = Active(host.clone());
        let execution = Arc::new(Self {
            original,
            host,
            runtime,
            generation,
            _active: active,
            _usage: usage,
        });
        execution.validate(call)?;
        Ok(execution)
    }

    pub(crate) fn validate(&self, call: &Call) -> Result<(), GatewayError> {
        if !self.original.matches(call) {
            return Err(unavailable());
        }
        call.runtime_matches(self.identity(), self.generation)?;
        let _lifecycle = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
        let slot = self
            .host
            .embedded
            .runtime
            .try_read()
            .map_err(|_| unavailable())?;
        if !current(&self.host, "trae", self.generation)
            || super::super::super::runtime::admission_closed()
            || self.runtime.shared.cancel.is_cancelled()
            || !slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &self.runtime))
        {
            return Err(unavailable());
        }
        Ok(())
    }

    /// action只允许原本机会话→Host→Session短同步消费；不得等待、联网或递归来源核对。
    pub(crate) fn with_current<R>(
        &self,
        call: &Call,
        action: impl FnOnce() -> Result<R, GatewayError>,
    ) -> Result<R, GatewayError> {
        if !self.original.matches(call) {
            return Err(unavailable());
        }
        call.runtime_matches(self.identity(), self.generation)?;
        call.with_trae_login_source(|| {
            let _lifecycle = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let slot = self.host.embedded.runtime.try_read().map_err(|_| unavailable())?;
            if !current(&self.host, "trae", self.generation)
                || super::super::super::runtime::admission_closed()
                || self.runtime.shared.cancel.is_cancelled()
                || !slot.as_ref().is_some_and(|current| Arc::ptr_eq(current, &self.runtime)) {
                return Err(unavailable());
            }
            action()
        })
    }

    pub(crate) fn host(&self) -> &Arc<Host> {
        &self.host
    }
    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.runtime.shared) as usize
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}
