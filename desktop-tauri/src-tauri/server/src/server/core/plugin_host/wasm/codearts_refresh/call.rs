//! CodeArts原token.refresh调用的内存来源和强运行占用；不读账号、协议或严格Ready槽。
//! 仅Host现有Runtime入口捕获；请求/账号/路径字符串或JSON不能重建调用与出口来源。
//! 本Call的CAS不证明持久Pending或凭据保存完成，更不承诺跨资源原子exactly-once。

use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex};

use agent2api_provider_protocol as proto;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::server::core::{
    plugin_host::{self as host, account_egress::EgressSelection, activity::UsageLease, Host},
    proxies::{self, chain::SelectedPath, ProxyResolution},
};
use crate::server::core::account_store::credential_version::{
    ReadyRead, PendingSlot, PendingBinding, PendingCompletion, PendingCompletionCheck,
    PendingFinalCallback, PendingFinalState, PendingCommitFact,
};
use super::super::{context::Shared, runtime::Runtime};

const PROVIDER: &str = "codearts";
const ENTRY: &str = proto::methods::gateway::TOKEN_REFRESH;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage { Captured, Entered, Prepared, Sent, Saving }

struct State {
    stage: Stage,
    closed: bool,
    path_taken: bool,
    path_ready: bool,
    final_attempted: bool,
    save_attempted: bool,
    pending: Option<PendingBinding>,
    egress: Option<EgressSelection>,
}

struct Signal {
    state: Mutex<State>,
    closed: AtomicBool,
    ended: CancellationToken,
}

impl Signal {
    fn close(&self) {
        {
            // close只取得最后一级state锁，不反向取得Host生命周期或Runtime锁。
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.closed = true;
            self.closed.store(true, Ordering::Release);
        }
        // 唤醒在锁外进行；关闭不退还prepare/enter/send机会，也不撤回已发网络请求。
        self.ended.cancel();
    }
}

/// 无Clone/serde/公开字段；Arc克隆只延长原强运行占用，不能产生第二次入场或发送。
/// 唯一构造只借Host已选定的真实Runtime；不ensure、不全局查Host、目录或当前凭据。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct Call {
    host: Arc<Host>,
    runtime: Option<Arc<Runtime>>,
    shared: Option<Arc<Shared>>,
    usage: Option<Arc<UsageLease>>,
    active: bool,
    generation: u64,
    source_digest: [u8; 32],
    module_digest: String,
    account: String,
    deadline: Instant,
    original_cancel: CancellationToken,
    signal: Signal,
}

