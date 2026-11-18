//! 流式帧的网关侧消费者。
//!
//! ── 这个类型解决什么 ────────────────────────────────────────
//! `ForwardOutcome::Stream` 要求一个**活的** `Stream`，而插件的字节在另一个
//! 进程里。跨进程把它拉平之后，网关这侧拿到的是「一条无界通道 + 一个额度
//! 计数器」，本类型把那两样重新装成一条 `Stream` —— 上层（`chat.rs` 的
//! `RecordingStream`、协议翻译、续写）看到的形状与内置家完全一致。
//!
//! ── 背压在这里发生 ──────────────────────────────────────────
//! 插件侧的窗口是 [`proto::DEFAULT_WINDOW_BYTES`]，每发一个 chunk 就
//! `forget()` 掉等量额度（见 SDK 的 `StreamSink`）。额度只有网关发
//! `credit{n}` 才补回来。所以本类型**每消费一个 chunk 就归还等量额度**：
//! 客户端读得慢 → 上层不 poll 我们 → 我们不归还 → 插件停在 `send` 上。
//! 这条链断了的话症状是「插件把整段回答缓冲进自己的内存」，背压形同虚设。
//!
//! ── 为什么收尾要合成错误帧 ──────────────────────────────────
//! 客户端此时已经收到 HTTP 200 与部分内容，唯一能告诉它「这条流是失败的」
//! 的办法就是在流里写一条错误帧（内置家的 `ForwardStream` 是同一处置）。
//! 插件自己可能已经写过一条 —— 所以合成前先看尾巴：末尾已经有 `[DONE]`
//! 就不再补，避免客户端收到两个终止信号。

pub(super) mod workbuddy;

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use serde_json::{json, Value};

use agent2api_provider_protocol as proto;

use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::errors::GatewayError;
use crate::server::logging;

pub(crate) type FailureSlot = Arc<Mutex<Option<GatewayError>>>;
pub(crate) type PrivateFailureSlot = Arc<Mutex<Option<Value>>>;

use super::errors::strict;
use super::wasm::conversation::ConversationOwner;
use super::{Host, StreamEvent};

/// 标准 Chat SSE 的只读用量旁路；跨分片缓冲有界，不改写下发字节。
#[derive(Default)]
struct UsageSniffer {
    strict_provider: Option<&'static str>,
    workbuddy: bool,
    workbuddy_failure: Option<GatewayError>,
    line: Vec<u8>,
    data: Vec<u8>,
    line_bytes: usize,
    event_bytes: usize,
    skipping_event: bool,
    skip_lf: bool,
    first_line_seen: bool,
    done: bool,
    failure: Option<Value>,
}

impl UsageSniffer {
    const MAX_EVENT_BYTES: usize = 1024 * 1024;

