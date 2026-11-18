//! 原 TaskCallRef 绑定已提交安装或原加载实例；不授予额外网络或创建许可。
//! 最末核对只用原调用门和原来源，随后进入账号事务，不读取磁盘或配置。

use std::marker::PhantomData;
use std::sync::Arc;

use super::super::conversation::{CallGate, TaskCallRef};
use super::permission_install::{InstallationScope, Loaded};
use crate::server::core::plugin_host::packages::provenance::InstallLease;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingError {
    DifferentSource,
    Unavailable,
}

/// 字段封闭；只接收真实Loaded或安装来源及原调用，没有JSON/摘要/现成Host导入。
/// Clone仅复制两个Arc，不持Owner/CallLease，也不延长Host/Runtime/Shared寿命。
enum Source {
    Installed(Arc<InstallLease>),
    Conversation(Arc<RuntimeSource>),
}

struct RuntimeSource {
    source_digest: [u8; 32],
    module_digest: String,
}

impl Clone for Source {
    fn clone(&self) -> Self {
        match self {
            Self::Installed(lease) => Self::Installed(lease.clone()),
            Self::Conversation(source) => Self::Conversation(source.clone()),
        }
    }
}

#[derive(Clone)]
pub(super) struct InstallBinding {
    call: TaskCallRef,
    source: Source,
}

/// 真正Lease锁由try_hold的同步栈拥有；这里只可借用，不能带出回调或跨await。
/// 不暴露Lease、Host、调用或解锁接口，也不承担提交资格。
#[must_use]
pub(super) struct InstallGuard<'a> {
    binding: &'a &'a InstallBinding,
    invariant: PhantomData<&'a mut &'a ()>,
    not_send: PhantomData<std::rc::Rc<()>>,
}

impl InstallBinding {
    pub(super) fn from_call_gate(call: &TaskCallRef, gate: &CallGate<'_>) -> Option<Self> {
        if !gate.matches(call) {
            return None;
        }
        let source =
            match crate::server::core::plugin_host::packages::provenance::try_current_optional(
                call.package(),
            )
            .ok()?
            {
                Some(lease) if gate.matches_install(call, &lease) => Source::Installed(lease),
                Some(lease) if lease.started_runtime().is_some() => return None,
                _ => Source::Conversation(Arc::new(RuntimeSource {
                    source_digest: *gate.source_digest(),
                    module_digest: gate.module_digest().to_string(),
                })),
            };
        Some(Self {
            call: call.clone(),
            source,
        })
    }

    pub(super) fn from_loaded(loaded: &Loaded, call: &TaskCallRef) -> Option<Self> {
        let lease = loaded.lease_for_call(call)?;
        Some(Self {
            call: call.clone(),
            source: Source::Installed(lease),
        })
    }

    pub(super) fn from_scope(
        scope: &InstallationScope,
        call: &TaskCallRef,
        gate: &CallGate<'_>,
    ) -> Option<Self> {
        let lease = scope.lease_for_gate(call, gate)?;
        Some(Self {
            call: call.clone(),
            source: Source::Installed(lease),
        })
    }

    pub(super) fn matches_scope(&self, scope: &InstallationScope) -> bool {
        matches!(&self.source, Source::Installed(lease) if scope.is_lease(lease))
    }

    pub(super) fn belongs_to(&self, call: &TaskCallRef) -> bool {
        self.call.same_source(call)
    }

    /// 必须在原call.with_gate内、持必要外层锁后调用；同源比较全部先于唯一一次Lease try。
    /// 最末检查只同步借用guard；不得回查注册表、Host、文件、配置或调用门；账号终检可在其后。
    /// R不借用短期guard，返回Future也不能携带guard；本函数不执行/等待Future。
    pub(super) fn try_hold<R>(
        &self,
        gate: &CallGate<'_>,
        inspect: impl for<'a> FnOnce(&InstallGuard<'a>) -> R,
    ) -> Result<R, BindingError> {
        if !gate.matches(&self.call) {
            return Err(BindingError::DifferentSource);
        }
        let lease_guard = match &self.source {
            Source::Installed(lease) => {
                if !gate.matches_install(&self.call, lease) {
                    return Err(BindingError::DifferentSource);
                }
                Some(lease.try_hold().map_err(|_| BindingError::Unavailable)?)
            }
            Source::Conversation(source) => {
                if gate.source_digest() != &source.source_digest
                    || gate.module_digest() != source.module_digest
                {
                    return Err(BindingError::DifferentSource);
                }
                None
            }
        };
        let binding = self;
        let guard = InstallGuard {
            binding: &binding,
            invariant: PhantomData,
            not_send: PhantomData,
        };
        let result = inspect(&guard);
        drop(lease_guard);
        Ok(result)
    }
}

impl InstallGuard<'_> {
    /// 只认同一安装对象和原调用；Clone可以同源，重新安装的相同内容不能同源。
    pub(super) fn matches(&self, binding: &InstallBinding) -> bool {
        self.binding.call.same_source(&binding.call)
            && match (&self.binding.source, &binding.source) {
                (Source::Installed(left), Source::Installed(right)) => Arc::ptr_eq(left, right),
                (Source::Conversation(left), Source::Conversation(right)) => {
                    Arc::ptr_eq(left, right)
                }
                _ => false,
            }
    }
}
