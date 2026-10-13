//! 上游**响应协议**的翻译层：上游吐的不是 chat SSE 时，先折成 chat SSE 再下发。
//!
//! ── 为什么在内置家的路径上需要它 ────────────────────────────
//! `ForwardStream` 与聚合器只认**标准 chat SSE**（它们是 SSE 逐行解析的唯一
//! 入口：reasoning 合并、usage 提取、model 回写都挂在那两层）。自定义提供商的
//! 翻译路径（`providers::custom::forward` 的 `ProtocolTranslateStream`）早已
//! 证明了这个结构，但那个实现在 `providers::custom` 下、并且要 slot /
//! connections 两个凭证参数 —— 内置家（ZCode 的活动套餐通道，见
//! `providers::zcode::plan`）由编排层移交凭证，因此这里给一个**不持凭证**的
//! 版本：构造它、交给 `ForwardStream::from_translated` 或聚合器，凭证仍由
//! `provider_loop` 那两处按既有规则处置。
//!
//! ── 本文件服务三种上游响应协议（三家各一台，互不共用）──────────
//!   1. **Anthropic Messages SSE**（`AnthropicToChatStream` ←
//!      `protocol::anthropic_outbound::ChatFromAnthropicStream`）：内置家里只有
//!      ZCode 的活动套餐通道说这套协议；
//!   2. **Command Code NDJSON**（`CommandCodeToChatStream` ←
//!      `protocol::commandcode_outbound::ChatFromCommandCodeStream`）：
//!      `api.commandcode.ai` 的 `/alpha/generate` 返回
//!      `application/x-ndjson`（一行一个 JSON 事件、**HTTP 恒 200**、错误在流内）。
//!      两台状态机不合并：行形态（有无 `data:` 前缀）与事件词表都不同，
//!      合并意味着在两套词表之间做映射，而它们本来就没有共同的上位概念。
//!   3. **Antigravity 的 Gemini v1internal SSE**（`AntigravityToChatStream` ←
//!      `protocol::antigravity_stream::ChatFromAntigravityStream`）：Google Cloud
//!      Code Assist 的 `:streamGenerateContent?alt=sse`，行形态是 SSE 但每帧是
//!      `{"response":{…gemini…}}` 信封（缺省回退顶层），字段路径（`candidates`
//!      / `usageMetadata` / `thought` / `thoughtSignature`）全是 Gemini 方言 ——
//!      与上面两台同样没有可复用的词表，另开一台。
//! 三台壳逐行同构（同一种错误处置、同一种空闲守卫、同一种调试采集），
//! 差别只有内部那台状态机与日志前缀 —— 因此下面三个 struct 读起来是镜像的。
//!
//! ── 与自定义家那条的三个差别（都是刻意的）────────────────────
//!   1. `model` 传**上游真名**（与自定义家一致）：帧里的 `model` 由下游的
//!      回写层按适配器的 `sse_model_rewrite()` 决定要不要改回请求名；
//!   2. `model` 之外不再接受任何 provider 参数：三台状态机各自认自己的事件词表，
//!      壳只负责搬运字节；
//!   3. 调试采集在这里采**上游原始字节**（翻译前），与 chat 路径「采上游原样
//!      吐出的东西」的语义一致 —— 翻译后的 chat 帧只是网关内部的中间形态。
//!
//! ── 硬盘约束 ────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures::Stream;

use crate::server::core::protocol::anthropic_outbound::ChatFromAnthropicStream;
use crate::server::core::protocol::antigravity_stream::ChatFromAntigravityStream;
use crate::server::core::protocol::commandcode_outbound::ChatFromCommandCodeStream;

/// 上游 Anthropic SSE → 标准 chat SSE 的字节流（`reqwest::Response` 直接进来）。
///
/// 管道结构：`响应字节 → 空闲守卫 → 转换器 → 帧队列`。错误在构造时就描述成
/// 文案折进 `io::Error`（与 `ForwardStream::new` 同一手法）：空闲守卫的入参是
/// 这个类型，下游两个消费层拿到的也是同一形态 —— 于是「上游断开」与「流式
/// 空闲超时」在流式路径由 `ForwardStream` 补错误帧收尾、在聚合路径转 502，
/// 与 chat 路径的错误语义逐条相同。
pub struct AnthropicToChatStream {
    /// 上游字节流（已描述错误、已装空闲守卫）
    inner: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    /// 协议状态机（上游事件 → chat 帧）
    machine: ChatFromAnthropicStream,
    /// 已翻译待下发的帧（一个上游 chunk 可能产出多帧）
    pending: VecDeque<Bytes>,
    /// 上游已结束（不再 poll 上游，把 pending 与收尾帧吐完即 None）
    upstream_done: bool,
    /// 调试模式的采集器（None = 未开启）
    capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
}

