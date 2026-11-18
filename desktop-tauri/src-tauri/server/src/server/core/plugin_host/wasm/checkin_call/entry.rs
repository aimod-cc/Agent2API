//! 原签到等待端到一次实际执行的窄入口；ensure一次，绑定之后不再重选Runtime。

use std::future::Future;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio::time::Instant;

use super::{CallResult, CheckinCallRef, EndReason, ExecutionGuard, Signal, Source, invalid};
use super::super::runtime::Runtime;
use super::super::super::{activity::UsageLease, errors::strict, rpc::rpc_error, Host};
use crate::server::errors::GatewayError;

/// 只包含原等待端的意图；尚未绑定实际WASM Host/Runtime，不能放入Context。
#[must_use]
pub(in crate::server::core::plugin_host) struct Pending {
    signal: Arc<Signal>,
    provider: String,
    account: String,
    deadline: Instant,
}

impl Pending {
    pub(super) fn new(signal: Arc<Signal>, provider: &str, account: &str, timeout: Duration) -> Self {
        Self { signal, provider: provider.to_string(), account: account.to_string(), deadline: Instant::now() + timeout }
    }

    /// 执行守卫在返回future之前建立，ensure失败、未poll或spawn前drop均留下终态。
    pub(in crate::server::core::plugin_host) fn run(
        self, host: Arc<Host>, lease: Arc<UsageLease>, params: Value, timeout: Duration,
    ) -> impl Future<Output = Result<CallResult, GatewayError>> {
        let mut ended = ExecutionGuard::new(self.signal.clone());
        async move {
            let provider = self.provider.clone();
            let prepare = async {
                if host.options.script || host.options.wasm.as_ref().is_none_or(|options| options.provider != self.provider)
                    || !valid(&self.provider, 128) || !valid(&self.account, 512)
                    || params["provider"].as_str() != Some(self.provider.as_str())
                    || params["account"].as_str() != Some(self.account.as_str()) {
                    return Err(rpc_error(&provider, invalid(), proto::methods::gateway::CHECKIN_CLAIM));
                }
                // 出口参数已在原RPC等待端按network路径捕获；本处不再重读出口。
                if let Err(error) = host.ensure().await {
                    ended.finish(EndReason::EnsureFailed);
                    return Err(if strict::applies(&provider) {
                        strict::fatal(&provider, 503, "runtime_transport_unavailable")
                    } else { GatewayError::with_status(503, format!("提供商插件不可用：{error}")) });
                }
                self.bind(&host, &params)
                    .map_err(|error| rpc_error(&provider, error, proto::methods::gateway::CHECKIN_CLAIM))
            }.await;
            let (runtime, call) = match prepare {
                Ok(bound) => bound,
                Err(error) => { ended.finish(EndReason::PreparationFailed); return Err(error); }
            };
            // Guard与绑定好的实际Runtime先装进future，后台尚未poll就被销毁也能关闭来源。
            let prepared = Prepared { runtime, call, ended, params };
            let work = async move {
                let _lease = lease;
                host.call_wasm_checkin(prepared, timeout).await
                    .map_err(|error| rpc_error(&provider, error, proto::methods::gateway::CHECKIN_CLAIM))
            };
            // 不把等待端的取消信号接到这个任务的abort上：已发出的普通HTTP仍按原行为收尾。
            // self的未绑定意图不留在等待端；这里只等待同一个已装配工作项。
            drop(self);
            crate::spawn_task(work).await
                .map_err(|_| GatewayError::with_status(503, "插件调用任务中断"))?
        }
    }

    fn bind(&self, host: &Arc<Host>, params: &Value) -> Result<(Arc<Runtime>, CheckinCallRef), proto::WireError> {
        let mut state = self.signal.state.lock().map_err(|_| invalid())?;
        if state.bound || state.execution.is_some() { return Err(invalid()); }
        // 不因取消或本来源截止到期中断旧调用，但标记一旦关闭就不能重新变为有效。
        if Instant::now() >= self.deadline {
            state.reason.get_or_insert(EndReason::Timeout);
            self.signal.ended.cancel();
        }
        let _lifecycle = host.lifecycle.lock().map_err(|_| invalid())?;
        let current = host.embedded.runtime.try_read().map_err(|_| invalid())?;
        let runtime = current.as_ref().filter(|runtime| host.ready()
            && !super::super::super::runtime::admission_closed()
            && !runtime.shared.cancel.is_cancelled()
            && runtime.shared.options.provider == self.provider).ok_or_else(invalid)?.clone();
        let generation = host.generation.load(Ordering::Acquire);
        if generation == 0 || params["account"].as_str() != Some(self.account.as_str()) { return Err(invalid()); }
        state.bound = true;
        let source = Source { signal: self.signal.clone(), host: Arc::downgrade(host),
            runtime: Arc::downgrade(&runtime), shared: Arc::downgrade(&runtime.shared),
            runtime_cancel: runtime.shared.cancel.clone(), generation,
            provider: self.provider.clone(), account: self.account.clone(), deadline: self.deadline };
        Ok((runtime, CheckinCallRef { source: Arc::new(source) }))
    }
}

/// 输入、实际执行体和执行guard一起移交，无构造/Clone/serde出口。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct Prepared {
    // 未poll时先释放输入，再记录执行终止；不依赖WASM侧析构。
    params: Value,
    runtime: Arc<Runtime>,
    call: CheckinCallRef,
    ended: ExecutionGuard,
}

pub(in crate::server::core::plugin_host::wasm) struct Entered {
    pub runtime: Arc<Runtime>,
    pub call: CheckinCallRef,
    pub ended: ExecutionGuard,
    pub params: Value,
}

impl Prepared {
    /// 原Host内的同步一次登记。调用者成功后立刻建立Active守卫，再开始任何await。
    pub(in crate::server::core::plugin_host::wasm) fn enter(self, host: &Host) -> Result<Entered, proto::WireError> {
        let source = &self.call.source;
        let mut state = source.signal.state.lock().map_err(|_| invalid())?;
        if !std::ptr::eq(source.host.as_ptr(), host) || !state.bound || state.entered || state.execution.is_some() {
            return Err(invalid());
        }
        let _lifecycle = host.lifecycle.lock().map_err(|_| invalid())?;
        let current = host.embedded.runtime.try_read().map_err(|_| invalid())?;
        if !host.ready() || super::super::super::runtime::admission_closed() {
            state.reason.get_or_insert(EndReason::HostClosed);
            source.signal.ended.cancel();
            return Err(invalid());
        }
        if host.generation.load(Ordering::Acquire) != source.generation
            || !current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &self.runtime))
            || !self.call.matches_runtime(&self.runtime) {
            state.reason.get_or_insert(EndReason::RuntimeReplaced);
            source.signal.ended.cancel();
            return Err(invalid());
        }
        if source.runtime_cancel.is_cancelled() {
            state.reason.get_or_insert(EndReason::RuntimeCancelled);
            source.signal.ended.cancel();
            return Err(invalid());
        }
        if Instant::now() >= source.deadline {
            state.reason.get_or_insert(EndReason::Timeout);
            source.signal.ended.cancel();
        }
        state.entered = true;
        host.embedded.active.fetch_add(1, Ordering::AcqRel);
        drop(current);
        drop(_lifecycle);
        drop(state);
        Ok(Entered { runtime: self.runtime, call: self.call, ended: self.ended, params: self.params })
    }
}

fn valid(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
