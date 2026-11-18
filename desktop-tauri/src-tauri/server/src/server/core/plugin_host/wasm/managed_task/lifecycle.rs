//! 一次调用的轻量状态与已有执行器的结束意图。Drop 只置位/唤醒，绝不启动网络。
//! 唯一重材料（Cookie、正文和冻结出口）在 worker 栈，不在本地终态或收据中。

use std::sync::{
    atomic::{AtomicU16, Ordering},
    Arc, Mutex, MutexGuard,
};

use tokio::{
    sync::{mpsc, oneshot, Notify},
    time::{sleep_until, Instant},
};
use tokio_util::sync::CancellationToken;

use super::super::conversation::TaskCallRef;

use super::{
    input::{Binding, CallWindow},
    network::{
        ObservationEnd, ObservationFailure, ObservationItem, ObservationWindow, TextChunk,
        WitnessedCreation,
    },
    registry::{self, Scope},
    status::{EndReason, NotSentReason, Phase, ResultKind, Snapshot, StageResult},
};

// 同一个原子字线性化结束意图与一次提交：低字节=首个原因，高位=已消耗机会。
const SUBMITTED: u16 = 1 << 8;
const REASON_MASK: u16 = 0xff;
const DRAIN_EXPIRED: u16 = 1 << 9;
const OBSERVATION_CLAIMED: u16 = 1 << 10;
const OBSERVATION_READ: u16 = 1 << 11;
const ATTACH_CLAIMED: u16 = 1 << 12;
const OBSERVATION_BASE: u16 = SUBMITTED | OBSERVATION_CLAIMED;
const _: () = assert!(super::types::CREATE_SUBMISSIONS_PER_CALL == 1);

pub(in crate::server::core::plugin_host::wasm) struct Record {
    binding: Arc<Binding>,
    task_call: TaskCallRef,
    scope: Arc<Scope>,
    prepare_deadline: Instant,
    call_deadline: Instant,
    cancellation: CancellationToken,
    control: AtomicU16,
    wake: Notify,
    state: Mutex<State>,
    settled: tokio_util::sync::CancellationToken,
}

pub(super) struct State {
    pub(super) snapshot: Snapshot,
    pub(super) reserved: bool,
    pub(super) runner_present: bool,
    pub(super) create_deadline: Option<Instant>,
    pub(super) stop_deadline: Option<Instant>,
    pub(super) start_gate: Option<oneshot::Sender<()>>,
    // 在交付 opaque 前先写入；终态 take 掉，不形成 UUID 墓碑表。
    pub(super) creation: Option<WitnessedCreation>,
    pub(super) receipt_nonce: Option<[u8; 32]>,
    pub(super) observation_commands: Option<mpsc::Sender<registry::ObserveCommand>>,
    pub(super) observation_window: Option<ObservationWindow>,
    // 固定终态不排进输出队列，停止无需等待下游腾位置；不保存文本或网络材料。
    pub(super) observation_end: Option<ObservationEnd>,
}

impl Record {
    pub(super) fn new(scope: Arc<Scope>, binding: Arc<Binding>, window: CallWindow) -> Self {
        Self {
            state: Mutex::new(State {
                snapshot: Snapshot::new(binding.trace()),
                reserved: false,
                runner_present: false,
                create_deadline: None,
                stop_deadline: None,
                start_gate: None,
                creation: None,
                receipt_nonce: None,
                observation_commands: None,
                observation_window: None,
                observation_end: None,
            }),
            binding,
            task_call: window.task_call().clone(),
            scope,
            prepare_deadline: window.prepare_deadline(),
            call_deadline: window.call_deadline(),
            cancellation: window.cancellation().clone(),
            control: AtomicU16::new(0),
            wake: Notify::new(),
            settled: tokio_util::sync::CancellationToken::new(),
        }
    }

    pub(in crate::server::core::plugin_host::wasm) fn task_call(&self) -> &TaskCallRef {
        &self.task_call
    }

