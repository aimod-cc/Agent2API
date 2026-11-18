//! 固定Free换证调用；原Pending与新凭据保存由同一后台所有者持有，取消不重发。

pub(super) mod protocol;
mod worker;

use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex, Weak};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{super::{account_egress::EgressSelection, activity::UsageLease, Host}, runtime::Runtime};
use crate::server::core::{account_store::credential_version::{FreeRefreshFinal, FreeRefreshPending, FreeRefreshOrigin},
    proxies::{self, chain::SelectedPath, ProxyResolution}};

const CAPABILITY: &str = "credential.cline-free-refresh.v1";

#[derive(PartialEq)]
enum Stage { Captured, Entered, Prepared, Sent, Saving }
struct State {
    stage: Stage,
    closed: bool,
    path_taken: bool,
    path_ready: bool,
    final_attempted: bool,
    save_attempted: bool,
    pending: Option<Weak<()>>,
    egress: Option<EgressSelection>,
}

pub(super) struct Call {
    host: Arc<Host>,
    runtime: Arc<Runtime>,
    usage: Option<Arc<UsageLease>>,
    active: bool,
    generation: u64,
    source_digest: [u8; 32],
    module_digest: String,
    account: String,
    deadline: Instant,
    closed: AtomicBool,
    ended: CancellationToken,
    state: Mutex<State>,
}