    fn feed(&mut self, bytes: &[u8], telemetry: &RequestTelemetry) {
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' { continue; }
            }
            if byte == b'\r' || byte == b'\n' {
                self.consume_line(telemetry);
                self.skip_lf = byte == b'\r';
            } else {
                self.line_bytes = self.line_bytes.saturating_add(1);
                self.event_bytes = self.event_bytes.saturating_add(1);
                if self.event_bytes > Self::MAX_EVENT_BYTES {
                    self.line.clear();
                    self.data.clear();
                    // 超大事件只在下一个空行恢复，余段不能被误认成新事件。
                    self.skipping_event = true;
                }
                if !self.skipping_event { self.line.push(byte); }
            }
        }
    }

    fn finish(&mut self, telemetry: &RequestTelemetry) {
        if self.line_bytes > 0 { self.consume_line(telemetry); }
        if !self.skipping_event { self.consume_data(telemetry, false); }
        self.line.clear();
        self.data.clear();
        self.line_bytes = 0;
        self.event_bytes = 0;
        self.skipping_event = false;
        self.skip_lf = false;
    }

    fn consume_line(&mut self, telemetry: &RequestTelemetry) {
        if self.line_bytes > 0 {
            self.event_bytes = self.event_bytes.saturating_add(1);
            if self.event_bytes > Self::MAX_EVENT_BYTES {
                self.line.clear();
                self.data.clear();
                self.skipping_event = true;
            }
        }
        if self.line_bytes == 0 {
            if !self.skipping_event { self.consume_data(telemetry, true); }
            self.data.clear();
            self.event_bytes = 0;
            self.skipping_event = false;
        } else if !self.skipping_event {
            let line = if !self.first_line_seen {
                self.line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&self.line)
            } else { &self.line };
            let payload = line.strip_prefix(b"data:").or_else(|| (line == b"data").then_some(&[][..]));
            if let Some(payload) = payload {
                let payload = payload.strip_prefix(b" ").unwrap_or(payload);
                self.data.extend_from_slice(payload);
                self.data.push(b'\n');
                // 聚合器逐行识别错误；本家完整JSON行到达即可记住，不能等空行而漏过聚合早退。
                if self.workbuddy { self.consume_data(telemetry, false); }
            }
        }
        self.first_line_seen = true;
        self.line.clear();
        self.line_bytes = 0;
    }

    fn remember_workbuddy_error(&mut self, error: GatewayError) {
        match &mut self.workbuddy_failure {
            Some(previous) => super::errors::workbuddy_stream::merge(previous, error),
            None => self.workbuddy_failure = Some(error),
        }
    }

    fn consume_data(&mut self, telemetry: &RequestTelemetry, complete: bool) {
        let Ok(text) = std::str::from_utf8(&self.data) else { return };
        if text.trim() == "[DONE]" {
            self.done |= complete;
            return;
        }
        if ![b"usage".as_slice(), b"error".as_slice()]
            .iter().any(|field| self.data.windows(field.len()).any(|window| window == *field))
        {
            return;
        }
        let Ok(chunk) = serde_json::from_slice::<Value>(&self.data) else { return };
        if let Some(usage) = chunk.get("usage").filter(|value| value.is_object()) {
            telemetry.report_usage(usage);
        }
        if let Some(provider) = self.strict_provider {
            if let Some((failure, private)) = strict::stream_failure(provider, &chunk) {
                let prior_interruption = self.failure.as_ref().is_some_and(|body| {
                    matches!(body.get("statusCode").and_then(Value::as_u64), Some(499 | 504))
                });
                if self.failure.is_none() || strict::stopped(&failure) && !prior_interruption {
                    self.failure = Some(private);
                }
                telemetry.note_error(&failure.message);
            }
            return;
        }
        if self.workbuddy {
            if let Some(error) = chunk.get("error").filter(|value| crate::server::core::protocol::is_truthy(value)) {
                self.remember_workbuddy_error(super::errors::workbuddy_stream::frame(error));
            }
            return;
        }
        if let Some(error) = chunk.get("error").filter(|value| crate::server::core::protocol::is_truthy(value)) {
            if self.failure.is_none() { self.failure = Some(error.clone()); }
            let message = error.get("message").and_then(Value::as_str)
                .or_else(|| error.as_str()).filter(|text| !text.trim().is_empty())
                .map(str::to_string).unwrap_or_else(|| "上游流式返回错误".to_string());
            telemetry.note_error(&message);
        }
    }
}

/// 错误正文先暂停下发，最多等一秒/16条/256KiB补齐同一调用的控制终态。
struct WorkbuddyTail {
    until: Pin<Box<tokio::time::Sleep>>,
    events: usize,
    bytes: usize,
}

/// 一条插件流在网关侧的形态。
pub(crate) struct RpcStream {
    /// 宿主（Drop 时要发 abort / 清登记）。
    host: Arc<Host>,
    lease: Option<Arc<super::activity::UsageLease>>,
    conversation: Option<ConversationOwner>,
    /// 本次调用的 RPC id。
    id: i64,
    /// 插件的 chunk / 收尾事件。
    events: super::StreamEvents,
    /// 归还额度的发送端（见模块头）。通道关闭 = 本流结束，泵任务自行退出。
    credit: tokio::sync::mpsc::UnboundedSender<usize>,
    /// 待下发的合成帧（错误帧 / 补的 `[DONE]`）。
    pending: VecDeque<Bytes>,
    /// 已经排好收尾帧，下一次 poll 直接返回 `None`。
    ended: bool,
    /// usage / 首响旁路槽。
    telemetry: Arc<RequestTelemetry>,
    usage: UsageSniffer,
    failure: Option<FailureSlot>,
    private_failure: Option<PrivateFailureSlot>,
    raw: bool,
    workbuddy: bool,
    workbuddy_owner: Option<workbuddy::Owner>,
    workbuddy_tail: Option<WorkbuddyTail>,
    workbuddy_synthetic: bool,
    settling: Option<futures::future::BoxFuture<'static, ()>>,
}