impl AnthropicToChatStream {
    /// `model` 是**上游真名**（下发帧里的 `model` 字段，见模块头第 2 条）。
    pub fn new(
        response: reqwest::Response,
        model: &str,
        telemetry: &Arc<super::usage::RequestTelemetry>,
    ) -> Self {
        use futures::StreamExt;
        // reqwest 错误就地描述成文案（折进 io::Error 之后只剩文本可读）；
        // 空闲守卫用设置页「请求超时」的「流式响应空闲超时」那一档 ——
        // 与 chat 路径、自定义家路径三处同源，改设置三处一起变
        let described = response.bytes_stream().map(|item| {
            item.map_err(|error| {
                std::io::Error::other(crate::server::core::egress::describe_error_detail(&error))
            })
        });
        let guarded = super::stall::idle_guard(
            Box::pin(described),
            Duration::from_millis(crate::server::config::timeout_settings().stream_idle_ms()),
        );
        Self {
            inner: guarded,
            machine: ChatFromAnthropicStream::new(model),
            pending: VecDeque::new(),
            upstream_done: false,
            capture: telemetry.capture(),
        }
    }
}

impl Stream for AnthropicToChatStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        use futures::StreamExt;
        loop {
            // 先把转换器攒下的帧发完，再拉上游 —— 顺序反了会让同一个上游 chunk
            // 产出的多帧乱序（工具调用的宣告帧必须先于参数帧）
            if let Some(frame) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(frame)));
            }
            if self.upstream_done {
                return Poll::Ready(None);
            }
            match self.inner.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    // 上游 EOF：转换器收尾（finish_reason / usage 帧 / [DONE]）
                    self.upstream_done = true;
                    for frame in self.machine.finish() {
                        self.pending.push_back(frame);
                    }
                }
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Some(capture) = &self.capture {
                        capture.push(&bytes);
                    }
                    for frame in self.machine.push(&bytes[..]) {
                        self.pending.push_back(frame);
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    self.upstream_done = true;
                    // 文案已在构造时描述好（见 inner 字段说明）：原样上抛，
                    // 由下游的流 / 聚合器转成错误帧或 502
                    return Poll::Ready(Some(Err(error)));
                }
            }
        }
    }
}

/// 上游 **Command Code NDJSON** → 标准 chat SSE 的字节流
/// （`reqwest::Response` 直接进来）。
///
/// 与 [`AnthropicToChatStream`] 逐行同构：管道结构同样是
/// `响应字节 → 空闲守卫 → 转换器 → 帧队列`，错误同样在构造时就描述成文案折进
/// `io::Error`，`pending` 队列同样保证「同一个上游 chunk 产出的多帧不乱序」
/// （工具的宣告帧必须先于参数帧 —— 本家虽是一次性 `tool-call` 事件，但收尾帧
/// 与 usage 帧也必须排在内容帧之后），EOF 时同样调 `machine.finish()`
/// 完成「收尾 / 报截断」的判定。
///
/// ── 唯一实质差别：上游 HTTP 恒 200 ───────────────────────────
/// 状态码不是成败判据（认证失败、限流、余额不足都在流内 `{"type":"error"}`），
/// 因此编排层**不**按 `response.status()` 分流错误 —— 它照旧把 200 与响应体
/// 一起交进来，成败由这台状态机折出的错误帧表达（聚合器据此转 502、
/// 流式路径原样把错误帧透给客户端）。
pub struct CommandCodeToChatStream {
    /// 上游字节流（已描述错误、已装空闲守卫）
    inner: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    /// 协议状态机（上游 NDJSON 事件 → chat 帧）
    machine: ChatFromCommandCodeStream,
    /// 已翻译待下发的帧（一个上游 chunk 可能产出多帧）
    pending: VecDeque<Bytes>,
    /// 上游已结束（不再 poll 上游，把 pending 与收尾帧吐完即 None）
    upstream_done: bool,
    /// 调试模式的采集器（None = 未开启）
    capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
}

impl CommandCodeToChatStream {
    /// `model` 是**上游真名**（下发帧里的 `model` 字段，见模块头）。
    pub fn new(
        response: reqwest::Response,
        model: &str,
        telemetry: &Arc<super::usage::RequestTelemetry>,
    ) -> Self {
        use futures::StreamExt;
        // reqwest 错误就地描述成文案（折进 io::Error 之后只剩文本可读）；
        // 空闲守卫用设置页「请求超时」的「流式响应空闲超时」那一档 ——
        // 与 chat 路径、Anthropic 那条三处同源，改设置三处一起变
        let described = response.bytes_stream().map(|item| {
            item.map_err(|error| {
                std::io::Error::other(crate::server::core::egress::describe_error_detail(&error))
            })
        });
        let guarded = super::stall::idle_guard(
            Box::pin(described),
            Duration::from_millis(crate::server::config::timeout_settings().stream_idle_ms()),
        );
        Self {
            inner: guarded,
            machine: ChatFromCommandCodeStream::new(model),
            pending: VecDeque::new(),
            upstream_done: false,
            capture: telemetry.capture(),
        }
    }
}

