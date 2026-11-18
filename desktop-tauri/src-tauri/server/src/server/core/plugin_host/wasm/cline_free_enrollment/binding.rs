//! Cline Free 首次明确登记的安装与运行绑定；不构造管理授权、账号或续期资格。
//! 外层必须持原回环本机会话来源；本片不读取文件、ensure 或重选运行实例。

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

const PROVIDER: &str = "cline-free";
const REQUIRED: &[&str] = &["accounts.cas.v1", "accounts.identity.v1", "login.forms.v1", "credential.cline-free-refresh.v1"];

/// 字段私有，无 Clone/serde；包名、路径、摘要或后来同内容实例均不能恢复此绑定。
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
        // 仅借内存中已登记的 Free 执行体；不读取登记文件、目录或启动备用 Host。
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
        // Lease 只能来自实际 ZIP 安装提交；缺首次新 Host 的启动回执直接拒绝。
        let lease = provenance::try_current(&pack).map_err(install_error)?;
        let artifact = lease.artifact();
        // wasm_provider 同时要求包只声明一个身份；Free 不能借用另一额度池的包。
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
        // 原安装对象必须仍在内存见证表；变更中、已撤销或来源已替换均拒绝。
        let _install = binding.lease.try_hold().map_err(install_error)?;
        drop(_install);
        Ok(binding)
    }

    /// 只公开原包身份、版本和三份真实摘要；不公开声明、路径、凭据或配置。
    pub(super) fn package(&self) -> Value {
        let artifact = self.lease.artifact();
        json!({
            "id": artifact.pack(), "version": artifact.version(),
            "archiveDigest": hex(artifact.archive_digest()),
            "moduleDigest": self.module_digest,
            "declarationDigest": hex(&self.source_digest),
        })
    }

    /// 锁序 HostLifecycle → RuntimeTryRead → 安装见证；外层仍须持原本机会话门。
    /// action 仅作短同步准入，不得等待、读库/文件、联网、spawn 或缓存成提交许可。
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

    /// 只返回原 Host 的强引用供门外持活动保护；不保留锁、不保证后续提交仍有效。
    pub(super) fn host(&self) -> Result<Arc<Host>, EnrollmentError> {
        let host = self.host.upgrade().ok_or(EnrollmentError::Unavailable)?;
        self.with_current(|_| Ok(host))
    }

    /// 关闭筛选只比较原对象指针；不升级弱引用、查表或签发运行资格。
    pub(super) fn belongs_to_host(&self, host: &Host) -> bool {
        std::ptr::eq(self.host.as_ptr(), host)
    }

    /// 已持原 HostLifecycle/RuntimeTryRead；只核内存来源，不证明账号/数据库事务。
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