    pub(super) fn binding(&self) -> &Arc<Binding> {
        &self.binding
    }
    pub(super) fn scope(&self) -> &Arc<Scope> {
        &self.scope
    }
    pub(super) fn prepare_deadline(&self) -> Instant {
        self.prepare_deadline
    }
    pub(super) fn call_deadline(&self) -> Instant {
        self.call_deadline
    }
    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(in crate::server::core::plugin_host::wasm) fn request_stop(&self, reason: EndReason) {
        if reason == EndReason::DrainTimeout {
            self.control.fetch_or(DRAIN_EXPIRED, Ordering::AcqRel);
        }
        let _ = self
            .control
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                (value & REASON_MASK == 0).then_some(value | u16::from(encode(reason)))
            });
        self.wake.notify_waiters();
    }

    /// 只接受最终账号回调中的借用许可；此时所有调用/记录门已持有，不再读取token或取锁。
    pub(super) fn consume_submission(
        &self,
        permit: &registry::FinalConsume<'_, '_, '_>,
    ) -> Result<Instant, NotSentReason> {
        if !permit.matches_record(self) {
            return Err(NotSentReason::InvalidBinding);
        }
        let control = self.control.load(Ordering::Acquire);
        if control & SUBMITTED != 0 {
            return Err(NotSentReason::AlreadySubmitted);
        }
        if control != 0 {
            return Err(NotSentReason::CallEnded);
        }
        let now = Instant::now();
        if now >= self.call_deadline {
            return Err(NotSentReason::CallEnded);
        }
        if now >= self.prepare_deadline {
            return Err(NotSentReason::PreparationExpired);
        }
        let deadline = self.call_deadline.min(now + super::types::CREATE_TIMEOUT);
        self.control
            .compare_exchange(0, SUBMITTED, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| deadline)
            .map_err(|value| {
                if value & SUBMITTED != 0 {
                    NotSentReason::AlreadySubmitted
                } else {
                    NotSentReason::CallEnded
                }
            })
    }

    /// 仅registry完成Owned/收据/运行代次检查后调用；与结束意图共用CAS且不返还机会。
    pub(super) fn claim_observation(&self) -> Result<(), ObservationFailure> {
        if self.end_reason().is_some() {
            return Err(ObservationFailure::Closed);
        }
        self.control
            .compare_exchange(
                SUBMITTED,
                OBSERVATION_BASE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|value| {
                if value & (REASON_MASK | DRAIN_EXPIRED) != 0 {
                    ObservationFailure::Closed
                } else if value & OBSERVATION_CLAIMED != 0 {
                    ObservationFailure::AlreadyObserved
                } else {
                    ObservationFailure::Unavailable
                }
            })
    }

    /// 每个新GET/attach的确认点；先结束则拒绝，先确认则属于可立即取消的在途读取。
    /// 不能用它创建许可；registry仍须先核对同一Record及已登记真实创建结果。
    pub(super) fn begin_observation_read(&self, attach: bool) -> Result<(), ObservationFailure> {
        if self.end_reason().is_some() {
            return Err(ObservationFailure::Closed);
        }
        let next = OBSERVATION_BASE | OBSERVATION_READ | if attach { ATTACH_CLAIMED } else { 0 };
        self.control
            .compare_exchange(OBSERVATION_BASE, next, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| ObservationFailure::Closed)
    }

    /// 只结束在途读槽，不清除观察、attach或结束意图；失败的attach不能重连。
    pub(super) fn finish_observation_read(&self) {
        self.control.fetch_and(!OBSERVATION_READ, Ordering::AcqRel);
    }

    pub(super) fn finish_observation(&self, mut ended: ObservationEnd) {
        let _scope = self.scope.submission_gate();
        let mut state = self.lock();
        // 真实RoundEnded与显式停止争同一原子字；停止先赢时不能晚交付正常轮次结束。
        if matches!(ended, ObservationEnd::RoundEnded) {
            let expected = OBSERVATION_BASE | ATTACH_CLAIMED;
            if self.end_reason().is_some()
                || self
                    .control
                    .compare_exchange(
                        expected,
                        expected | u16::from(encode(EndReason::Normal)),
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_err()
            {
                ended = ObservationEnd::Failed(ObservationFailure::Closed);
            }
        }
        if state.observation_end.is_none() {
            state.observation_end = Some(ended);
        }
        drop(state);
        self.wake.notify_waiters();
    }

    pub(super) fn observation_end(&self) -> Option<ObservationEnd> {
        self.lock().observation_end
    }

    pub(super) fn submitted(&self) -> bool {
        self.control.load(Ordering::Acquire) & SUBMITTED != 0
    }

    pub(super) fn drain_expired(&self) -> bool {
        self.control.load(Ordering::Acquire) & DRAIN_EXPIRED != 0
    }

    pub(super) fn end_reason(&self) -> Option<EndReason> {
        // 不能回调TaskCallRef/Signal：调用者可能已持Table/Record锁。
        if let Some(reason) = self.scope.end_reason() {
            self.request_stop(reason);
        }
        if self.cancellation.is_cancelled() {
            // 真实关闭原因已在Signal门内先写原子位；此处只是缺失登记时的保守兜底。
            self.request_stop(EndReason::CallerDropped);
        } else if Instant::now() >= self.call_deadline {
            self.request_stop(EndReason::CallTimeout);
        }
        decode((self.control.load(Ordering::Acquire) & REASON_MASK) as u8)
    }

    /// 原子交付确认点。返回后发生的close属于已交付窗口，但仍由同一worker清理。
    pub(super) fn accept_receipt_delivery(&self) -> bool {
        let _admission = self.scope.submission_gate();
        if self.end_reason().is_some() {
            return false;
        }
        self.control
            .compare_exchange(SUBMITTED, SUBMITTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(super) async fn ended(&self) -> EndReason {
        loop {
            let notified = self.wake.notified();
            tokio::pin!(notified);
            // 先登记等待，再检查意图，避免检查与进入等待之间丢失唤醒。
            notified.as_mut().enable();
            if let Some(reason) = self.end_reason() {
                return reason;
            }
            tokio::select! {
                biased;
                _ = self.scope.ended() => {},
                _ = self.cancellation.cancelled() => self.request_stop(EndReason::CallerDropped),
                _ = sleep_until(self.call_deadline) => self.request_stop(EndReason::CallTimeout),
                _ = &mut notified => {},
            }
        }
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        let reason = self.end_reason();
        let mut snapshot = self.lock().snapshot.clone();
        if let Some(reason) = reason {
            if snapshot.phase() != Phase::Finished {
                snapshot.set_reason(reason);
            }
        }
        snapshot
    }

    pub(super) fn released(&self) {
        self.settled.cancel();
    }

    pub(super) async fn settled(&self) {
        self.settled.cancelled().await;
    }

    pub(in crate::server::core::plugin_host::wasm) fn preparing_error(
        &self,
    ) -> Option<NotSentReason> {
        if self.end_reason().is_some() {
            Some(NotSentReason::CallEnded)
        } else if Instant::now() >= self.prepare_deadline {
            Some(NotSentReason::PreparationExpired)
        } else {
            None
        }
    }

    pub(super) fn revert_preparation(&self) {
        let mut state = self.lock();
        if !state.snapshot.submitted() {
            state.snapshot.phase_is(Phase::NotSent);
        }
    }

    pub(super) fn complete_creation(&self, result: StageResult) {
        self.lock().snapshot.creation_is(result);
    }

    pub(super) fn complete_stop(&self, result: StageResult) {
        self.lock().snapshot.stop_is(result);
    }

    /// worker 的材料已经释放后才调用。未知终态保留 submitted，但不占用未结清名额。
    pub(super) fn settle(&self, lost: bool) {
        let reason = self.end_reason();
        let mut state = self.lock();
        if self.submitted() && !state.snapshot.submitted() {
            state.snapshot.submitted_now();
        }
        if state.snapshot.creation().kind() == ResultKind::Pending {
            state.snapshot.creation_is(StageResult::unknown());
        }
        if state.snapshot.creation().kind() == ResultKind::Created
            && state
                .snapshot
                .stopping()
                .is_none_or(|result| result.kind() == ResultKind::Pending)
        {
            state.snapshot.stop_is(StageResult::unknown());
        }
        if lost || self.drain_expired() {
            state.snapshot.interrupted(
                if self.control.load(Ordering::Acquire) & DRAIN_EXPIRED != 0 {
                    EndReason::DrainTimeout
                } else {
                    EndReason::ExecutorLost
                },
            );
            if !self.submitted() && state.snapshot.not_sent_reason().is_none() {
                // 原准备所有者已确认取消/到期时保留真实NotSent原因；排空中断另记。
                state
                    .snapshot
                    .mark_not_sent(NotSentReason::RuntimeUnavailable);
            }
        }
        if let Some(reason) = reason {
            state.snapshot.set_reason(reason);
        }
        state.creation.take();
        state.observation_commands.take();
        state.observation_window.take();
        if self.control.load(Ordering::Acquire) & OBSERVATION_CLAIMED != 0
            && state.observation_end.is_none()
        {
            state.observation_end = Some(ObservationEnd::Failed(if lost {
                ObservationFailure::ExecutorLost
            } else {
                ObservationFailure::Closed
            }));
        }
        // nonce由registry::release在同锁移除索引；这里先take会留下死收据表项。
        state.snapshot.finish();
    }
}

/// 生命周期所有者与唯一提交机会。不能 Clone/Deserialize；调用结束只请求停止。
pub(super) struct Invocation {
    record: Arc<Record>,
}

impl Invocation {
    pub(super) fn new(record: Arc<Record>) -> Self {
        Self { record }
    }
    pub(super) fn record(&self) -> &Arc<Record> {
        &self.record
    }
    pub(super) fn close(&self, reason: EndReason) {
        self.record.request_stop(reason);
    }
    pub(super) fn snapshot(&self) -> Snapshot {
        self.record.snapshot()
    }
    pub(super) fn replay_forbidden(&self) -> bool {
        let state = self.record.lock();
        // 冻结后的NotSent/准备失败/配额回收都不能解除本次请求的禁止重放。
        self.record.submitted() || state.snapshot.frozen()
    }
}

impl Drop for Invocation {
    fn drop(&mut self) {
        self.record.request_stop(EndReason::CallerDropped);
    }
}

/// 只存随机引用与本次绑定；没有任务 UUID、凭据或解除清理义务的开关。
/// 保留轻量Record，故旧句柄也受128调用上限约束；无客户端引用时不留全局强引用。
/// 只能在已见证真实成功并保存 creation 的 registry 临界区中构造。
pub(super) struct CreatedReceipt {
    nonce: [u8; 32],
    binding: Arc<Binding>,
    record: Arc<Record>,
}

impl CreatedReceipt {
    pub(super) fn after_recorded(nonce: [u8; 32], record: &Arc<Record>) -> Self {
        Self {
            nonce,
            binding: record.binding().clone(),
            record: record.clone(),
        }
    }
    pub(super) fn nonce(&self) -> &[u8; 32] {
        &self.nonce
    }
    pub(super) fn binding(&self) -> &Binding {
        &self.binding
    }
    pub(super) fn record(&self) -> &Arc<Record> {
        &self.record
    }
    pub(super) fn snapshot(&self) -> Snapshot {
        self.record.snapshot()
    }

    pub(super) fn observe(&self, caller: &Binding) -> Result<Observation, ObservationFailure> {
        registry::observe_receipt(self, caller)
    }

    pub(super) fn close(&self, caller: &Binding) -> bool {
        registry::close_receipt(self, caller)
    }
}

impl Drop for CreatedReceipt {
    fn drop(&mut self) {
        self.record.request_stop(EndReason::ReceiptDropped);
    }
}

/// 非Clone观察接收端；没有UUID/URL/账号材料/client/socket，旧句柄仍占调用对象配额。
/// 单次next的等待被取消也请求停止，不取消原worker或已提交的创建future。
pub(super) struct Observation {
    record: Arc<Record>,
    receiver: mpsc::Receiver<TextChunk>,
}

impl Observation {
    pub(super) fn after_admitted(record: Arc<Record>, receiver: mpsc::Receiver<TextChunk>) -> Self {
        Self { record, receiver }
    }

    pub(super) fn next(&mut self) -> impl std::future::Future<Output = ObservationItem> + '_ {
        let record = &self.record;
        let receiver = &mut self.receiver;
        // guard在future之外创建：尚未首次poll就取消等待，也能通知原worker停止。
        let mut waiting = ObservationWait {
            record,
            completed: false,
        };
        async move {
            let terminal = || {
                record
                    .observation_end()
                    .unwrap_or(ObservationEnd::Failed(ObservationFailure::Closed))
            };
            // 观察结果固定后只取已有的有界队列，不再等待或进行远端读取；保留末尾文本。
            let item = if record.observation_end().is_some() {
                match receiver.try_recv() {
                    Ok(text) => ObservationItem::Text(text),
                    Err(_) => ObservationItem::End(terminal()),
                }
            } else {
                tokio::select! {
                    biased;
                    _ = record.ended() => {
                        if record.observation_end().is_some() {
                            match receiver.try_recv() {
                                Ok(text) => ObservationItem::Text(text),
                                Err(_) => ObservationItem::End(terminal()),
                            }
                        } else {
                            ObservationItem::End(ObservationEnd::Failed(ObservationFailure::Closed))
                        }
                    }
                    received = receiver.recv() => {
                        if record.end_reason().is_some() && record.observation_end().is_none() {
                            ObservationItem::End(ObservationEnd::Failed(ObservationFailure::Closed))
                        } else {
                            match received {
                                Some(text) => ObservationItem::Text(text),
                                None => ObservationItem::End(record.observation_end().unwrap_or(
                                    ObservationEnd::Failed(ObservationFailure::ExecutorLost),
                                )),
                            }
                        }
                    }
                }
            };
            waiting.complete();
            item
        }
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        self.record.request_stop(EndReason::ReceiverDropped);
    }
}

struct ObservationWait<'a> {
    record: &'a Record,
    completed: bool,
}

