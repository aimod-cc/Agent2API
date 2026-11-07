//! 进行中行的**断线兜底守卫**（参考 OmniProxy 的 `res.on('close')` 收尾链）。
//!
//! ── 它解决什么 ──────────────────────────────────────────────
//! `record_started` 插了 status=0 的行之后，收尾只有两条路：handler 走到
//! 记账点（`pipeline::record_entry` / `error_response`），或流式路径由
//! `RecordingStream` 在流结束时 settle。而客户端在**响应产生之前**放弃连接
//! 时，axum/hyper 会直接丢弃 handler future（官方语义：连接关闭即取消
//! handler，见 axum discussions #1094 / #2811）—— 两条路都走不到，行永远
//! 停在「进行中」（只能等 1 小时的僵尸清扫，期间列表里像一条永远在跑的请求）。
//! 这正是用户实测到的「一排不终止的进行中」。
//!
//! 本守卫把「handler 被丢弃」变成**第三条收尾路径**：Drop 读取最后的实时快照，
//! 与 408 终态在同一事务内保存，不依赖限频队列是否已经写入。
//! 没有进行中行时不新建记录，也不覆盖已经完成的请求。
//!
//! ── 三条出口（正常路径必须显式选一条）───────────────────────
//!   - [`DisconnectGuard::complete`]：记账已经完成（非流式 / 转发失败）→
//!     解除兜底并**注销取消令牌**（请求已不在途）；
//!   - [`DisconnectGuard::handoff`]：流式路径 —— 响应体已交给 axum，收尾归
//!     `RecordingStream`（它有自己的 Drop 兜底），本守卫不再写库；**但令牌
//!     保持登记** —— 流还没跑完，这条请求仍然可以被「终止请求」终止
//!     （注销由 `RecordingStream::settle` 完成）；
//!   - 什么都不调（Drop）：handler 被取消 → 补 408 终态 + 注销令牌。
//!
//! ── 为什么用 Drop 而不是显式取消钩子 ─────────────────────────
//! axum 不提供「客户端断开」的回调（那是 Node `res.on('close')` 才有的东西），
//! handler 被取消是唯一可观察的信号，而 Drop 是它在 Rust 里的落点。
//! 守卫本身是同步的（只写库），在 Drop 里跑没有 async 的禁忌。
//!
//! 守卫与 handler 同寿命，实时快照只在异常收尾时读取；流式交接后不再记账。

use std::sync::Arc;

use crate::server::core::upstream::{cancellation, usage::RequestTelemetry};
use crate::server::request_stats::RequestStats;

use super::pipeline;

/// 客户端在**响应产生之前**放弃连接时的错误摘要（本守卫写入）。
///
/// 与 `pipeline::STREAM_ABORTED` 的分工：那条是「流已经开始下发、中途被
/// 丢掉」，这条是「连响应头都还没发出、handler 直接被取消」——两种中断发生在
/// 完全不同的阶段，文案分开才能一眼看出断在哪一段。
pub const DISCONNECTED: &str = "客户端在响应完成前断开连接";

/// 「进行中」行的断线兜底守卫（语义与三条出口见模块头）。
pub struct DisconnectGuard {
    stats: Arc<RequestStats>,
    /// 本请求的关联 id（与进行中行、取消令牌注册表同一个键）
    id: String,
    telemetry: Arc<RequestTelemetry>,
    started_at: i64,
    /// 是否仍处于「转发还没产生响应」的阶段（armed）
    armed: bool,
}

impl DisconnectGuard {
    /// 在 `record_started` 之后创建（此刻进行中行已经在库里）。
    pub fn new(stats: Arc<RequestStats>, telemetry: Arc<RequestTelemetry>, started_at: i64) -> Self {
        Self { stats, id: telemetry.id(), telemetry, started_at, armed: true }
    }

    /// 记账已完成 —— 解除兜底并注销取消令牌（请求不再在途）。
    pub fn complete(&mut self) {
        self.armed = false;
        cancellation::unregister(&self.id);
    }

    /// 收尾移交给响应流 —— 解除兜底但**保留**取消令牌的登记：流还没跑完，
    /// 这条请求仍然可以被「终止请求」终止（长回答的流式请求正是最需要终止的
    /// 那类）。注销归 `pipeline::RecordingStream::settle`（流结束时）。
    pub fn handoff(&mut self) {
        self.armed = false;
    }
}

impl Drop for DisconnectGuard {
    fn drop(&mut self) {
        // armed 走到这里 = handler 在响应产生前被取消（客户端断开 / 服务退出）：
        // 补一条明确的终态并注销令牌，别让这一行永远停在「进行中」。
        // handoff 之后（armed = false）这里**什么都不做** —— 流的生命周期
        // 比 handler 长，注销与收尾都归 `RecordingStream`。
        if self.armed {
            let mut snapshot = self.telemetry.snapshot();
            let error = snapshot.error.clone().unwrap_or_else(|| {
                if self.telemetry.cancel_token().is_some_and(|token| token.is_cancelled()) {
                    cancellation::MANUAL_TERMINATED.to_string()
                } else {
                    DISCONNECTED.to_string()
                }
            });
            let error = if self.telemetry.capture().is_some() || crate::server::core::debug_traffic::enabled() {
                error
            } else {
                pipeline::truncate_chars(&error, pipeline::ERROR_SUMMARY_CHARS)
            };
            let finished_at = crate::server::logging::now_ms();
            snapshot.finish_trace(finished_at, Some(&error));
            let progress = pipeline::running_progress(&snapshot, self.started_at);
            // 必须与终态同事务保存，不能等限频队列再写回。
            crate::server::core::blocking::in_place(|| {
                self.stats.finalize_interrupted(&self.id, &error, &progress, finished_at);
            });
            cancellation::unregister(&self.id);
        }
    }
}