impl RpcStream {
    /// 建一条流，并起一个**额度泵**。
    ///
    /// ── 为什么额度要单独一个任务 ────────────────────────────────
    /// `poll_next` 是同步的，而归还额度要写 stdin（异步）。在 `poll_next`
    /// 里 `tokio::spawn` 一个写任务会让每个 chunk 起一个任务（高频流上是
    /// 每秒几十个），而泵任务是一条流一个：它阻塞在 `recv` 上，来一个还一个。
    ///
    /// `pending` 是「已经从通道里取出来、但还没下发」的正文（见
    /// [`Self::with_pending`] 的说明）。
    pub(super) fn with_pending(
        provider: &'static str,
        host: Arc<Host>,
        id: i64,
        events: super::StreamEvents,
        pending: Option<Vec<u8>>,
        telemetry: Arc<RequestTelemetry>,
        failure: Option<FailureSlot>,
        private_failure: Option<PrivateFailureSlot>,
        lease: Arc<super::activity::UsageLease>,
    ) -> Self {
        let (credit, mut credit_rx) = tokio::sync::mpsc::unbounded_channel::<usize>();
        let pump_host = host.clone();
        tokio::spawn(async move {
            while let Some(n) = credit_rx.recv().await {
                pump_host.credit(id, n).await;
            }
        });
        let mut queue = VecDeque::new();
        let mut usage = UsageSniffer {
            strict_provider: strict::applies(provider).then_some(provider),
            workbuddy: provider == "workbuddy" && host.options.wasm.is_some(),
            ..UsageSniffer::default()
        };
        if let Some(bytes) = pending {
            if usage.workbuddy { let _ = credit.send(bytes.len() + proto::FRAME_OVERHEAD_BYTES); }
            usage.feed(&bytes, &telemetry);
            queue.push_back(Bytes::from(bytes));
        }
        Self {
            host,
            lease: Some(lease),
            conversation: None,
            id,
            events,
            credit,
            pending: queue,
            ended: false,
            telemetry,
            usage,
            failure,
            private_failure,
            raw: false,
            workbuddy: false,
            workbuddy_owner: None,
            workbuddy_tail: None,
            workbuddy_synthetic: false,
            settling: None,
        }
    }

    pub(super) fn with_workbuddy(mut self, owner: Option<workbuddy::Owner>) -> Self {
        self.workbuddy = owner.is_some();
        self.workbuddy_owner = owner;
        // constructor中已嗅探pending首块，此时补同一处理，不允许首块绕过记账。
        if self.workbuddy { self.capture_frame_failure(); self.wait_workbuddy_tail(); }
        self
    }

    fn start_settlement(&mut self) {
        if !self.workbuddy || self.settling.is_some() || self.workbuddy_owner.is_none() { return; }
        if self.telemetry.is_cancelled() {
            self.capture_failure(crate::server::core::upstream::cancellation::cancelled_error());
        }
        let error = self.failure.as_ref().and_then(|slot| slot.lock().ok().and_then(|value| value.clone()));
        if error.is_none() { return; }
        if let (Some(error), Some(owner), Some(slot)) = (error, self.workbuddy_owner.take(), self.failure.clone()) {
            self.settling = Some(owner.settle(error, slot));
        }
    }

    fn workbuddy_error(&self) -> Option<GatewayError> {
        self.failure.as_ref().and_then(|slot| slot.lock().ok().and_then(|error| error.clone()))
    }

    fn wait_workbuddy_tail(&mut self) {
        if !self.workbuddy || self.workbuddy_owner.is_none() || self.workbuddy_tail.is_some()
            || self.workbuddy_error().is_none() { return; }
        if self.ended { self.start_settlement(); return; }
        self.workbuddy_tail = Some(WorkbuddyTail {
            until: Box::pin(tokio::time::sleep(std::time::Duration::from_secs(1))), events: 0, bytes: 0,
        });
    }

