//! 单一有界登记表：调用对象最多128，未结清全局16/每包4/每provider+账号1。
//! 顺序只允许 Signal -> Host生命周期 -> Runtime只读 -> Table -> ScopeGate -> Record -> Replay。
//! 关闭/回收可跳过前置锁，但不能从Table/Record反向进入Signal；网络/spawn不持锁。
//! 终态仅由当次调用/收据强引用保留，表只存Weak，不创建全局墓碑。

pub(in crate::server::core::plugin_host) mod shutdown;
mod preparing;
pub(in crate::server::core::plugin_host::wasm) use preparing::PreparingGuard;
pub(super) use preparing::{finalize, try_preparing, CreatePermit, FinalConsume, FinalizeFailure};
use shutdown::ShutdownReason;

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex, MutexGuard, OnceLock, Weak,
    },
};

use tokio::{
    sync::{mpsc, oneshot, Notify},
    task::AbortHandle,
    time::Instant,
};

use super::super::conversation::{CallGate, TaskCallRef};

use super::{
    input::{self, Binding, CallWindow, RuntimeBinding},
    lifecycle::{self, CreatedReceipt, Invocation, Observation, Record},
    network::{ObservationFailure, ObservationWindow, TextChunk, WitnessedCreation},
    status::{EndReason, NotSentReason, Phase, ResultKind, StageResult},
    types,
};

struct Table {
    scopes: Vec<Weak<Scope>>,
    calls: Vec<Weak<Record>>,
    running: Vec<Running>,
    receipts: HashMap<[u8; 32], Weak<Record>>,
}

struct Running {
    record: Arc<Record>,
    abort: Option<AbortHandle>,
    drain_abort_sent: bool,
    _memory: crate::server::core::runtime_capacity::Allocation,
}

fn table() -> &'static Mutex<Table> {
    static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();
    TABLE.get_or_init(|| {
        Mutex::new(Table {
            scopes: Vec::new(),
            calls: Vec::new(),
            running: Vec::new(),
            receipts: HashMap::new(),
        })
    })
}

fn lock() -> MutexGuard<'static, Table> {
    table().lock().unwrap_or_else(|error| error.into_inner())
}

fn changed() -> &'static Notify {
    static CHANGED: OnceLock<Notify> = OnceLock::new();
    CHANGED.get_or_init(Notify::new)
}

fn prune(table: &mut Table) {
    table.calls.retain(|record| record.strong_count() > 0);
    table.scopes.retain(|scope| scope.strong_count() > 0);
}

pub(super) struct Scope {
    binding: RuntimeBinding,
    runtime_cancel: tokio_util::sync::CancellationToken,
    gate: Mutex<()>,
    accepting: AtomicBool,
    reason: AtomicU8,
    shutdown: Mutex<Option<shutdown::Window>>,
    completed: Mutex<shutdown::Completed>,
    end: tokio_util::sync::CancellationToken,
}

