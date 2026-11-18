//! 固定豆包其他任务领奖的原Host入口；普通插件HTTP不能替代耐久Pending或真人操作。

mod protocol;
mod worker;

use std::{sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex}, time::Duration};

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{runtime::Runtime, super::{Host, activity::UsageLease}};

const CAPABILITY: &str = "claim.doubao-tasks.v1";
struct State { entered: bool, used: bool, closed: bool, sequence: u64, saving: bool }
pub(super) struct Call {
    host: Arc<Host>, runtime: Arc<Runtime>, generation: u64, account: String, campaign: String, deadline: Instant,
    digest: [u8; 32], module: String, state: Mutex<State>, ended: CancellationToken,
    closed: AtomicBool, _usage: Arc<UsageLease>, active: bool,
}
impl Call {
    pub(super) fn capture(host: &Arc<Host>, runtime: &Arc<Runtime>, account: &str, campaign: &str, timeout: Duration) -> Result<Arc<Self>, proto::WireError> {
        if account.is_empty() || account.len() > 512 || account.trim() != account || account.chars().any(char::is_control)
            || runtime.shared.options.provider != "doubaowork" || !runtime.shared.options.allows(CAPABILITY)
            || (!campaign.is_empty() && (!campaign.starts_with("doubao-task-") || campaign.len() != 76
                || !campaign[12..].bytes().all(|byte| byte.is_ascii_hexdigit()))) {
            return Err(unavailable());
        }
        let usage = host.activity.acquire().ok_or_else(unavailable)?;
        let mut call = Self { host: host.clone(), runtime: runtime.clone(), generation: host.generation.load(Ordering::Acquire),
            account: account.to_string(), campaign: campaign.to_string(), deadline: Instant::now() + timeout, digest: *runtime.shared.options.source_digest(),
            module: runtime.module_digest().to_string(), state: Mutex::new(State { entered: false, used: false, closed: false, sequence: 0, saving: false }),
            ended: CancellationToken::new(), closed: AtomicBool::new(false), _usage: usage, active: false };
        call.gate(|_| {
            host.embedded.active.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| value.checked_add(1)).map_err(|_| unavailable())?;
            Ok(())
        })?;
        call.active = true;
        Ok(Arc::new(call))
    }
    pub(super) fn enter(&self, runtime: &Runtime, method: &str, account: &str) -> Result<(), proto::WireError> {
        if !std::ptr::eq(self.runtime.as_ref(), runtime)
            || method != (if self.campaign.is_empty() { proto::methods::gateway::CLAIM_STATUS } else { proto::methods::gateway::CLAIM })
            || account != self.account {
            self.close(); return Err(unavailable());
        }
        self.gate(|state| { if state.entered { return Err(unavailable()); } state.entered = true; Ok(()) })
    }
    fn prepare(&self) -> Result<(), proto::WireError> {
        self.gate(|state| { if !state.entered || state.used { return Err(unavailable()); } state.used = true; Ok(()) })
    }
    fn fresh(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && !self.runtime.shared.cancel.is_cancelled() && self.deadline > Instant::now()
            && !self.host.closing.load(Ordering::Acquire) && !super::super::runtime::admission_closed()
            && self.host.generation.load(Ordering::Acquire) == self.generation
    }
    fn gate<R>(&self, action: impl FnOnce(&mut State) -> Result<R, proto::WireError>) -> Result<R, proto::WireError> {
        let result = (|| {
            if !self.fresh() || self.host.activity.retired() { return Err(unavailable()); }
            let _life = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let current = self.host.embedded.runtime.try_read().map_err(|_| unavailable())?;
            let mut state = self.state.try_lock().map_err(|_| unavailable())?;
            let options = self.host.options.wasm.as_ref().ok_or_else(unavailable)?;
            if !self.fresh() || !self.host.ready() || state.closed || self.generation == 0
                || options.provider != "doubaowork" || !options.allows(CAPABILITY) || options.source_digest() != &self.digest
                || self.runtime.shared.options.source_digest() != &self.digest || self.runtime.module_digest() != self.module
                || !current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &self.runtime)) { return Err(unavailable()); }
            action(&mut state)
        })();
        if result.is_err() { self.close(); }
        result
    }
    fn current(&self) -> Result<(), proto::WireError> { self.gate(|_| Ok(())) }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.state.lock().unwrap_or_else(|error| error.into_inner()).closed = true;
        self.ended.cancel();
    }
    async fn ended(&self) {
        if !self.fresh() { self.close(); return; }
        tokio::select! {
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
        if self.active { self.host.embedded.active.fetch_sub(1, Ordering::AcqRel); }
        self.host.embedded.idle.notify_waiters();
    }
}

pub(super) async fn exchange(call: Arc<Call>) -> Result<Value, proto::WireError> { worker::exchange(call).await }
pub(super) fn release_ready(host: &Host) -> bool { host.embedded.active.load(Ordering::Acquire) == 0 }
fn unavailable() -> proto::WireError {
    proto::WireError::with_code("豆包工作原任务领奖来源、待确认保存或官方回查未确认；未重发或代做任务", "doubao_tasks_unconfirmed")
}