impl ObservationWait<'_> {
    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for ObservationWait<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.record.request_stop(EndReason::ReceiverDropped);
        }
    }
}

pub(super) fn encode(reason: EndReason) -> u8 {
    match reason {
        EndReason::Normal => 1,
        EndReason::ExplicitClose => 2,
        EndReason::CallCancelled => 3,
        EndReason::CallTimeout => 4,
        EndReason::Trap => 5,
        EndReason::CallerDropped => 6,
        EndReason::ReceiverDropped => 7,
        EndReason::ReceiptDropped => 8,
        EndReason::HostShutdown => 9,
        EndReason::RuntimeReplaced => 10,
        EndReason::ExecutorLost => 11,
        EndReason::DrainTimeout => 12,
        EndReason::ExecutionInterrupted => 13,
    }
}

pub(super) fn decode(value: u8) -> Option<EndReason> {
    Some(match value {
        1 => EndReason::Normal,
        2 => EndReason::ExplicitClose,
        3 => EndReason::CallCancelled,
        4 => EndReason::CallTimeout,
        5 => EndReason::Trap,
        6 => EndReason::CallerDropped,
        7 => EndReason::ReceiverDropped,
        8 => EndReason::ReceiptDropped,
        9 => EndReason::HostShutdown,
        10 => EndReason::RuntimeReplaced,
        11 => EndReason::ExecutorLost,
        12 => EndReason::DrainTimeout,
        13 => EndReason::ExecutionInterrupted,
        _ => return None,
    })
}
