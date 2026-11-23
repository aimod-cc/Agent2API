//! 请求级禁止重放：真实调用的任务准备与固定消息投递共用同一宿主安全对象。
//!
//! 一份入站请求共享一个 ReplayGuard；账号/提供商尝试、视觉子调用和续写不能重建它。
//! 开启调用、撤销调用、准备冻结共用一把短锁，锁内不 await、不调用外部代码或网络。
//! blocked 只表示不得再开调用，不证明创建已发送；进入准备后即使得到 NotSent 也不解除。
//! 这不是跨入站请求的幂等机制，也不是任务提交/停止许可；后两者仍由受控执行器裁决。

use std::sync::{Arc, Mutex, MutexGuard};

/// 仅作早停返回值；是否禁止重放仍须读取宿主安全对象，不能按错误码推断。
pub(super) fn blocked_error() -> crate::server::errors::GatewayError {
    crate::server::errors::GatewayError::with_status(409, "本次请求已禁止重放，未开启新的对话调用")
        .with_code("request_replay_blocked")
}

/// 仅供宿主处理的拒绝原因，不从插件错误码、JSON 或字符串恢复权限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReplayError {
    Blocked,
    CallActive,
    Revoked,
    /// 锁中毒或调用代次耗尽；永久关闭准入，不尝试恢复。
    Unavailable,
}

/// 不可序列化的请求安全对象。只保存当前调用和永久禁止位，不保存每轮历史或业务 ID。
pub(crate) struct ReplayGuard {
    state: Mutex<State>,
}

struct State {
    blocked: bool,
    permanent_block: bool,
    last_generation: u64,
    active_generation: Option<u64>,
    frozen_generation: Option<u64>,
    rejection_verified_generation: Option<u64>,
}

impl ReplayGuard {
    /// 只由 upstream 的请求上下文构造；子调用必须继承已有 Arc。
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                blocked: false,
                permanent_block: false,
                last_generation: 0,
                active_generation: None,
                frozen_generation: None,
                rejection_verified_generation: None,
            }),
        }
    }

    /// 只读判断，不能把 false 当作准入凭证；真正开启调用必须经过 open_call。
    /// 安全锁异常一律按已禁止处理，与可容忍丢失的用量统计锁不同。
    pub(crate) fn blocked(&self) -> bool {
        match self.lock() {
            Ok(state) => state.blocked || state.permanent_block,
            Err(_) => true,
        }
    }

    /// 同一请求只准一个活动调用；不会自动替换或撤销旧调用。
    /// 调用所有者必须先 close/drop，下一轮才能取得新的、不可复用的代次。
    pub(crate) fn open_call(self: &Arc<Self>) -> Result<CallLease, ReplayError> {
        let mut state = self.lock()?;
        if state.blocked || state.permanent_block {
            return Err(ReplayError::Blocked);
        }
        if state.active_generation.is_some() {
            return Err(ReplayError::CallActive);
        }
        let Some(generation) = state.last_generation.checked_add(1) else {
            state.blocked = true;
            state.permanent_block = true;
            state.active_generation = None;
            return Err(ReplayError::Unavailable);
        };
        state.last_generation = generation;
        state.active_generation = Some(generation);
        state.frozen_generation = None;
        state.rejection_verified_generation = None;
        Ok(CallLease { request: Arc::clone(self), generation })
    }

    fn revoke(&self, generation: u64) {
        if let Ok(mut state) = self.lock() {
            if state.active_generation == Some(generation) {
                state.active_generation = None;
            }
            if state.frozen_generation == Some(generation) {
                // 仅在宿主 HTTP 栈亲眼收到「未执行即拒绝」的状态码且无其他不安全事件时放行下一轮尝试；
                // 否则保持永久禁止重放，不从插件侧恢复发送资格。
                if !state.permanent_block && state.rejection_verified_generation == Some(generation) {
                    state.blocked = false;
                } else {
                    state.permanent_block = true;
                }
                state.frozen_generation = None;
                state.rejection_verified_generation = None;
            }
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, ReplayError> {
        match self.state.lock() {
            Ok(state) => Ok(state),
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.blocked = true;
                state.permanent_block = true;
                state.active_generation = None;
                state.rejection_verified_generation = None;
                // 不清除中毒标记，不把可能不完整的状态重新当成有效权限。
                Err(ReplayError::Unavailable)
            }
        }
    }
}

