//! 有限、无凭据的生命周期快照。没有事件队列，不覆写对话响应、usage 或尝试记录。

use tokio::time::Instant;

use super::input::Trace;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    NotSent,
    Preparing,
    Creating,
    Owned,
    Stopping,
    Finished,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::server::core::plugin_host::wasm) enum EndReason {
    Normal,
    ExplicitClose,
    CallCancelled,
    CallTimeout,
    Trap,
    CallerDropped,
    ReceiverDropped,
    ReceiptDropped,
    HostShutdown,
    RuntimeReplaced,
    ExecutorLost,
    ExecutionInterrupted,
    DrainTimeout,
}

/// 首个结束意图只写一次；创建/停止结果另记，不能把 cleanup 错误当对话错误。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResultKind {
    NotSent,
    Pending,
    Created,
    Acknowledged,
    Rejected,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::server::core::plugin_host::wasm) enum NotSentReason {
    AdmissionClosed,
    InvalidBinding,
    Capacity,
    AlreadySubmitted,
    AlreadyPreparing,
    CallEnded,
    PreparationExpired,
    RuntimeUnavailable,
    RandomUnavailable,
    RequestUnavailable,
}

#[derive(Clone, Copy)]
pub(super) struct StageResult {
    kind: ResultKind,
    http_status: Option<u16>,
    business_code: Option<i64>,
    message: &'static str,
}

impl StageResult {
    pub(super) fn new(
        kind: ResultKind,
        http_status: Option<u16>,
        business_code: Option<i64>,
        message: &'static str,
    ) -> Self {
        Self {
            kind,
            http_status,
            business_code,
            message,
        }
    }
    pub(super) fn kind(&self) -> ResultKind {
        self.kind
    }
    pub(super) fn http_status(&self) -> Option<u16> {
        self.http_status
    }
    pub(super) fn business_code(&self) -> Option<i64> {
        self.business_code
    }
    pub(super) fn message(&self) -> &'static str {
        self.message
    }

    pub(super) fn pending() -> Self {
        Self::new(ResultKind::Pending, None, None, "请求已登记，结果尚未确认")
    }

    pub(super) fn not_sent() -> Self {
        Self::new(ResultKind::NotSent, None, None, "没有提交创建请求")
    }

    pub(super) fn unknown() -> Self {
        Self::new(
            ResultKind::Unknown,
            None,
            None,
            "执行器已中断，远端状态未确认；不自动重放",
        )
    }
}

/// 只有当次调用读取的一个快照；不保留正文、Cookie、UUID、出口或响应头。
#[derive(Clone)]
pub(super) struct Snapshot {
    trace: Trace,
    phase: Phase,
    frozen: bool,
    submitted: bool,
    create: StageResult,
    stop: Option<StageResult>,
    stop_attempts: u8,
    reason: Option<EndReason>,
    stopped_by: Option<EndReason>,
    not_sent_reason: Option<NotSentReason>,
    registered_at: Instant,
    submitted_at: Option<Instant>,
    creation_finished_at: Option<Instant>,
    stop_started_at: Option<Instant>,
    finished_at: Option<Instant>,
    updated_at: Instant,
}

impl Snapshot {
    pub(super) fn new(trace: Trace) -> Self {
        let now = Instant::now();
        Self {
            trace,
            phase: Phase::NotSent,
            frozen: false,
            submitted: false,
            create: StageResult::not_sent(),
            stop: None,
            stop_attempts: 0,
            reason: None,
            stopped_by: None,
            not_sent_reason: None,
            registered_at: now,
            submitted_at: None,
            creation_finished_at: None,
            stop_started_at: None,
            finished_at: None,
            updated_at: now,
        }
    }

    pub(super) fn trace(&self) -> &Trace {
        &self.trace
    }
    pub(super) fn phase(&self) -> Phase {
        self.phase
    }
    pub(super) fn submitted(&self) -> bool {
        self.submitted
    }
    pub(super) fn frozen(&self) -> bool {
        self.frozen
    }
    pub(super) fn replay_forbidden(&self) -> bool {
        self.frozen || self.submitted
    }
    pub(super) fn creation(&self) -> StageResult {
        self.create
    }
    pub(super) fn stopping(&self) -> Option<StageResult> {
        self.stop
    }
    pub(super) fn stop_attempts(&self) -> u8 {
        self.stop_attempts
    }
    pub(super) fn reason(&self) -> Option<EndReason> {
        self.reason
    }
    pub(super) fn stopped_by(&self) -> Option<EndReason> {
        self.stopped_by
    }
    pub(super) fn interrupted(&mut self, reason: EndReason) {
        self.stopped_by = Some(reason);
        self.updated_at = Instant::now();
    }

    pub(super) fn not_sent_reason(&self) -> Option<NotSentReason> {
        self.not_sent_reason
    }
    pub(super) fn registered_at(&self) -> Instant {
        self.registered_at
    }
    pub(super) fn submitted_at(&self) -> Option<Instant> {
        self.submitted_at
    }
    pub(super) fn creation_finished_at(&self) -> Option<Instant> {
        self.creation_finished_at
    }
    pub(super) fn stop_started_at(&self) -> Option<Instant> {
        self.stop_started_at
    }
    pub(super) fn finished_at(&self) -> Option<Instant> {
        self.finished_at
    }
    pub(super) fn updated_at(&self) -> Instant {
        self.updated_at
    }

    pub(super) fn phase_is(&mut self, phase: Phase) {
        self.phase = phase;
        self.updated_at = Instant::now();
    }

    pub(super) fn submitted_now(&mut self) {
        self.not_sent_reason = None;
        self.finished_at = None;
        self.creation_finished_at = None;
        self.submitted = true;
        self.submitted_at = Some(Instant::now());
        self.create = StageResult::pending();
        self.phase_is(Phase::Creating);
    }

    pub(super) fn mark_not_sent(&mut self, reason: NotSentReason) {
        if !self.submitted {
            self.not_sent_reason = Some(reason);
            self.creation_is(StageResult::not_sent());
        }
    }

    pub(super) fn frozen_now(&mut self) {
        self.frozen = true;
        self.updated_at = Instant::now();
    }

    pub(super) fn begin_preparing(&mut self) {
        self.finished_at = None;
        self.not_sent_reason = None;
        self.phase_is(Phase::Preparing);
    }

    pub(super) fn creation_is(&mut self, result: StageResult) {
        self.create = result;
        self.creation_finished_at = Some(Instant::now());
        self.updated_at = Instant::now();
    }

    pub(super) fn stop_started(&mut self) {
        self.stop_attempts = 1;
        self.stop_started_at = Some(Instant::now());
        self.stop = Some(StageResult::pending());
        self.phase_is(Phase::Stopping);
    }

    pub(super) fn stop_is(&mut self, result: StageResult) {
        self.stop = Some(result);
        self.updated_at = Instant::now();
    }

    pub(super) fn set_reason(&mut self, reason: EndReason) {
        if self.reason.is_none() {
            self.reason = Some(reason);
            self.updated_at = Instant::now();
        }
    }

    pub(super) fn finish(&mut self) {
        self.finished_at = Some(Instant::now());
        self.phase_is(Phase::Finished);
    }
}