    fn finish_workbuddy_tail(&mut self) {
        self.workbuddy_tail = None;
        self.close_conversation();
        self.usage.finish(&self.telemetry);
        self.capture_frame_failure();
        self.ended = true;
        self.start_settlement();
    }

    fn poll_workbuddy_tail(&mut self, context: &mut Context<'_>) -> Poll<()> {
        loop {
            let Some(tail) = &mut self.workbuddy_tail else { return Poll::Ready(()); };
            if self.telemetry.is_cancelled() {
                self.capture_failure(crate::server::core::upstream::cancellation::cancelled_error());
                self.finish_workbuddy_tail();
                return Poll::Ready(());
            }
            // 宿主WASM分片最多64KiB；读下一片前预留完整一片，尾部不会越过256KiB。
            if tail.until.as_mut().poll(context).is_ready() || tail.events >= 16
                || tail.bytes.saturating_add(64 * 1024) > 256 * 1024 {
                self.finish_workbuddy_tail();
                return Poll::Ready(());
            }
            let event = match self.events.poll_recv(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(event) => event,
            };
            if let Some(tail) = &mut self.workbuddy_tail { tail.events += 1; }
            match event {
                Some(StreamEvent::Chunk(bytes)) => {
                    let _ = self.credit.send(bytes.len() + proto::FRAME_OVERHEAD_BYTES);
                    if let Some(tail) = &mut self.workbuddy_tail { tail.bytes = tail.bytes.saturating_add(bytes.len()); }
                    self.usage.feed(&bytes, &self.telemetry);
                    self.capture_frame_failure();
                    self.pending.push_back(Bytes::from(bytes));
                }
                Some(StreamEvent::Head(_)) => {},
                Some(StreamEvent::Done(Err(error))) => {
                    self.capture_failure(super::errors::workbuddy_stream::control(error));
                    self.finish_workbuddy_tail();
                    return Poll::Ready(());
                }
                Some(StreamEvent::Done(Ok(_))) => {
                    self.finish_workbuddy_tail();
                    return Poll::Ready(());
                }
                Some(StreamEvent::Failed(_)) | None => {
                    self.capture_failure(GatewayError::with_status(502, "WorkBuddy流在错误终态确认前中断")
                        .with_code("runtime_stream_interrupted"));
                    self.finish_workbuddy_tail();
                    return Poll::Ready(());
                }
            }
        }
    }

    pub(super) fn with_conversation(mut self, owner: Option<ConversationOwner>) -> Self {
        self.conversation = owner;
        self
    }

    fn close_conversation(&mut self) { drop(self.conversation.take()); }

    pub(super) fn raw(mut self) -> Self {
        self.raw = true;
        self
    }

    fn capture_failure(&self, error: GatewayError) {
        if let Some(slot) = &self.failure {
            let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.workbuddy {
                match slot.as_mut() {
                    Some(previous) => super::errors::workbuddy_stream::merge(previous, error),
                    None => *slot = Some(error),
                }
                return;
            }
            // 已确认的取消/超时/清理失败不能被更早的额度帧覆盖。
            if slot.is_none() || self.usage.strict_provider.is_some() && strict::stopped(&error)
                && !slot.as_ref().is_some_and(|prior| matches!(prior.status_code, 499 | 504)) {
                *slot = Some(error);
            }
        }
    }

    fn capture_frame_failure(&self) {
        if self.workbuddy {
            if let Some(error) = &self.usage.workbuddy_failure { self.capture_failure(error.clone()); }
            return;
        }
        let Some(error) = &self.usage.failure else { return };
        if let Some(provider) = self.usage.strict_provider {
            if let Some((failure, private)) = strict::stream_failure(provider, &json!({ "error": error })) {
                if let Some(slot) = &self.private_failure {
                    let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if slot.is_none() || strict::stopped(&failure) { *slot = Some(private); }
                }
                self.capture_failure(failure);
            }
            return;
        }
        let status = error.get("status").or_else(|| error.get("statusCode")).and_then(Value::as_u64)
            .and_then(|status| i32::try_from(status).ok()).filter(|status| (400..600).contains(status)).unwrap_or(502);
        let mut failure = GatewayError::with_status(status,
            error.get("message").and_then(Value::as_str).unwrap_or("上游流式返回错误"));
        failure.code = error.get("code").and_then(|value| value.as_str().map(str::to_string)
            .or_else(|| value.as_i64().map(|code| code.to_string())))
            .or_else(|| error.get("type").and_then(Value::as_str).map(str::to_string));
        failure.upstream_code = error.get("upstreamCode").or_else(|| error.get("upstream_code"))
            .and_then(Value::as_i64).or_else(|| error.get("code").and_then(Value::as_i64));
        self.capture_failure(failure);
    }

