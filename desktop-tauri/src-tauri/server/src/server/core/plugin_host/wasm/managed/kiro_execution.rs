//! Kiro method0 原宿主调用的执行占用；只由真实 Host/Runtime/Call 入口捕获。
//! 不持强 Call，不从 flow 或管理 scope 找执行体；原 Session/输入终检由 consume 完成。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::super::{context::Shared, runtime::Runtime};
use crate::server::{
    core::{
        login::managed::Call,
        plugin_host::{activity::UsageLease, Host},
    },
    errors::GatewayError,
};

#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct KiroExecution {
    host: Arc<Host>,
    // 显式 take 后才归还 active；不依赖字段声明顺序或普通 Active 的析构。
    runtime: Option<Arc<Runtime>>,
    shared: Option<Arc<Shared>>,
    usage: Option<Arc<UsageLease>>,
    active_counted: AtomicBool,
    generation: u64,
    identity: usize,
    declaration_digest: [u8; 32],
    module_digest: String,
    flow: String,
    id: String,
    expected: String,
    method: &'static str,
    deadline: Instant,
    cancellation: CancellationToken,
}

impl KiroExecution {
    pub(in crate::server::core::plugin_host::wasm) fn capture(
        host: &Arc<Host>,
        runtime: &Arc<Runtime>,
        call: &Call,
    ) -> Result<Arc<Self>, GatewayError> {
        if call.provider() != "kiro"
            || call.method_index() != 0
            || [call.flow(), call.id(), call.expected()]
                .iter()
                .any(|value| value.is_empty() || value.len() > 256)
        {
            return Err(unavailable());
        }
        let generation = host.generation.load(Ordering::Acquire);
        let identity = Arc::as_ptr(&runtime.shared) as usize;
        // 此处在 Host 门外一次核对原 Call 的真实运行绑定；with_current 不再进入 Session。
        call.runtime_matches(identity, generation)?;
        let module_digest = runtime.module_digest();
        if generation == 0
            || module_digest.len() != 64
            || !module_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(unavailable());
        }
        // Activity 内部有自己的锁，必须先在 Host lifecycle 外取得占用。
        let usage = host.activity.acquire().ok_or_else(unavailable)?;
        let execution = Arc::new(Self {
            host: host.clone(),
            runtime: Some(runtime.clone()),
            shared: Some(runtime.shared.clone()),
            usage: Some(usage),
            active_counted: AtomicBool::new(false),
            generation,
            identity,
            declaration_digest: *runtime.shared.options.source_digest(),
            module_digest: module_digest.to_string(),
            flow: call.flow().to_string(),
            id: call.id().to_string(),
            expected: call.expected().to_string(),
            method: call.method(),
            deadline: call.deadline(),
            cancellation: call.cancellation(),
        });
        // 所有强引用与失败清理所有者已在门外建立；失败先退门，才析构 execution。
        let admitted = (|| {
            let _lifecycle = host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let slot = host
                .embedded
                .runtime
                .try_read()
                .map_err(|_| unavailable())?;
            execution.check_locked(slot.as_ref())?;
            host.embedded
                .active
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                    active.checked_add(1)
                })
                .map_err(|_| unavailable())?;
            execution.active_counted.store(true, Ordering::Release);
            Ok::<(), GatewayError>(())
        })();
        admitted?;
        Ok(execution)
    }

    /// 只进入原 Host lifecycle → 原 Runtime read；action 必须是短同步消费。
    /// action 在此门内调用 input.consume(call)，不得联网、等待或递归执行来源核对。
    /// 不以这些只读 Call 索引代替 consume 的私有 Session/Material/Receipt 对象终检。
    pub(in crate::server::core::plugin_host::wasm) fn with_current<R>(
        &self,
        call: &Call,
        action: impl FnOnce() -> Result<R, GatewayError>,
    ) -> Result<R, GatewayError> {
        if !self.matches_call(call)
            || !self.active_counted.load(Ordering::Acquire)
            || self.cancellation.is_cancelled()
            || Instant::now() >= self.deadline
            || self.host.activity.retired()
        {
            return Err(unavailable());
        }
        // retired 的 Activity 锁已在外面释放；关闭最终仲裁使用同一 Host lifecycle。
        let result = {
            let _lifecycle = self.host.lifecycle.try_lock().map_err(|_| unavailable())?;
            let slot = self
                .host
                .embedded
                .runtime
                .try_read()
                .map_err(|_| unavailable())?;
            self.check_locked(slot.as_ref())?;
            action()
        };
        // 返回值/错误交由退门后的调用者处理，不在这里验收候选正文或处理网络结果。
        result
    }

    pub(in crate::server::core::plugin_host::wasm) fn host(&self) -> &Arc<Host> { &self.host }

    pub(in crate::server::core::plugin_host::wasm) fn belongs_to_host(&self, host: &Host) -> bool {
        std::ptr::eq(Arc::as_ptr(&self.host), host)
    }

    fn matches_call(&self, call: &Call) -> bool {
        call.provider() == "kiro"
            && call.method_index() == 0
            && call.flow() == self.flow
            && call.id() == self.id
            && call.expected() == self.expected
            && call.method() == self.method
            && call.deadline() == self.deadline
    }

    // 调用者已持生命周期与运行槽只读门；不查全局 Host 表，不读文件或配置。
    fn check_locked(&self, current: Option<&Arc<Runtime>>) -> Result<(), GatewayError> {
        let runtime = self.runtime.as_ref().ok_or_else(unavailable)?;
        let shared = self.shared.as_ref().ok_or_else(unavailable)?;
        let declared = self.host.options.wasm.as_ref().ok_or_else(unavailable)?;
        let options = &shared.options;
        if !self.host.ready()
            || self.host.closing.load(Ordering::Acquire)
            || self.host.disabled.load(Ordering::Acquire)
            || crate::server::core::plugin_host::runtime::admission_closed()
            || self.host.options.script
            || self.generation == 0
            || self.host.generation.load(Ordering::Acquire) != self.generation
            || current.is_none_or(|current| !Arc::ptr_eq(current, runtime))
            || !Arc::ptr_eq(&runtime.shared, shared)
            || Arc::as_ptr(shared) as usize != self.identity
            || self.cancellation.is_cancelled()
            || shared.cancel.is_cancelled()
            || Instant::now() >= self.deadline
            || options.provider != "kiro"
            || declared.provider != "kiro"
            || options.source_digest() != &self.declaration_digest
            || declared.source_digest() != &self.declaration_digest
            || runtime.module_digest() != self.module_digest
            || !options.allows("login.managed.v1")
            || !options.allows("login.loopback.v1")
            || !declared.allows("login.managed.v1")
            || !declared.allows("login.loopback.v1")
            || options
                .manifest
                .managed_login
                .as_ref()
                .is_none_or(|declaration| !declaration.methods.contains(&0))
        {
            return Err(unavailable());
        }
        Ok(())
    }
}

impl Drop for KiroExecution {
    fn drop(&mut self) {
        // 最后一份 Arc 退出时，先放下本对象的运行实例、共享资源与使用租约，
        // 再归还活动计数。Host 本身保留至 notify 之后，不能提前表现为空闲。
        drop(self.runtime.take());
        drop(self.shared.take());
        drop(self.usage.take());
        if self.active_counted.swap(false, Ordering::AcqRel) {
            self.host.embedded.active.fetch_sub(1, Ordering::AcqRel);
        }
        self.host.embedded.idle.notify_waiters();
    }
}

fn unavailable() -> GatewayError {
    GatewayError::with_status(
        409,
        "Kiro 原托管执行来源已结束、换代或无法确认；不重选执行体，也不重放本次材料",
    )
    .with_code("kiro_execution_unavailable")
}
