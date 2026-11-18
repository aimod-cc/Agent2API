//! 原 worker 的一次观察；这里的窗口和投影不是权限，许可只由 registry 签发。
//! 总期限继承调用（准备时已钳到1h），Owned时冻结就绪/建连/空闲预算；不按read重置。

use std::{future::Future, sync::Arc, time::Duration};

use tokio::{
    sync::mpsc,
    time::{sleep_until, Instant},
};

use super::super::{
    lifecycle::Record,
    observe::{
        DetailFailure, DetailUnknown, Interaction, Invalid, StreamFailure, TextKind,
        ToolObservation,
    },
    registry::ObservePermit,
    status::EndReason,
    types::MAX_LIFETIME,
};
use super::Session;

pub(super) const MAX_DETAIL_REQUESTS: usize = 1800;
pub(super) const DETAIL_INTERVAL: Duration = Duration::from_secs(2);
pub(super) const DETAIL_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const MAX_DETAIL_TOTAL_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_UPGRADED_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_MESSAGES: usize = 4096;
pub(super) const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub(super) const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub(in super::super) struct ObservationWindow {
    pub(super) total: Instant,
    pub(super) ready: Instant,
    pub(super) connect: Duration,
    pub(super) idle: Duration,
}

impl ObservationWindow {
    pub(in super::super) fn total_deadline(&self) -> Instant {
        self.total
    }

    /// 在登记Owned之前、持Table/Record锁之前读一次宿主配置；插件不能提供这些值。
    pub(in super::super) fn at_owned(call_deadline: Instant) -> Self {
        let now = Instant::now();
        let timeouts = crate::server::config::timeout_settings();
        let total = call_deadline.min(now + MAX_LIFETIME);
        Self {
            total,
            ready: total.min(now + Duration::from_millis(timeouts.headers_ms()).min(MAX_LIFETIME)),
            // 保留原MonkeyCode建连范围与模型空闲设置（默认300s，旧下限30s）。
            connect: Duration::from_millis(timeouts.connect_ms().clamp(5_000, 120_000)),
            idle: Duration::from_millis(timeouts.stream_idle_ms().max(30_000)).min(MAX_LIFETIME),
        }
    }
}

#[derive(Clone, Copy)]
pub(in super::super) enum ObservationFailure {
    Unavailable,
    AlreadyObserved,
    Closed,
    ExecutorLost,
    TotalTimeout,
    ReadinessTimeout,
    DetailTimeout,
    HandshakeTimeout,
    IdleTimeout,
    HeartbeatTimeout,
    Transport,
    Handshake,
    UnexpectedFrame,
    StreamClosed,
    DetailRequests,
    DetailBytes,
    UpgradedBytes,
    MessageCount,
    MessageBytes,
    OutputBytes,
    Identity,
    Invalid(Invalid),
    DetailFailed(DetailFailure),
    DetailUnknown(DetailUnknown),
    StreamFailed(StreamFailure),
}

impl ObservationFailure {
    pub(in super::super) fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "任务观察收据、运行代次或固定出口不可用，未授权读取",
            Self::AlreadyObserved => "本次调用的唯一观察机会已消耗，不重新读取或连接",
            Self::Closed => "任务观察已结束或消费者已取消，不再读取",
            Self::ExecutorLost => "任务观察执行器已退出，未确认本轮结束",
            Self::TotalTimeout => "任务观察超过原调用截止或一小时总上限，未确认本轮结束",
            Self::ReadinessTimeout => "任务环境未在原响应头等待预算内就绪，已停止观察",
            Self::DetailTimeout => "本次任务详情查询超过二十秒或剩余等待预算，未重试",
            Self::HandshakeTimeout => "任务流未在原建连预算内完成升级，未重新连接",
            Self::IdleTimeout => "任务流持续没有模型文本进展，心跳与未知控制消息不延长期限",
            Self::HeartbeatTimeout => "任务流固定心跳写入超时，未继续观察",
            Self::Transport => "任务观察传输中断，未重试或重新连接",
            Self::Handshake => "任务流握手不符合固定HTTP/1.1升级约束，未读取任务流",
            Self::UnexpectedFrame => "任务流返回了本层不接受的非文本数据帧，未确认本轮结束",
            Self::StreamClosed => "任务流在真实本轮结束消息之前关闭，不能当作成功",
            Self::DetailRequests => "任务详情查询已达到本次调用的1800次上限，未确认就绪",
            Self::DetailBytes => "任务详情超过单次1MiB或累计32MiB正文预算，未截断为成功",
            Self::UpgradedBytes => "任务升级流入站字节预算32MiB已耗尽，未截断为成功",
            Self::MessageCount => "任务流已达到4096条消息上限，未确认本轮结束",
            Self::MessageBytes => "任务流帧或重组消息超过本层大小限制，未截断为成功",
            Self::OutputBytes => "任务观察文本或工具投影超过累计8MiB预算，未截断为成功",
            Self::Identity => "任务详情编号与本次登记的真实创建结果不一致，已停止观察",
            Self::Invalid(invalid) => invalid.message(),
            Self::DetailFailed(DetailFailure::TaskError) => "任务详情明确报告任务失败",
            Self::DetailFailed(DetailFailure::EnvironmentFailed) => "任务详情明确报告执行环境失败",
            Self::DetailUnknown(DetailUnknown::HttpRejected) => "任务详情HTTP回应拒绝成功判定",
            Self::DetailUnknown(DetailUnknown::BusinessRejected) => "任务详情业务回应拒绝成功判定",
            Self::DetailUnknown(DetailUnknown::UnrecognizedStatus) => {
                "任务详情状态未实现，未继续观察或推测成功"
            }
            Self::StreamFailed(StreamFailure::TaskError) => "任务流明确报告任务失败",
            Self::StreamFailed(StreamFailure::Error) => "任务流明确报告错误",
            Self::StreamFailed(StreamFailure::ToolFailed) => {
                "远端Agent工具明确报告失败，本轮未确认正常完成"
            }
        }
    }
}