impl Scope {
    pub(super) fn submission_gate(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn end_reason(&self) -> Option<EndReason> {
        lifecycle::decode(self.reason.load(Ordering::Acquire)).or_else(|| {
            self.runtime_cancel
                .is_cancelled()
                .then_some(EndReason::ExecutionInterrupted)
        })
    }

    pub(super) async fn ended(&self) {
        tokio::select! {
            _ = self.end.cancelled() => {},
            _ = self.runtime_cancel.cancelled() => {},
        }
    }

    fn stop(&self, reason: EndReason) {
        self.stop_at(reason, Instant::now() + input::DRAIN_TIMEOUT);
    }

    fn stop_at(&self, reason: EndReason, deadline: Instant) {
        let _gate = self.submission_gate();
        self.accepting.store(false, Ordering::Release);
        let _ = self.reason.compare_exchange(
            0,
            lifecycle::encode(reason),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let mut window = self
            .shutdown
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let deadline = shutdown::bounded_deadline(deadline);
        *window = Some(shutdown::Window {
            deadline: window.map_or(deadline, |window| window.deadline.min(deadline)),
        });
        drop(window);
        self.end.cancel();
    }
}

/// 每个真实宿主运行代次一份。没有Clone/序列化入口，同一对象关闭后不可再开。
/// 表只保留Weak；同一Shared的单次槽和实际弱引用身份阻止回收后重建旧运行代次。
pub(super) struct Executor {
    scope: Arc<Scope>,
}

impl Executor {
    pub(super) fn from_call_gate(gate: &CallGate<'_>) -> Result<Self, NotSentReason> {
        if super::policy::profile(gate.provider()).is_none() {
            return Err(NotSentReason::InvalidBinding);
        }
        let binding = gate.runtime().clone();
        let mut table = lock();
        prune(&mut table);
        // Host/global先封闭，再以同Table临界区阻止迟到的旧CallGate新建scope。
        if !binding.admission_open() {
            return Err(NotSentReason::AdmissionClosed);
        }
        if table.scopes.len() >= input::MAX_EXECUTORS {
            return Err(NotSentReason::Capacity);
        }
        if table
            .scopes
            .iter()
            .filter_map(Weak::upgrade)
            .any(|scope| scope.binding == binding)
        {
            return Err(NotSentReason::InvalidBinding);
        }
        let scope = Arc::new(Scope {
            binding,
            runtime_cancel: gate.shared().cancel.clone(),
            gate: Mutex::new(()),
            accepting: AtomicBool::new(true),
            reason: AtomicU8::new(0),
            shutdown: Mutex::new(None),
            completed: Mutex::new(Default::default()),
            end: tokio_util::sync::CancellationToken::new(),
        });
        table.scopes.push(Arc::downgrade(&scope));
        Ok(Self { scope })
    }

    pub(super) fn invocation(
        &self,
        call: TaskCallRef,
        gate: &mut CallGate<'_>,
        binding: Arc<Binding>,
        window: CallWindow,
    ) -> Result<Invocation, NotSentReason> {
        invocation(self.scope.clone(), call, gate, binding, window)
    }

    pub(super) fn active_for(&self, gate: &CallGate<'_>) -> bool {
        &self.scope.binding == gate.runtime()
            && self.scope.binding.admission_open()
            && !self.scope.runtime_cancel.is_cancelled()
            && self.scope.accepting.load(Ordering::Acquire)
    }

    pub(super) fn revoke(&self) {
        let reason = if crate::server::core::plugin_host::runtime::admission_closed() {
            EndReason::HostShutdown
        } else {
            EndReason::RuntimeReplaced
        };
        self.scope.stop(reason);
    }

    pub(super) fn stop_admission(&self) {
        // 与最终Creating转换共用表锁，返回后尚未提交的worker不能再进入Creating。
        let _table = lock();
        let _gate = self.scope.submission_gate();
        self.scope.accepting.store(false, Ordering::Release);
    }

    pub(super) fn close_for_shutdown(&self, reason: ShutdownReason, deadline: Instant) {
        let table = lock();
        self.scope.stop_at(reason.reason(), deadline);
        for item in table
            .running
            .iter()
            .filter(|item| Arc::ptr_eq(item.record.scope(), &self.scope))
        {
            item.record
                .request_stop(self.scope.end_reason().unwrap_or(reason.reason()));
        }
        drop(table);
        changed().notify_waiters();
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        // 不联网、不spawn。运行代次销毁只撤销准入并唤醒已登记worker。
        self.revoke();
    }
}

/// 只能经同源CallGate与封闭Binding登记一次，不接受插件JSON构造绑定。
fn invocation(
    scope: Arc<Scope>,
    call: TaskCallRef,
    gate: &mut CallGate<'_>,
    binding: Arc<Binding>,
    window: CallWindow,
) -> Result<Invocation, NotSentReason> {
    if !gate.matches(&call)
        || !call.same_source(window.task_call())
        || !binding.matches_call(&call)
        || !gate.can_bind()
        || &scope.binding != gate.runtime()
    {
        return Err(NotSentReason::InvalidBinding);
    }
    let mut table = lock();
    prune(&mut table);
    let _gate = scope.submission_gate();
    if !scope.accepting.load(Ordering::Acquire) || !scope.binding.admission_open() {
        return Err(NotSentReason::AdmissionClosed);
    }
    if !binding.in_runtime(&scope.binding) {
        return Err(NotSentReason::InvalidBinding);
    }
    if table.calls.len() >= input::MAX_INVOCATIONS {
        return Err(NotSentReason::Capacity);
    }
    if table
        .calls
        .iter()
        .filter_map(Weak::upgrade)
        .any(|record| record.binding().same_call(&binding))
    {
        return Err(NotSentReason::AlreadySubmitted);
    }
    let record = Arc::new(Record::new(scope.clone(), binding, window));
    if let Some(reason) = record.preparing_error() {
        return Err(reason);
    }
    gate.bind_record(&record)?;
    table.calls.push(Arc::downgrade(&record));
    Ok(Invocation::new(record))
}

/// 本函数同时核对全部容量并占用，不先放行创建再异步竞争清理名额。
pub(super) fn reserve(
    record: &Arc<Record>,
    binding: &Binding,
    start_gate: oneshot::Sender<()>,
) -> Result<(), NotSentReason> {
    let call = record.task_call().clone();
    call.with_gate(|gate| reserve_gated(record, binding, start_gate, gate))
}

fn reserve_gated(
    record: &Arc<Record>,
    binding: &Binding,
    start_gate: oneshot::Sender<()>,
    gate: &CallGate<'_>,
) -> Result<(), NotSentReason> {
    let mut table = lock();
    prune(&mut table);
    let _gate = record.scope().submission_gate();
    if !record.scope().accepting.load(Ordering::Acquire) || !record.scope().binding.admission_open()
    {
        return Err(NotSentReason::AdmissionClosed);
    }
    if record.binding().as_ref() != binding || !gate.owns_record(record) {
        return Err(NotSentReason::InvalidBinding);
    }
    if !table
        .calls
        .iter()
        .any(|item| Weak::ptr_eq(item, &Arc::downgrade(record)))
    {
        return Err(NotSentReason::InvalidBinding);
    }
    let mut state = record.lock();
    if state.snapshot.submitted() {
        return Err(NotSentReason::AlreadySubmitted);
    }
    // 未发送的worker失败也是本次生命周期终态；不以同一个Record重新启动worker，
    // 避免迟到的旧track/退出保护触及后一个预留。容量失败尚未预留，不受此限。
    if state.snapshot.phase() == Phase::Finished {
        return Err(NotSentReason::CallEnded);
    }
    if state.reserved || state.runner_present {
        return Err(NotSentReason::AlreadyPreparing);
    }
    if let Some(reason) = record.preparing_error() {
        return Err(reason);
    }
    if table.running.len() >= types::UNSETTLED_GLOBAL
        || table
            .running
            .iter()
            .filter(|item| item.record.binding().same_package(binding))
            .count()
            >= types::UNSETTLED_PER_PACKAGE
        || table
            .running
            .iter()
            .filter(|item| item.record.binding().same_account(binding))
            .count()
            >= types::UNSETTLED_PER_ACCOUNT
    {
        return Err(NotSentReason::Capacity);
    }
    let memory = crate::server::core::runtime_capacity::Allocation::new(1024 * 1024)
        .map_err(|_| NotSentReason::Capacity)?;
    // 全部容量/身份/状态检查通过后才冻结；之后没有可返回失败的预留步骤。
    gate.freeze(record)?;
    state.snapshot.frozen_now();
    state.reserved = true;
    state.runner_present = true;
    state.start_gate = Some(start_gate);
    state.snapshot.begin_preparing();
    table.running.push(Running {
        record: record.clone(),
        abort: None,
        drain_abort_sent: false,
        _memory: memory,
    });
    Ok(())
}

/// 原worker收到这个信号前只等待；同一预留的abort句柄只能登记一次，不覆盖旧所有者。
pub(super) fn track(record: &Arc<Record>, abort: AbortHandle) -> bool {
    let mut table = lock();
    let Some(running) = table
        .running
        .iter_mut()
        .find(|item| Arc::ptr_eq(&item.record, record))
    else {
        return false;
    };
    let mut state = record.lock();
    if running.abort.is_some()
        || !state.reserved
        || !state.runner_present
        || state.snapshot.phase() != Phase::Preparing
        || !state.snapshot.frozen()
        || state.snapshot.submitted()
        || record.submitted()
        || state.create_deadline.is_some()
    {
        return false;
    }
    let Some(start) = state.start_gate.take() else {
        return false;
    };
    // 即使调用已取消也登记原任务，让它自己释放材料并结清；不在这里代替worker回收。
    running.abort = Some(abort);
    // 消费出口会try Table/Record；先解锁再唤醒，避免刚开闩就与登记自身产生假繁忙。
    drop(state);
    drop(table);
    // 门闩接收端消失也如实失败；外部仅abort，不重复release或为同一Record重建任务。
    start.send(()).is_ok()
}

/// 只能由 worker 的真实 create 返回调用；先保存 UUID+责任，再允许生成随机句柄。
/// 返回 None 仍保留 cleanup 责任：取消、关闭或随机句柄碰撞都不能遗弃已创建任务。
pub(super) fn record_created(
    record: &Arc<Record>,
    creation: WitnessedCreation,
    nonce: [u8; 32],
    observation_commands: mpsc::Sender<ObserveCommand>,
) -> Option<CreatedReceipt> {
    let window = ObservationWindow::at_owned(record.call_deadline());
    let mut table = lock();
    if !table
        .running
        .iter()
        .any(|item| Arc::ptr_eq(&item.record, record))
    {
        record.request_stop(EndReason::ExecutorLost);
        return None;
    }
    let mut state = record.lock();
    if !state.reserved
        || state.snapshot.phase() != Phase::Creating
        || !state.snapshot.submitted()
        || creation.binding() != record.binding().as_ref()
    {
        record.request_stop(EndReason::ExecutorLost);
        return None;
    }
    state.snapshot.creation_is(StageResult::new(
        ResultKind::Created,
        Some(creation.http_status()),
        Some(0),
        "创建响应已确认，宿主保留本次任务的停止责任",
    ));
    state.creation = Some(creation);
    state.observation_window = Some(window);
    state.observation_commands = Some(observation_commands);
    state.snapshot.phase_is(Phase::Owned);
    if record.end_reason().is_some() {
        return None;
    }
    if table.receipts.len() >= types::UNSETTLED_GLOBAL || table.receipts.contains_key(&nonce) {
        record.request_stop(EndReason::ExecutorLost);
        return None;
    }
    state.receipt_nonce = Some(nonce);
    table.receipts.insert(nonce, Arc::downgrade(record));
    Some(CreatedReceipt::after_recorded(nonce, record))
}

/// 仅核验收据后的命令可携带此许可；没有JSON/UUID/地址恢复入口，也不能Clone。
/// 弱Record避免未消费命令与Record中的sender构成强引用环。
pub(super) struct ObservePermit {
    nonce: [u8; 32],
    record: Weak<Record>,
    window: ObservationWindow,
}

impl ObservePermit {
    pub(super) fn window(&self) -> ObservationWindow {
        self.window
    }
}

pub(super) struct ObserveCommand {
    permit: ObservePermit,
    output: mpsc::Sender<TextChunk>,
}

impl ObserveCommand {
    pub(super) fn into_parts(self) -> (ObservePermit, mpsc::Sender<TextChunk>) {
        (self.permit, self.output)
    }
}

pub(super) fn observe_receipt(
    receipt: &CreatedReceipt,
    caller: &Binding,
) -> Result<Observation, ObservationFailure> {
    if receipt.binding() != caller {
        return Err(ObservationFailure::Unavailable);
    }
    let record = receipt.record().clone();
    let call = record.task_call().clone();
    call.with_gate(|gate| {
        let table = lock();
        let _scope = record.scope().submission_gate();
        let mut state = record.lock();
        if !owned_receipt(&table, &record, &state, receipt.nonce(), gate)
            || record.binding().as_ref() != caller
        {
            return Ok(Err(ObservationFailure::Unavailable));
        }
        let Some(window) = state.observation_window else {
            return Ok(Err(ObservationFailure::Unavailable));
        };
        let Some(commands) = state.observation_commands.take() else {
            return Ok(Err(ObservationFailure::AlreadyObserved));
        };
        if let Err(error) = record.claim_observation() {
            return Ok(Err(error));
        }
        let (output, receiver) = mpsc::channel(2);
        let command = ObserveCommand {
            permit: ObservePermit {
                nonce: *receipt.nonce(),
                record: Arc::downgrade(&record),
                window,
            },
            output,
        };
        // 无await、不暴露sender、不返还观察槽；只允许这一个已核验命令。
        if commands.try_send(command).is_err() {
            record.request_stop(EndReason::ExecutorLost);
            return Ok(Err(ObservationFailure::Unavailable));
        }
        Ok(Ok(Observation::after_admitted(record.clone(), receiver)))
    })
    .map_err(|_| ObservationFailure::Unavailable)?
}

fn owned_receipt(
    table: &Table,
    record: &Arc<Record>,
    state: &lifecycle::State,
    nonce: &[u8; 32],
    gate: &CallGate<'_>,
) -> bool {
    gate.owns_record(record)
        && gate.runtime() == &record.scope().binding
        && record.binding().in_runtime(gate.runtime())
        && record.scope().accepting.load(Ordering::Acquire)
        && record.scope().binding.admission_open()
        && table
            .receipts
            .get(nonce)
            .is_some_and(|item| Weak::ptr_eq(item, &Arc::downgrade(record)))
        && table
            .running
            .iter()
            .any(|item| Arc::ptr_eq(&item.record, record) && item.abort.is_some())
        && state.receipt_nonce.as_ref() == Some(nonce)
        && state.reserved
        && state.runner_present
        && state.snapshot.phase() == Phase::Owned
        && state.snapshot.submitted()
        && state
            .creation
            .as_ref()
            .is_some_and(|creation| creation.binding() == record.binding().as_ref())
        && state.observation_end.is_none()
        && record.end_reason().is_none()
}

/// 短同步闭包只能派生固定请求/比对编号。真实创建结果仍留在Record供唯一stop消费。
/// 每次新请求在同一原子字占在途槽，stop先置意图时不能产生后续读。
pub(super) fn observation_read<T>(
    record: &Arc<Record>,
    permit: &ObservePermit,
    attach: bool,
    prepare: impl FnOnce(&WitnessedCreation) -> Result<T, ObservationFailure>,
) -> Result<(T, ObservationRead), ObservationFailure> {
    if !Weak::ptr_eq(&permit.record, &Arc::downgrade(record)) {
        return Err(ObservationFailure::Unavailable);
    }
    let call = record.task_call().clone();
    call.with_gate(|gate| {
        let table = lock();
        let _scope = record.scope().submission_gate();
        let state = record.lock();
        if !owned_receipt(&table, record, &state, &permit.nonce, gate) {
            return Ok(Err(ObservationFailure::Unavailable));
        }
        if Instant::now() >= permit.window.total_deadline() {
            return Ok(Err(ObservationFailure::TotalTimeout));
        }
        if let Err(error) = record.begin_observation_read(attach) {
            return Ok(Err(error));
        }
        let lease = ObservationRead {
            record: record.clone(),
        };
        let Some(creation) = state.creation.as_ref() else {
            return Ok(Err(ObservationFailure::Unavailable));
        };
        Ok(prepare(creation).map(|prepared| (prepared, lease)))
    })
    .map_err(|_| ObservationFailure::Unavailable)?
}

pub(super) struct ObservationRead {
    record: Arc<Record>,
}

impl Drop for ObservationRead {
    fn drop(&mut self) {
        self.record.finish_observation_read();
    }
}

pub(super) fn close_receipt(receipt: &CreatedReceipt, caller: &Binding) -> bool {
    let table = lock();
    if receipt.binding() != caller {
        return false;
    }
    let Some(record) = table.receipts.get(receipt.nonce()).and_then(Weak::upgrade) else {
        return false;
    };
    if !Arc::ptr_eq(receipt.record(), &record) || record.binding().as_ref() != caller {
        return false;
    }
    let state = record.lock();
    if state.receipt_nonce.as_ref() != Some(receipt.nonce())
        || !state.reserved
        || state.creation.is_none()
    {
        return false;
    }
    record.request_stop(EndReason::ExplicitClose);
    true
}

/// 唯一停止机会产生的消耗型许可，不暴露UUID或任意请求模板。
pub(super) struct StopPermit {
    creation: WitnessedCreation,
    deadline: Instant,
}

impl StopPermit {
    pub(super) fn into_parts(self) -> (WitnessedCreation, Instant) {
        (self.creation, self.deadline)
    }
}

/// 一次停止机会与原预留共用，无清理排队。仅返回此前保存的真实成功编号。
pub(super) fn begin_stopping(record: &Arc<Record>) -> Option<StopPermit> {
    let deadline = Instant::now() + types::STOP_TIMEOUT;
    let mut table = lock();
    if !table
        .running
        .iter()
        .any(|item| Arc::ptr_eq(&item.record, record))
    {
        return None;
    }
    let mut state = record.lock();
    if !state.reserved
        || state.snapshot.phase() != Phase::Owned
        || record.end_reason().is_none()
        || record.drain_expired()
        || usize::from(state.snapshot.stop_attempts()) >= types::STOP_ATTEMPTS_PER_CREATION
    {
        return None;
    }
    let creation = state.creation.take()?;
    state.observation_commands.take();
    state.observation_window.take();
    if let Some(nonce) = state.receipt_nonce.take() {
        table.receipts.remove(&nonce);
    }
    state.stop_deadline = Some(deadline);
    state.snapshot.stop_started();
    Some(StopPermit { creation, deadline })
}

/// 只由 worker 退出保护执行；调用之前已 drop 网络 future、正文、Cookie和整个冻结出口。
pub(super) fn release(record: &Arc<Record>) {
    let mut table = lock();
    let mut state = record.lock();
    if table
        .running
        .iter()
        .any(|item| Arc::ptr_eq(&item.record, record))
    {
        record
            .scope()
            .completed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record(&state.snapshot, record.drain_expired());
    }
    if let Some(nonce) = state.receipt_nonce.take() {
        table.receipts.remove(&nonce);
    }
    table
        .receipts
        .retain(|_, item| !Weak::ptr_eq(item, &Arc::downgrade(record)));
    state.creation.take();
    state.observation_commands.take();
    state.observation_window.take();
    state.create_deadline.take();
    state.stop_deadline.take();
    state.start_gate.take();
    state.reserved = false;
    state.runner_present = false;
    table
        .running
        .retain(|item| !Arc::ptr_eq(&item.record, record));
    drop(state);
    prune(&mut table);
    drop(table);
    record.released();
    changed().notify_waiters();
}
