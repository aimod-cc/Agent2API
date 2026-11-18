//! 原签到调用只签发一份固定豆包账号出口；只接真实Empty快照签出的私有来源票据。
//! DailySource与SnapshotReceipt内部保持同一个Arc<RowSource>，不另prepare或按账号文字拼源。

use std::sync::{atomic::Ordering, Arc, OnceLock, Weak};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use tokio::{sync::Semaphore, time::Instant};

use super::{CheckinCallRef, Source};
use crate::server::core::{
    account_store::credential_version::{doubao_state::{DailySource, SnapshotReceipt}, RowProvider, RowSource},
    egress::managed::FrozenHttpClient,
    proxies::{self, chain, ProxyResolution},
};

#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct DailyEgress {
    source: Weak<Source>,
    account: DailySource,
    client: FrozenHttpClient,
    deadline: Instant,
}

impl DailyEgress {
    pub(in crate::server::core::plugin_host::wasm) fn matches(&self, call: &CheckinCallRef) -> bool {
        Weak::ptr_eq(&self.source, &Arc::downgrade(&call.source))
            && self.account().provider() == RowProvider::DoubaoWork
            && self.account().account() == call.source.account
            && self.deadline == call.source.deadline
    }
    pub(in crate::server::core::plugin_host::wasm::checkin_call) fn matches_snapshot(&self, snapshot: &SnapshotReceipt) -> bool {
        snapshot.matches_daily_source(&self.account)
    }
    pub(in crate::server::core::plugin_host::wasm) fn account(&self) -> &RowSource { self.account.account() }
    pub(in crate::server::core::plugin_host::wasm) fn client(&self) -> &FrozenHttpClient { &self.client }
}

impl CheckinCallRef {
    /// 只接真实快照，票据按值来自它的一次Empty槽；不接受独立RowSource拼配。
    pub(in crate::server::core::plugin_host::wasm) async fn take_daily_egress(
        &self, snapshot: &SnapshotReceipt,
    ) -> Result<DailyEgress, proto::WireError> {
        if self.source.provider != "doubaowork" || !snapshot.matches_doubao_account(&self.source.account) {
            return Err(unavailable());
        }
        let account;
        {
            let mut state = self.source.signal.state.try_lock().map_err(|_| unavailable())?;
            if state.daily_egress_issued { return Err(unavailable()); }
            self.check_daily_runtime(&state)?;
            account = snapshot.take_daily_source().map_err(|_| unavailable())?;
            state.daily_egress_issued = true;
        }
        let permit = slots().try_acquire_owned().map_err(|_| unavailable())?;
        let prepared_deadline = self.source.deadline.min(Instant::now() + Duration::from_secs(5));
        let call = self.clone();
        let work = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            call.check_daily_current()?;
            if Instant::now() >= prepared_deadline { return Err(unavailable()); }
            let config = account.account().original_doubao_proxy().map_err(|_| unavailable())?;
            // 原账号与Db锁已经退出，才读取一次选定代理/上游；不反向持库锁进入配置。
            let config = config.get("config").filter(|value| value.is_object()).unwrap_or(&config);
            let selected = match proxies::resolve_account_proxy_rotating(Some(config), account.account().account()) {
                None if config.is_null() || config["source"] == "direct" => None,
                Some(ProxyResolution::Resolved(proxy)) => Some(proxy),
                _ => return Err(unavailable()),
            };
            let path = chain::capture_selected(selected).map_err(|_| unavailable())?;
            call.check_daily_current()?;
            if Instant::now() >= prepared_deadline { return Err(unavailable()); }
            Ok::<_, proto::WireError>((account, path))
        });
        let (account, path) = tokio::select! {
            biased;
            _ = self.source.signal.ended.cancelled() => return Err(unavailable()),
            _ = self.source.runtime_cancel.cancelled() => return Err(unavailable()),
            result = tokio::time::timeout_at(prepared_deadline, work) => result
                .map_err(|_| unavailable())?.map_err(|_| unavailable())??,
        };
        let client = tokio::select! {
            biased;
            _ = self.source.signal.ended.cancelled() => return Err(unavailable()),
            _ = self.source.runtime_cancel.cancelled() => return Err(unavailable()),
            result = tokio::time::timeout_at(prepared_deadline, FrozenHttpClient::from_path(path)) => result
                .map_err(|_| unavailable())?.map_err(|_| unavailable())?,
        };
        self.check_daily_current()?;
        if Instant::now() >= prepared_deadline { return Err(unavailable()); }
        Ok(DailyEgress { source: Arc::downgrade(&self.source), account, client, deadline: self.source.deadline })
    }

    fn check_daily_current(&self) -> Result<(), proto::WireError> {
        let state = self.source.signal.state.try_lock().map_err(|_| unavailable())?;
        self.check_daily_runtime(&state)
    }

    fn check_daily_runtime(&self, state: &super::State) -> Result<(), proto::WireError> {
        if !state.bound || !state.entered || !state.context_entered || state.execution.is_some()
            || state.reason.is_some() || self.source.signal.ended.is_cancelled()
            || Instant::now() >= self.source.deadline || self.source.runtime_cancel.is_cancelled() {
            return Err(unavailable());
        }
        let host = self.source.host.upgrade().ok_or_else(unavailable)?;
        let _lifecycle = host.lifecycle.try_lock().map_err(|_| unavailable())?;
        let current = host.embedded.runtime.try_read().map_err(|_| unavailable())?;
        if !host.ready() || host.activity.retired() || super::super::super::runtime::admission_closed()
            || host.generation.load(Ordering::Acquire) != self.source.generation
            || !current.as_ref().is_some_and(|runtime| self.matches_runtime(runtime)
                && runtime.shared.options.allows("http.v1")) {
            return Err(unavailable());
        }
        Ok(())
    }
}

fn slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4))).clone()
}

fn unavailable() -> proto::WireError {
    proto::WireError::with_code("豆包工作原账号和出口来源未完成一次绑定，未查询、重选代理或恢复调用", "doubao_daily_egress_unavailable")
}