    /// 排上「错误帧 + [DONE]」收尾。
    fn finish_with_error(&mut self, message: &str) {
        self.close_conversation();
        self.usage.finish(&self.telemetry);
        self.capture_frame_failure();
        if let Some(provider) = self.usage.strict_provider {
            let failure = strict::fatal(provider, 502, "runtime_stream_interrupted");
            self.finish_strict_error(failure);
            return;
        }
        if self.workbuddy {
            if self.workbuddy_error().is_none() {
                self.capture_failure(GatewayError::with_status(502, message).with_code("runtime_stream_interrupted"));
            }
            self.ended = true;
            self.workbuddy_synthetic = !self.usage.done;
            self.start_settlement();
            return;
        }
        self.capture_failure(GatewayError::with_status(502, message).with_code("runtime_stream_interrupted"));
        self.ended = true;
        logging::console_line("[Model]", &format!("❌ {message}"));
        self.telemetry.note_error(message);
        if self.usage.done {
            // 插件自己已经收尾过了：再写一条会让客户端收到两个终止信号
            return;
        }
        self.pending.push_back(crate::server::core::upstream::sse::sse_frame(&json!({
            "error": {
                "message": message,
                "type": "proxy_error",
            }
        })));
        if self.failure.is_none() {
            self.pending.push_back(Bytes::from_static(b"data: [DONE]\n\n"));
        }
    }
    fn finish_strict_error(&mut self, failure: GatewayError) {
        self.close_conversation();
        let failure = self.failure.as_ref().and_then(|slot| {
            let guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.as_ref().filter(|prior| matches!(prior.status_code, 499 | 504)).cloned()
        }).unwrap_or(failure);
        if strict::stopped(&failure) {
            if let Some(slot) = &self.private_failure {
                slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
            }
        }
        self.capture_failure(failure.clone());
        self.ended = true;
        logging::console_line("[Model]", &failure.message);
        self.telemetry.note_error(&failure.message);
        if !self.usage.done {
            self.pending.push_back(crate::server::core::upstream::sse::sse_frame(&failure.payload()));
        }
    }
}

