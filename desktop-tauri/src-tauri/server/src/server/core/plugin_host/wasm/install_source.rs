//! 启动票据只能创建新Host；回执只记录该对象首次发布的实际运行来源，不授予执行资格。
//! 捕获顺序为Host生命周期 -> RuntimeTryRead；其中只读内存，不访问文件或安装见证表。

use std::path::Path;
use std::sync::{atomic::Ordering, Arc, Weak};

use sha2::{Digest, Sha256};

use super::{context::Shared, runtime::Runtime};
use crate::server::core::plugin_host::{
    packages::provenance::VerifiedArtifact, GatewayCallbacks, Host, HostOptions,
};

struct Expected {
    provider: String,
    source_digest: [u8; 32],
    module_digest: String,
}

impl Expected {
    fn from_artifact(artifact: &VerifiedArtifact) -> Option<Self> {
        let provider = artifact.wasm_provider()?;
        let source_digest = Sha256::digest(artifact.wasm_bytes()?).into();
        let mut module_digest = String::with_capacity(64);
        for byte in artifact.module_digest()? {
            use std::fmt::Write;
            let _ = write!(module_digest, "{byte:02x}");
        }
        Some(Self { provider: provider.to_string(), source_digest, module_digest })
    }

    fn options_match(&self, host: &Host) -> bool {
        !host.options.script && host.options.wasm.as_ref().is_some_and(|options| {
            options.provider == self.provider && options.source_digest() == &self.source_digest
        })
    }

    fn runtime_matches(&self, runtime: &Runtime) -> bool {
        runtime.shared.options.provider == self.provider
            && runtime.shared.options.source_digest() == &self.source_digest
            && runtime.module_digest() == self.module_digest
    }
}

/// 只弱指向本工厂刚创建的Host，没有接收现成Host的构造器。
pub(in crate::server::core::plugin_host) struct NewHost {
    host: Weak<Host>,
    first_generation: u64,
    expected: Expected,
}

/// 字段私有，无Clone/serde；离开捕获临界区后仅作来源证据，必须再核对当前实例。
pub(in crate::server::core::plugin_host) struct StartedRuntime {
    host: Weak<Host>,
    runtime: Weak<Runtime>,
    shared: Weak<Shared>,
    generation: u64,
    expected: Expected,
}

/// 路径核对只在新Host尚未交出、未持任何生命周期/安装锁时进行。
/// 不匹配仍返回普通Host，但不给启动来源回执；注册和ensure沿用原装配流程。
pub(in crate::server::core::plugin_host) fn new_host(
    options: HostOptions,
    callbacks: Arc<dyn GatewayCallbacks>,
    artifact: &VerifiedArtifact,
    expected_entry: &Path,
) -> (Arc<Host>, Option<NewHost>) {
    let expected = Expected::from_artifact(artifact);
    let entry_matches = expected_entry.canonicalize().ok().is_some_and(|entry| {
        options.exe.canonicalize().ok().as_ref() == Some(&entry)
    });
    let host = Arc::new(Host::new(options, callbacks));
    let claim = expected.filter(|expected| entry_matches && expected.options_match(&host))
        .and_then(|expected| {
            let generation = host.generation.load(Ordering::Acquire);
            if generation != 0 { return None; }
            Some(NewHost { host: Arc::downgrade(&host), first_generation: generation.checked_add(1)?, expected })
        });
    (host, claim)
}

impl NewHost {
    pub(in crate::server::core::plugin_host) fn observe_loaded(self) -> Option<StartedRuntime> {
        let host = self.host.upgrade()?;
        let _lifecycle = host.lifecycle.lock().ok()?;
        let current = host.embedded.runtime.try_read().ok()?;
        let runtime = current.as_ref()?;
        let generation = host.generation.load(Ordering::Acquire);
        if super::super::runtime::admission_closed() || !host.ready()
            || generation != self.first_generation || !self.expected.options_match(&host)
            || !self.expected.runtime_matches(runtime)
        { return None; }
        Some(StartedRuntime {
            host: self.host, runtime: Arc::downgrade(runtime), shared: Arc::downgrade(&runtime.shared),
            generation, expected: self.expected,
        })
    }
}

impl StartedRuntime {
    /// 安装commit前可在见证表锁之外复核；不把失效对象替换成后来的同内容实例。
    pub(in crate::server::core::plugin_host) fn still_current(&self) -> bool {
        let Some(host) = self.host.upgrade() else { return false; };
        let Ok(_lifecycle) = host.lifecycle.lock() else { return false; };
        let Ok(current) = host.embedded.runtime.try_read() else { return false; };
        current.as_ref().is_some_and(|runtime| {
            !super::super::runtime::admission_closed() && host.ready()
                && self.matches(&host, runtime, host.generation.load(Ordering::Acquire))
        })
    }

    /// 调用方已持Host生命周期与当前Runtime读锁；本比较不再取锁，不导出裸指针。
    pub(super) fn matches(&self, host: &Arc<Host>, runtime: &Arc<Runtime>, generation: u64) -> bool {
        self.generation == generation
            && std::ptr::eq(self.host.as_ptr(), Arc::as_ptr(host))
            && std::ptr::eq(self.runtime.as_ptr(), Arc::as_ptr(runtime))
            && std::ptr::eq(self.shared.as_ptr(), Arc::as_ptr(&runtime.shared))
            && self.expected.options_match(host)
            && self.expected.runtime_matches(runtime)
    }

    /// 只核对真实调用内保存的弱身份；不升级引用、不查Host，不从字符串构造来源。
    pub(super) fn matches_call(&self, host: &Weak<Host>, shared: &Weak<Shared>, generation: u64) -> bool {
        self.generation == generation
            && Weak::ptr_eq(&self.host, host)
            && Weak::ptr_eq(&self.shared, shared)
    }

    /// 整个Host退出或即将发布新运行时，旧回执按真实Host对象单向撤销。
    /// Weak仍保留对象分配身份；不看包名/摘要，不升级引用，也不重入生命周期锁。
    pub(in crate::server::core::plugin_host) fn belongs_to_host(&self, host: &Host) -> bool {
        std::ptr::eq(self.host.as_ptr(), host)
    }

    pub(in crate::server::core::plugin_host) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.expected.provider.len() + self.expected.module_digest.len()
    }
}