/// 唯一调用所有者；故意不实现 Clone，不能把它作为 worker 的共享引用。
/// 所有者应活到真实调用终止（流式含响应流），取消/超时则同步撤销。
#[must_use = "调用租约必须由真实调用所有者持有，结束时 close 或 drop"]
pub(crate) struct CallLease {
    request: Arc<ReplayGuard>,
    generation: u64,
}

impl CallLease {
    /// worker 只拿这份轻量引用；它不持有租约，不能延缓所有者 drop 时的撤销。
    /// 签发不是准入完成，freeze 会在同一把锁下重新核对当前活动代次。
    pub(crate) fn preparation(&self) -> PreparationRef {
        PreparationRef { request: Arc::clone(&self.request), generation: self.generation }
    }

    /// 同步撤销；只更新内存权限，不在 Drop 中安排网络清理。
    pub(crate) fn close(self) {
        drop(self);
    }
}

impl Drop for CallLease {
    fn drop(&mut self) {
        self.request.revoke(self.generation);
    }
}

/// 只能由调用租约签发的准备引用，字段私有且没有反序列化或按 ID 构造入口。
/// Arc 只保留请求安全状态，不保留所有者、遥测、账号材料或远程任务。
#[derive(Clone)]
pub(crate) struct PreparationRef {
    request: Arc<ReplayGuard>,
    generation: u64,
}

impl PreparationRef {
    /// 在宿主接受 Preparing、发布预留或启动 worker 之前调用；失败就不得进入准备。
    /// 必须在实际准备边界调用，不能缓存一次成功结果供未来任务当作提交许可。
    ///
    /// freeze 先赢：永久禁止后续调用，原调用仍可继续接收结果及承担停止责任。
    /// close/drop 先赢：即使下一代已开启，旧引用仍返回 Revoked，不能冻结新调用。
    /// 同一仍活跃调用的重复冻结幂等；本接口不代替执行器的一次性创建/停止许可。
    pub(crate) fn freeze(&self) -> Result<(), ReplayError> {
        let mut state = self.request.lock()?;
        // 必须先核对代次，不能因为 blocked 已为 true 就放行被撤销的旧引用。
        if state.active_generation != Some(self.generation) || state.permanent_block {
            return Err(ReplayError::Revoked);
        }
        if state.frozen_generation == Some(self.generation) {
            // 同一代次里重复冻结会锁死重放保护（但同代消费票重检不该用 freeze 重复触发）。
            state.blocked = true;
            return Ok(());
        }
        state.blocked = true;
        state.frozen_generation = Some(self.generation);
        state.rejection_verified_generation = None;
        Ok(())
    }

    /// 仅供宿主 HTTP 层调用：当单次出站明确收到「未开始流、未受理任务」的 HTTP 拒绝状态码时标记，
    /// 允许本轮租约撤销后释放跨账号/重试资格。
    pub(crate) fn confirm_unsubmitted_http_rejection(&self) {
        if let Ok(mut state) = self.request.lock() {
            if !state.permanent_block
                && state.active_generation == Some(self.generation)
                && state.frozen_generation == Some(self.generation)
            {
                state.rejection_verified_generation = Some(self.generation);
            }
        }
    }

    /// 永久锁死当前请求的重放资格（收到 2xx、流已开始、或发生结果未确认的网络异常）。
    pub(crate) fn lock_permanent_replay_block(&self) {
        if let Ok(mut state) = self.request.lock() {
            state.blocked = true;
            state.permanent_block = true;
            state.rejection_verified_generation = None;
        }
    }
}