/// 只有RoundEnded来自真实消息；它仍不等于任务停止或执行环境回收。
#[derive(Clone, Copy)]
pub(in super::super) enum ObservationEnd {
    RoundEnded,
    NeedsInteraction(Interaction),
    Failed(ObservationFailure),
}

/// 沿用原有有界通道的类型名；载荷区分文本与工具，不能把工具JSON塞进模型正文。
pub(in super::super) enum TextChunk {
    Text { kind: TextKind, text: String },
    Tool(ToolObservation),
}

impl TextChunk {
    fn output_bytes(&self) -> usize {
        match self {
            Self::Text { text, .. } => text.len(),
            Self::Tool(tool) => tool.output_bytes(),
        }
    }
}

pub(in super::super) enum ObservationItem {
    Text(TextChunk),
    End(ObservationEnd),
}

pub(super) struct Observer<'a> {
    pub(super) record: &'a Arc<Record>,
    pub(super) permit: &'a ObservePermit,
    pub(super) output: &'a mpsc::Sender<TextChunk>,
    pub(super) window: ObservationWindow,
    pub(super) detail_bytes: usize,
    messages: usize,
    output_bytes: usize,
}

impl Session {
    /// Session和实际socket始终留在调用它的原worker栈，不产生独立后台网络任务。
    pub(in super::super) async fn observe(
        &self,
        record: &Arc<Record>,
        permit: ObservePermit,
        output: &mpsc::Sender<TextChunk>,
    ) -> ObservationEnd {
        let mut observer = Observer {
            record,
            permit: &permit,
            output,
            window: permit.window(),
            detail_bytes: 0,
            messages: 0,
            output_bytes: 0,
        };
        let result = async {
            super::detail::wait_ready(self, &mut observer).await?;
            super::attach::run(self, &mut observer).await
        }
        .await;
        let result = result.and_then(|ended| {
            observer.check(observer.window.total, ObservationFailure::TotalTimeout)?;
            Ok(ended)
        });
        match result {
            Ok(ended) => ended,
            Err(failure) => ObservationEnd::Failed(failure),
        }
    }
}

impl Observer<'_> {
    /// 一切网络等待、间隔和输出背压都从这里经过；结束/消费者消失/期限优先。
    pub(super) async fn wait<T>(
        &self,
        deadline: Instant,
        timeout: ObservationFailure,
        work: impl Future<Output = Result<T, ObservationFailure>>,
    ) -> Result<T, ObservationFailure> {
        let deadline = deadline.min(self.window.total);
        self.check(deadline, timeout)?;
        let result = tokio::select! {
            biased;
            _ = self.record.ended() => Err(ObservationFailure::Closed),
            _ = self.output.closed() => {
                self.record.request_stop(EndReason::ReceiverDropped);
                Err(ObservationFailure::Closed)
            }
            _ = sleep_until(deadline) => Err(self.timeout(deadline, timeout)),
            result = work => result,
        };
        self.check(deadline, timeout)?;
        result
    }

    fn timeout(&self, deadline: Instant, otherwise: ObservationFailure) -> ObservationFailure {
        if deadline >= self.window.total {
            ObservationFailure::TotalTimeout
        } else {
            otherwise
        }
    }

    pub(super) fn check(
        &self,
        deadline: Instant,
        timeout: ObservationFailure,
    ) -> Result<(), ObservationFailure> {
        if let Some(reason) = self.record.end_reason() {
            return Err(if reason == EndReason::CallTimeout {
                ObservationFailure::TotalTimeout
            } else {
                ObservationFailure::Closed
            });
        }
        if self.output.is_closed() {
            self.record.request_stop(EndReason::ReceiverDropped);
            return Err(ObservationFailure::Closed);
        }
        if Instant::now() >= self.window.total {
            return Err(ObservationFailure::TotalTimeout);
        }
        if Instant::now() >= deadline {
            return Err(self.timeout(deadline, timeout));
        }
        Ok(())
    }

    pub(super) fn count_message(&mut self) -> Result<(), ObservationFailure> {
        if self.messages >= MAX_MESSAGES {
            return Err(ObservationFailure::MessageCount);
        }
        self.messages += 1;
        Ok(())
    }

    pub(super) fn messages_available(&self) -> Result<(), ObservationFailure> {
        (self.messages < MAX_MESSAGES)
            .then_some(())
            .ok_or(ObservationFailure::MessageCount)
    }

    pub(super) async fn emit(
        &mut self,
        chunk: TextChunk,
        idle: Instant,
    ) -> Result<(), ObservationFailure> {
        let bytes = chunk.output_bytes();
        if bytes > MAX_OUTPUT_BYTES.saturating_sub(self.output_bytes) {
            return Err(ObservationFailure::OutputBytes);
        }
        self.output_bytes += bytes;
        self.wait(idle, ObservationFailure::IdleTimeout, async {
            self.output
                .send(chunk)
                .await
                .map_err(|_| ObservationFailure::Closed)
        })
        .await
    }
}
