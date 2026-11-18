//! 本次安装独有的一次性启动来源。只有新Host工厂的回执可随同一事务提交。
//! 没有JSON、摘要或现成Host导入入口；票据/回执本身不代表已提交安装或执行许可。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::VerifiedArtifact;
use crate::server::core::plugin_host::{
    install::InstalledPack, wasm::install_source::{self, NewHost, StartedRuntime},
    GatewayCallbacks, Host, HostOptions,
};

struct TransactionIdentity;

/// 仅由PendingInstall持有；签发票据后不会再产生第二份，即使第一次启动未完成。
pub(super) struct PendingStart {
    identity: Arc<TransactionIdentity>,
    source_taken: bool,
    rejected: bool,
    receipt: Option<StartReceipt>,
}

pub(in crate::server::core::plugin_host) struct StartSource {
    identity: Arc<TransactionIdentity>,
    artifact: Arc<VerifiedArtifact>,
    entry: PathBuf,
}

pub(in crate::server::core::plugin_host) struct StartClaim {
    identity: Arc<TransactionIdentity>,
    host: NewHost,
}

pub(in crate::server::core::plugin_host) struct StartReceipt {
    identity: Arc<TransactionIdentity>,
    runtime: StartedRuntime,
}

impl PendingStart {
    pub(super) fn new() -> Self {
        Self { identity: Arc::new(TransactionIdentity), source_taken: false, rejected: false, receipt: None }
    }

    pub(super) fn take_source(&mut self, artifact: &Arc<VerifiedArtifact>, directory: &Path) -> Option<StartSource> {
        if self.source_taken || self.rejected { return None; }
        self.source_taken = true;
        artifact.wasm_provider()?;
        Some(StartSource { identity: self.identity.clone(), artifact: artifact.clone(), entry: directory.join(artifact.entry()) })
    }

    pub(super) fn accept(&mut self, receipt: StartReceipt) -> Result<(), &'static str> {
        if self.rejected || !self.source_taken || self.receipt.is_some()
            || !Arc::ptr_eq(&self.identity, &receipt.identity)
        {
            self.rejected = true;
            self.receipt = None;
            return Err("安装启动回执不属于本次事务，未建立可信运行来源");
        }
        self.receipt = Some(receipt);
        Ok(())
    }

    pub(super) fn still_current(&self) -> bool {
        self.receipt.as_ref().is_none_or(|receipt| receipt.runtime.still_current())
    }

    pub(super) fn clear_receipt(&mut self) {
        self.rejected = true;
        self.receipt = None;
    }

    pub(super) fn into_committed(self) -> CommittedStart {
        CommittedStart { identity: self.identity, receipt: self.receipt }
    }

    pub(super) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.receipt.as_ref().map_or(0, |receipt| receipt.runtime.retained_bytes())
    }
}

impl StartSource {
    pub(in crate::server::core::plugin_host) fn pack(&self) -> &str { self.artifact.pack() }

    pub(in crate::server::core::plugin_host) fn matches_pack(&self, pack: &InstalledPack) -> bool {
        self.artifact.matches(pack)
    }

    pub(in crate::server::core::plugin_host) fn new_host(
        self,
        options: HostOptions,
        callbacks: Arc<dyn GatewayCallbacks>,
    ) -> (Arc<Host>, Option<StartClaim>) {
        // 工厂永远新建对象，不把本次安装身份贴到外部传入的旧Host。
        let (host, claim) = install_source::new_host(options, callbacks, &self.artifact, &self.entry);
        (host, claim.map(|host| StartClaim { identity: self.identity, host }))
    }
}

impl StartClaim {
    pub(in crate::server::core::plugin_host) fn observe_loaded(self) -> Option<StartReceipt> {
        let runtime = self.host.observe_loaded()?;
        Some(StartReceipt { identity: self.identity, runtime })
    }
}

/// 只能被同事务commit移入InstallLease；缺回执的低层安装不能匹配运行实例。
pub(super) struct CommittedStart {
    identity: Arc<TransactionIdentity>,
    receipt: Option<StartReceipt>,
}

impl CommittedStart {
    pub(super) fn runtime(&self) -> Option<&StartedRuntime> {
        self.receipt.as_ref().filter(|receipt| Arc::ptr_eq(&self.identity, &receipt.identity)).map(|receipt| &receipt.runtime)
    }
}
