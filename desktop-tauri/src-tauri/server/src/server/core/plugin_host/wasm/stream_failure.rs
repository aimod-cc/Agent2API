//! WorkBuddy原流的同代分类器；只调用CLASSIFY，不ensure、不重选执行体、不请求对话。

use std::sync::{atomic::Ordering, Arc, Weak};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::json;
use tokio::time::Instant;

use super::{context::Shared, runtime::Runtime};
use super::super::{activity::UsageLease, Host};
use crate::server::core::{providers::adapter::UpstreamErrorClass, upstream::usage::RequestTelemetry};
use crate::server::errors::GatewayError;

pub(in crate::server::core::plugin_host) struct Classifier {
    host: Arc<Host>,
    runtime: Weak<Runtime>,
    shared: Weak<Shared>,
    generation: u64,
    _lease: Arc<UsageLease>,
}

struct Active(Arc<Host>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.embedded.active.fetch_sub(1, Ordering::AcqRel);
        self.0.embedded.idle.notify_waiters();
    }
}

impl Classifier {
    /// 只由实际开流入口传入即将执行的Runtime；不按provider重新选择，也不由RPC预先猜测。
    pub(super) fn capture(host: &Arc<Host>, runtime: &Arc<Runtime>, lease: Arc<UsageLease>) -> Result<Self, String> {
        let _state = host.lifecycle.try_lock().map_err(|_| unavailable())?;
        let current = host.embedded.runtime.try_read().map_err(|_| unavailable())?;
        if !host.ready() || runtime.shared.options.provider != "workbuddy"
            || runtime.shared.cancel.is_cancelled() || super::super::runtime::admission_closed()
            || !current.as_ref().is_some_and(|current| Arc::ptr_eq(current, runtime)) { return Err(unavailable()); }
        Ok(Self { host: host.clone(), runtime: Arc::downgrade(runtime), shared: Arc::downgrade(&runtime.shared),
            generation: host.generation.load(Ordering::Acquire), _lease: lease })
    }

    fn current(&self) -> Result<Arc<Runtime>, String> {
        if self.host.activity.retired() || super::super::registry::is_switching("workbuddy")
            || !self.host.callbacks.provider_allowed("workbuddy")
            || !super::super::host_for_provider("workbuddy").is_some_and(|host| Arc::ptr_eq(&host, &self.host)) {
            return Err(unavailable());
        }
        let _state = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
        let current = self.host.embedded.runtime.try_read().map_err(|_| unavailable())?;
        let runtime = current.as_ref().ok_or_else(unavailable)?;
        if !self.host.ready() || super::super::runtime::admission_closed()
            || self.host.generation.load(Ordering::Acquire) != self.generation
            || !std::ptr::eq(self.runtime.as_ptr(), Arc::as_ptr(runtime))
            || !std::ptr::eq(self.shared.as_ptr(), Arc::as_ptr(&runtime.shared))
            || runtime.shared.cancel.is_cancelled() { return Err(unavailable()); }
        Ok(runtime.clone())
    }

    pub(in crate::server::core::plugin_host) fn still_current(&self) -> bool { self.current().is_ok() }

    pub(in crate::server::core::plugin_host) async fn classify(
        &self, error: &GatewayError, telemetry: &RequestTelemetry, deadline: Instant,
    ) -> Result<UpstreamErrorClass, String> {
        if telemetry.is_cancelled() || Instant::now() >= deadline { return Err(unavailable()); }
        let runtime = self.current()?;
        let params = json!({ "provider": "workbuddy", "status": error.status_code,
            "body": super::super::errors::workbuddy_stream::detail(error) });
        super::managed::guard_ordinary(&runtime.shared.options, proto::methods::gateway::CLASSIFY, &params)
            .map_err(|_| unavailable())?;
        {
            let _state = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let current = self.host.embedded.runtime.try_read().map_err(|_| unavailable())?;
            if !self.host.ready() || runtime.shared.cancel.is_cancelled()
                || self.host.generation.load(Ordering::Acquire) != self.generation
                || !current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &runtime)) { return Err(unavailable()); }
            self.host.embedded.active.fetch_add(1, Ordering::AcqRel);
        }
        let _active = Active(self.host.clone());
        let cancel = telemetry.cancel_token();
        let cancelled = async { match cancel { Some(token) => token.cancelled().await, None => std::future::pending().await } };
        let result = tokio::select! {
            biased;
            _ = cancelled => return Err(unavailable()),
            result = tokio::time::timeout_at(deadline, runtime.invoke(proto::methods::gateway::CLASSIFY, params,
                deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(2)), None)) => result,
        }.map_err(|_| unavailable())?.map_err(|_| unavailable())?;
        if telemetry.is_cancelled() || Instant::now() >= deadline || !self.still_current() { return Err(unavailable()); }
        let value = super::errors::decode(result).map_err(|_| unavailable())?;
        let class: proto::ErrorClass = serde_json::from_value::<proto::ErrorClass>(value).map_err(|_| unavailable())?;
        Ok(match class {
            proto::ErrorClass::QuotaLimited { status, upstream_code, reset_at, message } =>
                UpstreamErrorClass::QuotaLimited { status, upstream_code, reset_at, message },
            proto::ErrorClass::TokenExpired { message } => UpstreamErrorClass::TokenExpired { message },
            proto::ErrorClass::ContentBlocked { status, upstream_code, message } =>
                UpstreamErrorClass::ContentBlocked { status, upstream_code, message },
            proto::ErrorClass::Fatal { status, upstream_code, message } =>
                UpstreamErrorClass::Fatal { status, upstream_code, message },
        })
    }
}

fn unavailable() -> String { "WorkBuddy流后错误分类未确认，未重启插件或猜测额度".into() }