impl Call {
    /// 只供原Host入口：method必须是proto真实token.refresh，不接受token.ensure或任意方法。
    /// egress有值时必须来自原请求；后台维护只使用原Ready事务取得的账号出口配置。
    pub(in crate::server::core::plugin_host::wasm) fn capture(
        host: &Arc<Host>, runtime: &Arc<Runtime>, method: &str, account: &str, deadline: Instant,
        egress: Option<EgressSelection>,
    ) -> Result<Arc<Self>, proto::WireError> {
        if method != ENTRY || !valid_account(account) || deadline <= Instant::now()
            || host.options.script || runtime.shared.options.provider != PROVIDER
            || host.options.wasm.as_ref().is_none_or(|options|
                options.provider != PROVIDER || !options.allows("credential.codearts-refresh.v1"))
            || host::runtime::admission_closed() || !host.ready() || runtime.shared.cancel.is_cancelled() {
            return Err(unavailable());
        }
        if egress.as_ref().is_some_and(|egress| !egress.matches_account(PROVIDER, account)) {
            return Err(egress_unavailable());
        }
        let source_digest = *runtime.shared.options.source_digest();
        let module_digest = runtime.module_digest().to_string();
        if !valid_digest(&module_digest) { return Err(unavailable()); }
        // Activity→Host生命周期的顺序沿既有关闭链；不能持life再调用acquire或retired。
        let usage = host.activity.acquire().ok_or_else(unavailable)?;
        let mut call = Self {
            host: host.clone(), runtime: Some(runtime.clone()), shared: Some(runtime.shared.clone()),
            usage: Some(usage), active: false, generation: host.generation.load(Ordering::Acquire),
            source_digest, module_digest, account: account.to_string(), deadline,
            original_cancel: runtime.shared.cancel.clone(),
            signal: Signal { state: Mutex::new(State {
                stage: Stage::Captured, closed: false, path_taken: false, path_ready: false,
                final_attempted: false, save_attempted: false, pending: None, egress,
            }), closed: AtomicBool::new(false), ended: CancellationToken::new() },
        };
        // Scope内不做SQL/IO/网络/spawn；所有失败后的usage/Runtime析构均在门退出后。
        {
            let _lifecycle = host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let current = host.embedded.runtime.try_read().map_err(|_| unavailable())?;
            let state = call.signal.state.try_lock().map_err(|_| unavailable())?;
            call.check_source(current.as_ref(), &state)?;
            host.embedded.active.fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| active.checked_add(1))
                .map_err(|_| unavailable())?;
        }
        call.active = true;
        Ok(Arc::new(call))
    }

    pub(super) fn deadline(&self) -> Instant { self.deadline }
    pub(super) fn account(&self) -> &str { &self.account }
    pub(super) fn host(&self) -> &Arc<Host> { &self.host }
    pub(super) fn belongs_to_host(&self, host: &Host) -> bool { std::ptr::eq(self.host.as_ref(), host) }

    /// 原Runtime真正装配时消费唯一入场；仅同一实例/固定方法/原目标账号可进入。
    pub(in crate::server::core::plugin_host::wasm) fn enter_once(
        &self, runtime: &Runtime, method: &str, account: &str,
    ) -> Result<(), proto::WireError> {
        if method != ENTRY || account != self.account || self.runtime.as_ref()
            .is_none_or(|original| !std::ptr::eq(original.as_ref(), runtime)) {
            self.signal.close();
            return Err(unavailable());
        }
        self.gate(|state| {
            if state.stage != Stage::Captured { return Err(consumed()); }
            state.stage = Stage::Entered;
            Ok(())
        })
    }

    /// worker准备前消费一次；不是Ready、Pending、账号/网络授权或可缓存的发送许可。
    pub(super) fn prepare_once(&self) -> Result<(), proto::WireError> {
        self.gate(|state| {
            if state.stage != Stage::Entered { return Err(consumed()); }
            state.stage = Stage::Prepared;
            Ok(())
        })
    }

    /// 只消费原真实EgressSelection；与其他CAS共用life→runtime→state门，不能二取。
    /// 成功返回的完整路径只供同一调用锁外冻结，不返回自由Client或重新读取代理配置。
    pub(super) fn take_path(&self, ready: &ReadyRead) -> Result<SelectedPath, proto::WireError> {
        if ready.borrow_stamp().account() != self.account {
            self.signal.close();
            return Err(egress_unavailable());
        }
        let original = self.gate(|state| {
            if state.stage != Stage::Prepared || state.path_taken { return Err(consumed()); }
            state.path_taken = true;
            Ok(state.egress.take())
        })?;
        let path = match original {
            Some(egress) if egress.matches_account(PROVIDER, &self.account) =>
                egress.into_path().map_err(|_| egress_unavailable()),
            Some(_) => Err(egress_unavailable()),
            None => match proxies::resolve_account_proxy(Some(ready.original_proxy())) {
                Some(ProxyResolution::Failed(_)) => Err(egress_unavailable()),
                selected => proxies::chain::capture_selected(selected.and_then(|selected| match selected {
                    ProxyResolution::Resolved(proxy) => Some(proxy),
                    ProxyResolution::Failed(_) => None,
                })).map_err(|_| egress_unavailable()),
            },
        };
        match path {
            Ok(path) => self.gate(|state| {
                if state.stage != Stage::Prepared || !state.path_taken || state.path_ready {
                    return Err(consumed());
                }
                state.path_ready = true;
                Ok(path)
            }),
            Err(error) => { self.signal.close(); Err(error) },
        }
    }

    pub(super) fn submit_pending_once(&self, pending: &PendingSlot) -> Result<(), proto::WireError> {
        if pending.account() != self.account {
            self.signal.close();
            return Err(unavailable());
        }
        let mut binding = Some(pending.binding());
        let checked: Result<_, proto::WireError> = (|| {
            if self.signal.closed.load(Ordering::Acquire) || self.host.activity.retired() { return Err(unavailable()); }
            let _lifecycle = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let current = self.host.embedded.runtime.try_read().map_err(|_| unavailable())?;
            let mut state = self.signal.state.try_lock().map_err(|_| unavailable())?;
            self.check_source(current.as_ref(), &state)?;
            if state.stage != Stage::Prepared || !state.path_taken || !state.path_ready
                || state.egress.is_some() || state.final_attempted || state.pending.is_some() {
                return Err(consumed());
            }
            state.final_attempted = true;
            state.pending = binding.take();
            let mut consume = || {
                if self.signal.closed.load(Ordering::Acquire) || self.original_cancel.is_cancelled()
                    || Instant::now() >= self.deadline || host::runtime::admission_closed()
                    || self.host.closing.load(Ordering::Acquire) || self.host.generation.load(Ordering::Acquire) != self.generation {
                    return false;
                }
                state.stage = Stage::Sent;
                true
            };
            Ok(pending.try_final_check(&mut consume))
        })();
        // 此对象持有原行和异常载荷；全部Call/Host/Runtime门退出后才拆开或释放。
        let result = checked.map(|checked| checked.into_state());
        if matches!(result, Ok(PendingFinalState::Entered {
            result: PendingFinalCallback::Returned(true), commit: PendingCommitFact::Confirmed, storage_closed: true,
        })) {
            Ok(())
        } else {
            self.signal.close();
            Err(unavailable())
        }
    }

    pub(super) fn complete_pending_once(
        &self,
        completion: &PendingCompletion,
    ) -> Result<PendingCompletionCheck, proto::WireError> {
        let checked = (|| {
            if completion.account() != self.account || self.signal.closed.load(Ordering::Acquire)
                || self.host.activity.retired() { return Err(unavailable()); }
            let _lifecycle = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let current = self.host.embedded.runtime.try_read().map_err(|_| unavailable())?;
            let mut state = self.signal.state.try_lock().map_err(|_| unavailable())?;
            self.check_source(current.as_ref(), &state)?;
            if state.stage != Stage::Sent || !state.final_attempted || state.save_attempted
                || state.pending.as_ref().is_none_or(|binding| !completion.matches_binding(binding)) {
                return Err(consumed());
            }
            state.save_attempted = true;
            let mut consume = || {
                if self.signal.closed.load(Ordering::Acquire) || self.original_cancel.is_cancelled()
                    || Instant::now() >= self.deadline || host::runtime::admission_closed()
                    || self.host.closing.load(Ordering::Acquire)
                    || self.host.generation.load(Ordering::Acquire) != self.generation { return false; }
                state.stage = Stage::Saving;
                true
            };
            Ok(completion.try_commit(&mut consume))
        })();
        if checked.is_err() { self.signal.close(); }
        checked
    }

    /// 只允许本模块worker短内存核对/CAS；严禁在action内IO、SQL、网络、spawn或回入Call门。
    /// 返回值不能缓存成后续发送许可；每次使用都重新核原Host/Runtime/声明/模块/截止。
    pub(super) fn with_current<R>(&self, action: impl FnOnce() -> Result<R, proto::WireError>) -> Result<R, proto::WireError> {
        self.gate(|state| {
            if state.stage == Stage::Captured { return Err(consumed()); }
            action()
        })
    }

    /// 原执行结束/取消只封未来准入；强Call与已发送worker仍持有原Runtime到真实退出。
    pub(super) fn close(&self) { self.signal.close(); }

    pub(super) fn is_closed(&self) -> bool {
        if self.signal.closed.load(Ordering::Acquire) { return true; }
        if self.original_cancel.is_cancelled() || Instant::now() >= self.deadline
            || self.host.closing.load(Ordering::Acquire) || self.host.generation.load(Ordering::Acquire) != self.generation
            || host::runtime::admission_closed() {
            self.signal.close();
            return true;
        }
        false
    }

    /// 只观察原Runtime取消/截止和本私有结束信号，不返回可被插件控制的CancellationToken。
    /// 不得用此future直接丢弃已发HTTP；worker应只据此封后续步骤/读等待。
    pub(super) async fn closed(&self) {
        if self.is_closed() { return; }
        tokio::select! {
            biased;
            _ = self.signal.ended.cancelled() => {},
            _ = self.original_cancel.cancelled() => {},
            _ = tokio::time::sleep_until(self.deadline) => {},
        }
        self.signal.close();
    }

    fn gate<R>(&self, action: impl FnOnce(&mut State) -> Result<R, proto::WireError>) -> Result<R, proto::WireError> {
        let result = self.gate_inner(action);
        if result.is_err() { self.signal.close(); }
        result
    }

    fn gate_inner<R>(&self, action: impl FnOnce(&mut State) -> Result<R, proto::WireError>) -> Result<R, proto::WireError> {
        // state绝不持有后反取life；Activity检查在life门外，gate内不调用配置/账号/目录回调。
        if self.signal.closed.load(Ordering::Acquire) || self.host.activity.retired() { return Err(unavailable()); }
        let _lifecycle = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
        let current = self.host.embedded.runtime.try_read().map_err(|_| unavailable())?;
        let mut state = self.signal.state.try_lock().map_err(|_| unavailable())?;
        self.check_source(current.as_ref(), &state)?;
        action(&mut state)
    }

    fn check_source(&self, current: Option<&Arc<Runtime>>, state: &State) -> Result<(), proto::WireError> {
        let runtime = self.runtime.as_ref().ok_or_else(unavailable)?;
        let shared = self.shared.as_ref().ok_or_else(unavailable)?;
        let options = self.host.options.wasm.as_ref().ok_or_else(unavailable)?;
        if self.generation == 0 || !self.host.ready() || self.host.options.script
            || host::runtime::admission_closed() || self.host.generation.load(Ordering::Acquire) != self.generation
            || state.closed || self.signal.closed.load(Ordering::Acquire) || self.original_cancel.is_cancelled()
            || shared.cancel.is_cancelled() || Instant::now() >= self.deadline
            || options.provider != PROVIDER || options.source_digest() != &self.source_digest
            || shared.options.provider != PROVIDER || shared.options.source_digest() != &self.source_digest
            || runtime.module_digest() != self.module_digest || !Arc::ptr_eq(&runtime.shared, shared)
            || !current.is_some_and(|current| Arc::ptr_eq(current, runtime)) {
            return Err(unavailable());
        }
        Ok(())
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.signal.close();
        drop(self.runtime.take());
        drop(self.shared.take());
        drop(self.usage.take());
        if self.active {
            self.host.embedded.active.fetch_sub(1, Ordering::AcqRel);
            self.active = false;
        }
        // capture失败、未poll、Runtime入场失败及真实最后worker/读者退出都通知。
        self.host.embedded.idle.notify_waiters();
    }
}

fn valid_account(account: &str) -> bool {
    !account.is_empty() && account.len() <= 512 && account.trim() == account && !account.chars().any(char::is_control)
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn unavailable() -> proto::WireError {
    proto::WireError::with_code("CodeArts原换证调用或运行来源已失效，未重新选择执行体或恢复发送资格", "codearts_refresh_call_unavailable")
}

fn consumed() -> proto::WireError {
    proto::WireError::with_code("CodeArts本次换证入场、准备或发送机会已消费，未再次执行", "codearts_refresh_call_consumed")
}

fn egress_unavailable() -> proto::WireError {
    proto::WireError::with_code("CodeArts原请求账号出口未确认，未使用管理来源、JSON代理或直连回退", "codearts_refresh_egress_unavailable")
}
