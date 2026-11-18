//! CodeArts 首次明确登记的安装与运行来源；不构造管理授权、账号或发送资格。
//! 所有使用仍须由外层原管理会话来源包裹；冷启动缺见证时不读文件、ensure 或重选。

use std::sync::{atomic::Ordering, Arc, TryLockError, Weak};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::EnrollmentError;
use crate::server::core::plugin_host::{
    self as plugin,
    packages::provenance::{self, InstallLease},
    wasm::{context::Shared, manifest, runtime::Runtime},
    Host,
};

const PROVIDER: &str = "codearts";
const REQUIRED: &[&str] = &["accounts.cas.v1", "accounts.identity.v1", "login.forms.v1"];

/// 字段私有，无 Clone/serde；弱身份不能按包名、摘要或后来同内容实例恢复。
pub(super) struct Binding {
    host: Weak<Host>,
    runtime: Weak<Runtime>,
    shared: Weak<Shared>,
    pub(super) stopped: CancellationToken,
    generation: u64,
    lease: Arc<InstallLease>,
    source_digest: [u8; 32],
    module_digest: String,
}

impl Binding {
    pub(super) fn capture() -> Result<Self, EnrollmentError> {
        if plugin::runtime::admission_closed() || plugin::registry::is_switching(PROVIDER) {
            return Err(EnrollmentError::Unavailable);
        }
        // 只选一次当前内存中已登记的执行体，不读取 plugins.json/安装目录。
        let pack = plugin::runtime::provider_pack(PROVIDER).ok_or(EnrollmentError::Unavailable)?;
        let host = plugin::runtime::host_for_pack(&pack).ok_or(EnrollmentError::Unavailable)?;
        if !plugin::runtime::host_for_provider(PROVIDER)
            .is_some_and(|owner| Arc::ptr_eq(&owner, &host))
            || !plugin::serves_provider(PROVIDER)
            || host.options.script
        {
            return Err(EnrollmentError::Unavailable);
        }
        let _lifecycle = host.lifecycle.try_lock().map_err(lock_error)?;
        let current = host
            .embedded
            .runtime
            .try_read()
            .map_err(|_| EnrollmentError::Busy)?;
        let runtime = current.as_ref().ok_or(EnrollmentError::Unavailable)?;
        let generation = host.generation.load(Ordering::Acquire);
        // Lease 只来自真实 ZIP 安装提交；StartedRuntime 必须来自该安装的首次新 Host。
        let lease = provenance::try_current(&pack).map_err(install_error)?;
        let artifact = lease.artifact();
        if artifact.pack() != pack
            || artifact.wasm_provider() != Some(PROVIDER)
            || artifact.target() != manifest::TARGET
        {
            return Err(EnrollmentError::Unavailable);
        }
        let source_digest =
            Sha256::digest(artifact.wasm_bytes().ok_or(EnrollmentError::Unavailable)?).into();
        let module_digest = hex(artifact
            .module_digest()
            .ok_or(EnrollmentError::Unavailable)?);
        let binding = Self {
            host: Arc::downgrade(&host),
            runtime: Arc::downgrade(runtime),
            shared: Arc::downgrade(&runtime.shared),
            stopped: runtime.shared.cancel.clone(),
            generation,
            lease,
            source_digest,
            module_digest,
        };
        binding.check_locked(&host, runtime)?;
        // 见证门内确认仍是原安装对象；登记变动、运行撤销或任何待提交安装均拒绝。
        let _install = binding.lease.try_hold().map_err(install_error)?;
        drop(_install);
        Ok(binding)
    }

    /// 只公开原包身份和三份真实摘要，不公开目录、声明正文或任何凭据。
    pub(super) fn package(&self) -> Value {
        let artifact = self.lease.artifact();
        json!({
            "id": artifact.pack(), "version": artifact.version(),
            "archiveDigest": hex(artifact.archive_digest()),
            "moduleDigest": self.module_digest,
            "declarationDigest": hex(&self.source_digest),
        })
    }

    /// 锁序 HostLifecycle → RuntimeTryRead → 安装见证。
    /// action 仅做短同步准入；不得读文件/库、网络、spawn 或把返回值当未来提交许可。
    pub(super) fn with_current<R>(
        &self,
        action: impl FnOnce(&Host) -> Result<R, EnrollmentError>,
    ) -> Result<R, EnrollmentError> {
        let host = self.host.upgrade().ok_or(EnrollmentError::Unavailable)?;
        let _lifecycle = host.lifecycle.try_lock().map_err(lock_error)?;
        let current = host
            .embedded
            .runtime
            .try_read()
            .map_err(|_| EnrollmentError::Busy)?;
        let runtime = current.as_ref().ok_or(EnrollmentError::Unavailable)?;
        self.check_locked(&host, runtime)?;
        let _install = self.lease.try_hold().map_err(install_error)?;
        action(&host)
    }

    /// 仅返回原 Host 的强引用，供 worker 在门外取得并实际持有活动凭证。
    /// 此复核不保留任何锁；真正消费仍须重新经过外层原会话和本绑定的准入门。
    pub(super) fn host(&self) -> Result<Arc<Host>, EnrollmentError> {
        let host = self.host.upgrade().ok_or(EnrollmentError::Unavailable)?;
        self.with_current(|_| Ok(host))
    }

    /// 退出筛选只比较原对象身份；不升级弱引用、不查表、不签发运行许可。
    pub(super) fn belongs_to_host(&self, host: &Host) -> bool {
        std::ptr::eq(self.host.as_ptr(), host)
    }

    /// 调用方已持原 HostLifecycle/RuntimeTryRead；本检查只核内存来源，不恢复任何资格。
    fn check_locked(
        &self,
        host: &Arc<Host>,
        runtime: &Arc<Runtime>,
    ) -> Result<(), EnrollmentError> {
        let options = &runtime.shared.options;
        if self.generation == 0
            || host.generation.load(Ordering::Acquire) != self.generation
            || !std::ptr::eq(self.host.as_ptr(), Arc::as_ptr(host))
            || !std::ptr::eq(self.runtime.as_ptr(), Arc::as_ptr(runtime))
            || !std::ptr::eq(self.shared.as_ptr(), Arc::as_ptr(&runtime.shared))
            || host.options.script
            || host.closing.load(Ordering::Acquire)
            || !host.ready()
            || host.activity.retired()
            || plugin::runtime::admission_closed()
            || self.stopped.is_cancelled()
            || runtime.shared.cancel.is_cancelled()
            || options.provider != PROVIDER
            || options.source_digest() != &self.source_digest
            || runtime.module_digest() != self.module_digest
            || REQUIRED
                .iter()
                .any(|capability| !options.allows(capability))
            || host.options.wasm.as_ref().is_none_or(|options| {
                options.provider != PROVIDER
                    || options.source_digest() != &self.source_digest
                    || REQUIRED
                        .iter()
                        .any(|capability| !options.allows(capability))
            })
            || !self
                .lease
                .started_runtime()
                .is_some_and(|started| started.matches(host, runtime, self.generation))
        {
            return Err(EnrollmentError::Unavailable);
        }
        Ok(())
    }
}

fn lock_error<T>(error: TryLockError<T>) -> EnrollmentError {
    match error {
        TryLockError::WouldBlock => EnrollmentError::Busy,
        TryLockError::Poisoned(_) => EnrollmentError::Unavailable,
    }
}

fn install_error(error: &'static str) -> EnrollmentError {
    if error == "安装见证检查忙" {
        EnrollmentError::Busy
    } else {
        EnrollmentError::Unavailable
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut value = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(value, "{byte:02x}");
    }
    value
}