impl Stream for CommandCodeToChatStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        use futures::StreamExt;
        loop {
            // 先把转换器攒下的帧发完，再拉上游 —— 顺序反了会让同一个上游 chunk
            // 产出的多帧乱序（内容帧必须先于本轮的收尾帧 / 错误帧）
            if let Some(frame) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(frame)));
            }
            if self.upstream_done {
                return Poll::Ready(None);
            }
            match self.inner.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    // 上游 EOF：转换器收尾（正常 finish 帧 / usage 帧 / [DONE]，
                    // 或「没有完成事件」的截断错误帧 —— 判定在那台状态机里）
                    self.upstream_done = true;
                    for frame in self.machine.finish() {
                        self.pending.push_back(frame);
                    }
                }
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Some(capture) = &self.capture {
                        capture.push(&bytes);
                    }
                    for frame in self.machine.push(&bytes[..]) {
                        self.pending.push_back(frame);
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    self.upstream_done = true;
                    // 文案已在构造时描述好（见 inner 字段说明）：原样上抛，
                    // 由下游的流 / 聚合器转成错误帧或 502
                    return Poll::Ready(Some(Err(error)));
                }
            }
        }
    }
}

/// 上游 **Antigravity 的 Gemini v1internal SSE** → 标准 chat SSE 的字节流
/// （`reqwest::Response` 直接进来）。
///
/// 与 [`AnthropicToChatStream`] 逐行同构：管道结构同样是
/// `响应字节 → 空闲守卫 → 转换器 → 帧队列`，错误同样在构造时就描述成文案折进
/// `io::Error`，`pending` 队列同样保证「同一个上游 chunk 产出的多帧不乱序」
/// （收尾帧与 usage 帧必须排在内容帧之后），EOF 时同样调 `machine.finish()`
/// 完成「收尾 / 报截断」的判定。
///
/// ── 与另外两台的差别：上游状态码仍是成败判据 ─────────────────
/// 本家的流内错误（`data: {"error":{…}}`）也存在，但 HTTP 非 2xx 同样是真实
/// 错误（401/429/5xx）—— 编排层照旧按状态码分流，翻译状态机只负责把 **200 的
/// 流**折成 chat（流内错误另走错误帧）。这与 Command Code「HTTP 恒 200、成败
/// 全在流内」的那台不同。
pub struct AntigravityToChatStream {
    /// 上游字节流（已描述错误、已装空闲守卫）
    inner: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    /// 协议状态机（Gemini 帧 → chat 帧）
    machine: ChatFromAntigravityStream,
    /// 已翻译待下发的帧（一个上游 chunk 可能产出多帧）
    pending: VecDeque<Bytes>,
    /// 上游已结束（不再 poll 上游，把 pending 与收尾帧吐完即 None）
    upstream_done: bool,
    /// 调试模式的采集器（None = 未开启）
    capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
}

impl AntigravityToChatStream {
    /// `model` 是**上游真名**（下发帧里的 `model` 字段，见模块头）。
    pub fn new(
        response: reqwest::Response,
        model: &str,
        telemetry: &Arc<super::usage::RequestTelemetry>,
    ) -> Self {
        use futures::StreamExt;
        // reqwest 错误就地描述成文案（折进 io::Error 之后只剩文本可读）；
        // 空闲守卫用设置页「请求超时」的「流式响应空闲超时」那一档 ——
        // 与另外两台三处同源，改设置三处一起变
        let described = response.bytes_stream().map(|item| {
            item.map_err(|error| {
                std::io::Error::other(crate::server::core::egress::describe_error_detail(&error))
            })
        });
        let guarded = super::stall::idle_guard(
            Box::pin(described),
            Duration::from_millis(crate::server::config::timeout_settings().stream_idle_ms()),
        );
        Self {
            inner: guarded,
            machine: ChatFromAntigravityStream::new(model),
            pending: VecDeque::new(),
            upstream_done: false,
            capture: telemetry.capture(),
        }
    }
}

impl Stream for AntigravityToChatStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        use futures::StreamExt;
        loop {
            // 先把转换器攒下的帧发完，再拉上游 —— 顺序反了会让同一个上游 chunk
            // 产出的多帧乱序（内容帧必须先于本轮的收尾帧 / 错误帧）
            if let Some(frame) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(frame)));
            }
            if self.upstream_done {
                return Poll::Ready(None);
            }
            match self.inner.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    // 上游 EOF：转换器收尾（正常 finish 帧 / usage 帧 / [DONE]，
                    // 或「没有完成信号」的截断错误帧 —— 判定在那台状态机里）
                    self.upstream_done = true;
                    for frame in self.machine.finish() {
                        self.pending.push_back(frame);
                    }
                }
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Some(capture) = &self.capture {
                        capture.push(&bytes);
                    }
                    for frame in self.machine.push(&bytes[..]) {
                        self.pending.push_back(frame);
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    self.upstream_done = true;
                    // 文案已在构造时描述好（见 inner 字段说明）：原样上抛，
                    // 由下游的流 / 聚合器转成错误帧或 502
                    return Poll::Ready(Some(Err(error)));
                }
            }
        }
    }
}