impl Call {
    pub(super) fn capture(host: &Arc<Host>, runtime: &Arc<Runtime>, account: &str, timeout: Duration,
        egress: Option<EgressSelection>,
    ) -> Result<Arc<Self>, proto::WireError> {
        if !host.ready() || host.options.script || runtime.shared.options.provider != "cline-free"
            || !runtime.shared.options.allows(CAPABILITY) || runtime.shared.cancel.is_cancelled()
            || account.is_empty() || account.len() > 512 || account.trim() != account || account.chars().any(char::is_control)
            || egress.as_ref().is_some_and(|egress| !egress.matches_account("cline-free", account)) {
            return Err(unavailable());
        }
        let usage = host.activity.acquire().ok_or_else(unavailable)?;
        let mut call = Self { host: host.clone(), runtime: runtime.clone(), usage: Some(usage), active: false,
            generation: host.generation.load(Ordering::Acquire), source_digest: *runtime.shared.options.source_digest(),
            module_digest: runtime.module_digest().to_string(), account: account.to_string(),
            deadline: Instant::now() + timeout,
            closed: AtomicBool::new(false), ended: CancellationToken::new(), state: Mutex::new(State {
                stage: Stage::Captured, closed: false, path_taken: false, path_ready: false,
                final_attempted: false, save_attempted: false, pending: None, egress,
            }) };
        call.gate(|_| {
            host.embedded.active.fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| active.checked_add(1))
                .map_err(|_| unavailable())?;
            Ok(())
        })?;
        call.active = true;
        Ok(Arc::new(call))
    }

    pub(super) fn enter(&self, runtime: &Runtime, method: &str, account: &str) -> Result<(), proto::WireError> {
        if !std::ptr::eq(self.runtime.as_ref(), runtime) || method != proto::methods::gateway::TOKEN_REFRESH || account != self.account {
            self.close(); return Err(unavailable());
        }
        self.gate(|state| {
            if state.stage != Stage::Captured { return Err(unavailable()); }
            state.stage = Stage::Entered;
            Ok(())
        })
    }

    fn prepare(&self) -> Result<(), proto::WireError> {
        self.gate(|state| {
            if state.stage != Stage::Entered { return Err(unavailable()); }
            state.stage = Stage::Prepared;
            Ok(())
        })
    }
    fn current(&self) -> Result<(), proto::WireError> { self.gate(|_| Ok(())) }

    fn take_path(&self, origin: &FreeRefreshOrigin) -> Result<SelectedPath, proto::WireError> {
        if origin.account() != self.account { self.close(); return Err(unavailable()); }
        let original = self.gate(|state| {
            if state.stage != Stage::Prepared || state.path_taken { return Err(unavailable()); }
            state.path_taken = true;
            Ok(state.egress.take())
        })?;
        let path = match original {
            Some(egress) => egress.into_path().map_err(|_| unavailable())?,
            None => match proxies::resolve_account_proxy(Some(origin.proxy())) {
                Some(ProxyResolution::Failed(_)) => { self.close(); return Err(unavailable()); },
                selected => proxies::chain::capture_selected(selected.and_then(|selected| match selected {
                    ProxyResolution::Resolved(proxy) => Some(proxy), ProxyResolution::Failed(_) => None,
                })).map_err(|_| unavailable())?,
            },
        };
        self.gate(|state| {
            if state.stage != Stage::Prepared || !state.path_taken || state.path_ready { return Err(unavailable()); }
            state.path_ready = true;
            Ok(path)
        })
    }

    fn authorize_send(&self, pending: &FreeRefreshPending) -> Result<(), proto::WireError> {
        if pending.account() != self.account { self.close(); return Err(unavailable()); }
        let check = pending.prepare_send().map_err(|_| unavailable())?;
        let mut binding = Some(pending.binding());
        let result = self.gate(|state| {
            if state.stage != Stage::Prepared || !state.path_ready || state.egress.is_some()
                || state.final_attempted || state.pending.is_some() { return Err(unavailable()); }
            state.final_attempted = true;
            state.pending = binding.take();
            Ok(check.run(|| {
                if !self.fresh() { return false; }
                state.stage = Stage::Sent;
                true
            }))
        }).and_then(|result| result.into_result().map_err(|_| unavailable()));
        if result.is_err() { self.close(); }
        result
    }

    fn complete(&self, pending: &FreeRefreshPending, secret: &Value, public: &Value) -> Result<(), proto::WireError> {
        if pending.account() != self.account { self.close(); return Err(unavailable()); }
        let check = pending.prepare_completion(secret, public).map_err(|_| unavailable())?;
        let result: Result<FreeRefreshFinal, _> = self.gate(|state| {
            if state.stage != Stage::Sent || !state.final_attempted || state.save_attempted
                || state.pending.as_ref().is_none_or(|binding| !pending.matches(binding)) { return Err(unavailable()); }
            state.save_attempted = true;
            Ok(check.run(|| {
                if !self.fresh() { return false; }
                state.stage = Stage::Saving;
                true
            }))
        });
        let result = result.and_then(|result| result.into_result().map_err(|_| unavailable()));
        if result.is_err() { self.close(); }
        result
    }

    fn fresh(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && !self.runtime.shared.cancel.is_cancelled()
            && self.deadline > Instant::now() && !self.host.closing.load(Ordering::Acquire)
            && self.host.generation.load(Ordering::Acquire) == self.generation && !super::super::runtime::admission_closed()
    }
    fn gate<R>(&self, consume: impl FnOnce(&mut State) -> Result<R, proto::WireError>) -> Result<R, proto::WireError> {
        let result = (|| {
            if !self.fresh() || self.host.activity.retired() { return Err(unavailable()); }
            let _life = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let current = self.host.embedded.runtime.try_read().map_err(|_| unavailable())?;
            let mut state = self.state.try_lock().map_err(|_| unavailable())?;
            let options = self.host.options.wasm.as_ref().ok_or_else(unavailable)?;
            if !self.fresh() || !self.host.ready() || self.generation == 0 || state.closed
                || options.provider != "cline-free" || !options.allows(CAPABILITY) || options.source_digest() != &self.source_digest
                || self.runtime.shared.options.source_digest() != &self.source_digest || self.runtime.module_digest() != self.module_digest
                || !current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &self.runtime)) { return Err(unavailable()); }
            consume(&mut state)
        })();
        if result.is_err() { self.close(); }
        result
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.state.lock().unwrap_or_else(|error| error.into_inner()).closed = true;
        self.ended.cancel();
    }
    async fn closed(&self) {
        if !self.fresh() { self.close(); return; }
        tokio::select! {
            biased;
            _ = self.ended.cancelled() => {},
            _ = self.runtime.shared.cancel.cancelled() => {},
            _ = tokio::time::sleep_until(self.deadline) => {},
        }
        self.close();
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.close();
        drop(self.usage.take());
        if self.active { self.host.embedded.active.fetch_sub(1, Ordering::AcqRel); }
        self.host.embedded.idle.notify_waiters();
    }
}

pub(super) async fn exchange(call: Arc<Call>) -> Result<Value, proto::WireError> { worker::exchange(call).await }

pub(super) fn release_ready(host: &Host) -> bool { host.embedded.active.load(Ordering::Acquire) == 0 }

fn unavailable() -> proto::WireError {
    proto::WireError::with_code("Cline Free原续期、待确认状态或新凭据保存未确认；未重发或读取其他额度池", "cline_free_refresh_unconfirmed")
}