impl Stream for RpcStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        loop {
            if this.workbuddy_tail.is_some() && this.poll_workbuddy_tail(context).is_pending() { return Poll::Pending; }
            if let Some(settling) = &mut this.settling {
                if settling.as_mut().poll(context).is_pending() { return Poll::Pending; }
                this.settling = None;
            }
            if this.workbuddy_synthetic {
                this.workbuddy_synthetic = false;
                if let Some(error) = this.workbuddy_error() {
                    this.pending.push_back(super::errors::workbuddy_stream::stream_frame(&error));
                }
            }
            if let Some(frame) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(frame)));
            }
            if this.ended {
                this.close_conversation();
                return Poll::Ready(None);
            }
            if this.raw {
                match this.events.poll_recv(context) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Some(StreamEvent::Chunk(bytes))) => {
                        let _ = this.credit.send(bytes.len() + proto::FRAME_OVERHEAD_BYTES);
                        return Poll::Ready(Some(Ok(Bytes::from(bytes))));
                    }
                    Poll::Ready(Some(StreamEvent::Done(Ok(_)))) => {
                        this.close_conversation(); this.ended = true; return Poll::Ready(None);
                    }
                    Poll::Ready(Some(StreamEvent::Head(_))) => continue,
                    event => {
                        this.close_conversation();
                        let message = match event {
                            Poll::Ready(Some(StreamEvent::Done(Err(error)))) => error.message,
                            Poll::Ready(Some(StreamEvent::Failed(message))) => message,
                            _ => "插件流在收尾信号之前中断".to_string(),
                        };
                        this.ended = true;
                        return Poll::Ready(Some(Err(std::io::Error::other(message))));
                    }
                }
            }
            match this.events.poll_recv(context) {
                Poll::Pending => return Poll::Pending,
                // 通道关了却没有 `Done`：宿主被清表（进程没了 / 主动关停）。
                // 不合成错误的话，客户端会把截断当成正常结束。
                Poll::Ready(None) => this.finish_with_error("插件流在收尾信号之前中断"),
                // 重复的 head（插件协议违规）：消费者只认第一条，后续丢弃。
                // 不报错是因为它不影响正文 —— 一条多余的控制帧不该毁掉整条流。
                Poll::Ready(Some(StreamEvent::Head(_))) => {}
                Poll::Ready(Some(StreamEvent::Chunk(bytes))) => {
                    // 归还等量额度：载荷 + 帧开销（与插件侧计费口径逐字一致，
                    // 差一个常量就会让窗口缓慢泄漏或溢出）
                    let _ = this.credit.send(bytes.len() + proto::FRAME_OVERHEAD_BYTES);
                    this.usage.feed(&bytes, &this.telemetry);
                    this.capture_frame_failure();
                    this.telemetry.note_first_frame();
                    if this.workbuddy {
                        this.wait_workbuddy_tail();
                        if this.workbuddy_tail.is_some() || this.settling.is_some() {
                            this.pending.push_back(Bytes::from(bytes));
                            continue;
                        }
                    }
                    return Poll::Ready(Some(Ok(Bytes::from(bytes))));
                }
                Poll::Ready(Some(StreamEvent::Failed(message))) => {
                    this.close_conversation();
                    if let Some(provider) = this.usage.strict_provider {
                        this.usage.finish(&this.telemetry);
                        this.capture_frame_failure();
                        this.finish_strict_error(strict::fatal(provider, 502, "runtime_stream_interrupted"));
                    } else { this.finish_with_error(&message); }
                }
                Poll::Ready(Some(StreamEvent::Done(Ok(_)))) => {
                    this.close_conversation();
                    this.usage.finish(&this.telemetry);
                    this.capture_frame_failure();
                    if this.workbuddy { this.start_settlement(); }
                    // 插件正常结束。它本该自己写 `[DONE]`；漏了就在这里补上
                    // —— 少一个终止帧会让客户端一直等（连接挂着不关）。
                    if !this.usage.done {
                        if this.failure.is_some() {
                            this.finish_with_error("辅助程序流缺少真实 [DONE]，未补成功终态");
                            continue;
                        }
                        this.pending.push_back(Bytes::from_static(b"data: [DONE]\n\n"));
                    }
                    this.ended = true;
                }
                Poll::Ready(Some(StreamEvent::Done(Err(error)))) => {
                    this.close_conversation();
                    if this.workbuddy || this.usage.strict_provider.is_some() {
                        this.usage.finish(&this.telemetry);
                        this.capture_frame_failure();
                    }
                    if let Some(provider) = this.usage.strict_provider {
                        if let Some((_, private)) = strict::private_control(&error) {
                            if let Some(slot) = &this.private_failure {
                                let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                                if slot.is_none() { *slot = Some(private); }
                            }
                        }
                        let failure = super::rpc::rpc_error(provider, error, proto::methods::gateway::CONVERSATION);
                        this.finish_strict_error(failure);
                    } else {
                        let failure = if this.workbuddy { super::errors::workbuddy_stream::control(error.clone()) }
                            else { super::rpc::rpc_error("", error.clone(), proto::methods::gateway::CONVERSATION) };
                        this.capture_failure(failure);
                        this.finish_with_error(&error.message);
                    }
                }
            }
        }
    }
}

impl Drop for RpcStream {
    fn drop(&mut self) {
        self.close_conversation();
        let unfinished = self.host.close_stream(self.id);
        if unfinished {
            let host = self.host.clone();
            let id = self.id;
            let lease = self.lease.take();
            crate::spawn_task(async move {
                let _lease = lease;
                host.abort(id).await;
            });
        }
    }
}
