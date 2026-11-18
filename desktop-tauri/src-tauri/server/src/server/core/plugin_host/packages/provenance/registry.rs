//! 见证表只保留当前对象和正在写登记表的有界票据；所有文件/Host动作都在门外。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, TryLockError};

use super::{startup::{CommittedStart, PendingStart}, StartReceipt, StartSource, VerifiedArtifact};
use crate::server::core::plugin_host::{install::{InstalledPack, PackRegistry}, Host};

const MAX_CURRENT: usize = 256;
const MAX_METADATA: usize = 16 * 1024 * 1024;
const UNAVAILABLE: &str = "无可信安装见证";

#[derive(PartialEq, Eq)]
struct RecordIdentity {
    ordinal: usize,
    version: String,
    enabled: bool,
    providers: BTreeMap<String, bool>,
    entry: String,
    sha256: String,
    signed: bool,
    installed_at: i64,
    manifest: Vec<(String, String, i32, bool)>,
}

impl RecordIdentity {
    fn storage_bytes(&self) -> usize {
        self.version.len() + self.entry.len() + self.sha256.len() + 128
            + self.providers.keys().map(|id| id.len() + 64).sum::<usize>()
            + self.manifest.iter().map(|(id, label, _, _)| id.len() + label.len() + 64).sum::<usize>()
    }

    fn from_record(record: &InstalledPack, ordinal: usize) -> Option<Self> {
        if record.id.is_empty() || record.id.len() > 128 || record.version.len() > 128
            || record.entry.len() > 512 || record.sha256.len() > 64
            || record.providers.len() > MAX_CURRENT || record.manifest_providers.len() > MAX_CURRENT
            || record.providers.keys().any(|id| id.len() > 128)
            || record.manifest_providers.iter().any(|provider| provider.id.as_str().len() > 128 || provider.label.len() > 512)
        { return None; }
        Some(Self { ordinal, version: record.version.clone(), enabled: record.enabled, providers: record.providers.clone(),
            entry: record.entry.clone(), sha256: record.sha256.clone(), signed: record.signed, installed_at: record.installed_at,
            manifest: record.manifest_providers.iter().map(|provider|
                (provider.id.0.clone(), provider.label.clone(), provider.order, provider.default_enabled)).collect() })
    }
}

struct State {
    revision: u64,
    failed: bool,
    entries: BTreeMap<String, Arc<InstallLease>>,
    writes: BTreeMap<u64, usize>,
    changes: BTreeMap<u64, String>,
    registry: Option<Arc<BTreeMap<String, RecordIdentity>>>,
    metadata: usize,
}

static TABLE: Mutex<State> = Mutex::new(State { revision: 0, failed: false,
    entries: BTreeMap::new(), writes: BTreeMap::new(), changes: BTreeMap::new(), registry: None, metadata: 0 });

/// 安装对象绑定已提交登记与可选的新Host启动回执；没有回执不能匹配任何运行实例。
/// 对象身份而非字符串是代次；只在TABLE锁内翻转revoked，不可复活。
pub(in crate::server::core::plugin_host) struct InstallLease {
    artifact: Arc<VerifiedArtifact>,
    record: RecordIdentity,
    revoked: std::sync::atomic::AtomicBool,
    metadata_bytes: usize,
    start: CommittedStart,
}

impl InstallLease {
    pub(in crate::server::core::plugin_host) fn artifact(&self) -> &VerifiedArtifact { &self.artifact }

    pub(in crate::server::core::plugin_host) fn started_runtime(
        &self,
    ) -> Option<&crate::server::core::plugin_host::wasm::install_source::StartedRuntime> {
        self.start.runtime()
    }

    pub(in crate::server::core::plugin_host) fn matches_record(&self, record: &InstalledPack) -> bool {
        RecordIdentity::from_record(record, self.record.ordinal).as_ref() == Some(&self.record)
            && self.artifact.matches(record)
    }

    pub(in crate::server::core::plugin_host) fn try_hold(self: &Arc<Self>) -> Result<LeaseGuard, &'static str> {
        let guard = match TABLE.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Err("安装见证检查忙"),
            Err(TryLockError::Poisoned(_)) => return Err(UNAVAILABLE),
        };
        if guard.failed || !guard.writes.is_empty() || !guard.changes.is_empty() || self.revoked.load(std::sync::atomic::Ordering::Acquire)
            || !guard.entries.get(&self.artifact.pack).is_some_and(|current| Arc::ptr_eq(current, self))
        { return Err(UNAVAILABLE); }
        Ok(LeaseGuard { _guard: guard })
    }
}

#[must_use]
pub(in crate::server::core::plugin_host) struct LeaseGuard {
    _guard: std::sync::MutexGuard<'static, State>,
}

pub(in crate::server::core::plugin_host) fn try_current(pack: &str) -> Result<Arc<InstallLease>, &'static str> {
    try_current_optional(pack)?.ok_or(UNAVAILABLE)
}

pub(in crate::server::core::plugin_host) fn try_current_optional(pack: &str) -> Result<Option<Arc<InstallLease>>, &'static str> {
    if pack.len() > 128 { return Err(UNAVAILABLE); }
    let guard = match TABLE.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => return Err("安装见证检查忙"),
        Err(TryLockError::Poisoned(_)) => return Err(UNAVAILABLE),
    };
    if guard.failed || !guard.writes.is_empty() || !guard.changes.is_empty() { return Err(UNAVAILABLE); }
    Ok(guard.entries.get(pack).filter(|entry| !entry.revoked.load(std::sync::atomic::Ordering::Acquire)).cloned())
}

fn revoke(state: &mut State, id: &str) {
    if let Some(entry) = state.entries.remove(id) {
        entry.revoked.store(true, std::sync::atomic::Ordering::Release);
        state.metadata = state.metadata.saturating_sub(entry.metadata_bytes);
    }
}

fn advance(state: &mut State) -> Option<u64> {
    if state.failed { return None; }
    match state.revision.checked_add(1) {
        Some(value) => { state.revision = value; Some(value) }
        None => { fail(state); None }
    }
}

fn fail(state: &mut State) {
    state.failed = true;
    for entry in state.entries.values() { entry.revoked.store(true, std::sync::atomic::Ordering::Release); }
    state.entries.clear();
    state.writes.clear();
    state.changes.clear();
    state.registry = None;
    state.metadata = 0;
}

/// 只撤销表中已经提交的来源；空命中不制造revision事件，尤其不能打断首次安装的PendingInstall。
/// selector仅由下方两个封闭内存比较入口提供；门内不取Host/Runtime/脚本锁，不保留可恢复列表。
fn revoke_published(mut matches: impl FnMut(&InstallLease) -> bool) {
    let Ok(mut state) = TABLE.lock() else { return; };
    if state.failed { return; }
    let mut removed_bytes = 0usize;
    let mut removed = false;
    state.entries.retain(|_, lease| {
        if !matches(lease) { return true; }
        lease.revoked.store(true, std::sync::atomic::Ordering::Release);
        removed_bytes = removed_bytes.saturating_add(lease.metadata_bytes);
        removed = true;
        false
    });
    if removed {
        state.metadata = state.metadata.saturating_sub(removed_bytes);
        let _ = advance(&mut state);
    }
}

/// 发布前/退出时只命中这个实际Host的旧回执；新Host首次初始化还没有已提交回执，完全无操作。
/// Host/Runtime锁可在外层持有，表内只做封闭对象身份比较；不调用still_current。
pub(in crate::server::core::plugin_host) fn revoke_host_runtime(host: &Host) {
    revoke_published(|lease| lease.started_runtime().is_some_and(|started| started.belongs_to_host(host)));
}

/// 脚本应用真实归属变更之前调用；只关闭已提交且有运行回执的受影响provider，不签发任何来源。
/// provider集合由宿主准备好的归属快照给出；本函数不读取脚本登记、配置、文件或Db。
pub(in crate::server::core::plugin_host) fn revoke_provider_runtimes(providers: &std::collections::HashSet<String>) {
    if providers.is_empty() { return; }
    revoke_published(|lease| lease.started_runtime().is_some()
        && lease.artifact.wasm_provider().is_some_and(|provider| providers.contains(provider)));
}

/// 在实际变更动作前逐目标关闭旧对象；不查文件、不取得宿主锁，也不发停止操作。
pub(in crate::server::core::plugin_host) fn begin_change(id: &str) {
    let Ok(mut state) = TABLE.lock() else { return; };
    if advance(&mut state).is_none() { return; }
    revoke(&mut state, id);
}

#[must_use]
pub(in crate::server::core::plugin_host) struct FileChange {
    ticket: u64,
}

pub(in crate::server::core::plugin_host) fn begin_scope_change(ids: &[String], target: &str) -> FileChange {
    let mut change = FileChange { ticket: 0 };
    let Ok(mut state) = TABLE.lock() else { return change; };
    if ids.len() > MAX_CURRENT || ids.iter().any(|id| id.len() > 128) || target.len() > 128
        || state.changes.len() >= MAX_CURRENT || !ids.iter().any(|id| id == target)
    { fail(&mut state); return change; }
    for id in ids { revoke(&mut state, id); }
    let Some(ticket) = advance(&mut state) else { return change; };
    state.changes.insert(ticket, target.to_string());
    change.ticket = ticket;
    change
}

pub(in crate::server::core::plugin_host) fn begin_file_change(id: &str) -> FileChange {
    let mut change = FileChange { ticket: 0 };
    let Ok(mut state) = TABLE.lock() else { return change; };
    if id.len() > 128 || state.changes.len() >= MAX_CURRENT { fail(&mut state); return change; }
    revoke(&mut state, id);
    let Some(ticket) = advance(&mut state) else { return change; };
    state.changes.insert(ticket, id.to_string());
    change.ticket = ticket;
    change
}

impl Drop for FileChange {
    fn drop(&mut self) {
        let Ok(mut state) = TABLE.lock() else { return; };
        if self.ticket != 0 && state.changes.remove(&self.ticket).is_some() { let _ = advance(&mut state); }
    }
}

pub(in crate::server::core::plugin_host::packages) struct PendingInstall {
    artifact: Arc<VerifiedArtifact>,
    record: RecordIdentity,
    revision: u64,
    save_ticket: Option<u64>,
    change_ticket: u64,
    start: PendingStart,
}

/// 只能使用已从ZIP创建的见证开始安装；返回None不影响普通安装，但不能发布可信代次。
pub(in crate::server::core::plugin_host::packages) fn begin_install(
    artifact: Arc<VerifiedArtifact>, record: &InstalledPack, ordinal: usize, change: &FileChange,
) -> Option<PendingInstall> {
    let identity = RecordIdentity::from_record(record, ordinal)?;
    if !artifact.matches(record) || !record.enabled { return None; }
    let state = TABLE.lock().ok()?;
    if state.failed || !state.writes.is_empty() || state.changes.len() != 1
        || state.changes.get(&change.ticket).map(String::as_str) != Some(record.id.as_str())
    { return None; }
    Some(PendingInstall { artifact, record: identity, revision: state.revision, save_ticket: None,
        change_ticket: change.ticket, start: PendingStart::new() })
}

impl PendingInstall {
    pub(in crate::server::core::plugin_host::packages) fn take_start_source(&mut self, directory: &std::path::Path) -> Option<StartSource> {
        if self.save_ticket.is_none() { return None; }
        self.start.take_source(&self.artifact, directory)
    }

    pub(in crate::server::core::plugin_host::packages) fn accept_start(&mut self, receipt: StartReceipt) -> Result<(), &'static str> {
        self.start.accept(receipt)
    }

    /// 与唯一那次登记写入票据绑定；不能靠“当前SHA相同”认领别人的写入。
    pub(in crate::server::core::plugin_host::packages) fn saved_by(&mut self, saved: &RegistrySave) {
        if saved.before == self.revision && saved.succeeded && saved.pack.as_deref() == Some(self.artifact.pack.as_str())
            && saved.identities.as_ref().and_then(|records| records.get(&self.artifact.pack)) == Some(&self.record)
        {
            self.revision = saved.after;
            self.save_ticket = Some(saved.ticket);
        }
    }

    /// 在安装事务正式commit时调用；目录再验在门外，任何晚到登记写都会阻断发布。
    pub(in crate::server::core::plugin_host::packages) fn commit(self, directory: &std::path::Path, change: &mut FileChange) {
        if self.save_ticket.is_none() || self.change_ticket != change.ticket || self.artifact.recheck(directory).is_err() { return; }
        // 运行回执的复核在见证门外；若随后换代，使用端仍须重新匹配同一实例，不能缓存为许可。
        let mut start = self.start;
        if !start.still_current() { start.clear_receipt(); }
        let metadata_bytes = self.artifact.metadata_bytes + self.record.storage_bytes() + start.retained_bytes();
        let Ok(mut state) = TABLE.lock() else { return; };
        if state.failed || state.revision != self.revision || !state.writes.is_empty() || state.changes.len() != 1
            || state.changes.get(&change.ticket).map(String::as_str) != Some(self.artifact.pack.as_str())
            || state.registry.as_ref().and_then(|records| records.get(&self.artifact.pack)) != Some(&self.record)
        { return; }
        if state.entries.len() >= MAX_CURRENT || state.entries.contains_key(&self.artifact.pack)
            || metadata_bytes > MAX_METADATA.saturating_sub(state.metadata)
        { return; }
        if advance(&mut state).is_none() { return; }
        state.changes.remove(&change.ticket);
        change.ticket = 0;
        let pack = self.artifact.pack.clone();
        state.metadata += metadata_bytes;
        state.entries.insert(pack, Arc::new(InstallLease { artifact: self.artifact, record: self.record,
            revoked: std::sync::atomic::AtomicBool::new(false), metadata_bytes, start: start.into_committed() }));
    }
}

/// 写登记表前挂起借取，按将写入的有界身份撤销已变化的记录；无变化不会签发新对象。
pub(in crate::server::core::plugin_host) fn begin_registry_save(registry: &PackRegistry, install: Option<&str>) -> RegistrySave {
    let identities: Option<BTreeMap<_, _>> = if registry.packs.len() > MAX_CURRENT { None } else {
        let mut identities = BTreeMap::new();
        let mut valid = true;
        let mut bytes = 0usize;
        for (ordinal, pack) in registry.packs.iter().enumerate() {
            let Some(identity) = RecordIdentity::from_record(pack, ordinal) else { valid = false; break; };
            bytes = bytes.saturating_add(pack.id.len() + identity.storage_bytes());
            if bytes > MAX_METADATA || identities.insert(pack.id.clone(), identity).is_some() { valid = false; break; }
        }
        valid.then_some(identities)
    };
    let mut save = RegistrySave { ticket: 0, before: 0, after: 0,
        pack: install.map(str::to_string), identities: None, succeeded: false, finished: false };
    let Ok(mut state) = TABLE.lock() else { return save; };
    let Some(identities) = identities else { fail(&mut state); return save; };
    let mut affected = std::collections::BTreeSet::new();
    if let Some(previous) = &state.registry {
        for (id, record) in previous.iter().chain(identities.iter()) {
            if previous.get(id) != identities.get(id) {
                affected.extend(record.manifest.iter().map(|(provider, _, _, _)| provider.clone()));
                affected.extend(record.providers.keys().cloned());
            }
        }
    }
    let changed: Vec<_> = state.entries.iter().filter(|(id, entry)| {
        state.registry.is_none() || identities.get(*id) != Some(&entry.record)
            || entry.record.manifest.iter().any(|(provider, _, _, _)| affected.contains(provider.as_str()))
    }).map(|(id, _)| id.clone()).collect();
    // 只关闭身份变化或可能改变同provider归属的旧见证，不停止任何提供商。
    for id in changed { revoke(&mut state, &id); }
    let bytes = identities.iter().map(|(id, identity)| id.len() + identity.storage_bytes()).sum::<usize>();
    if state.writes.len() >= MAX_CURRENT
        || bytes > MAX_METADATA.saturating_sub(state.writes.values().sum::<usize>())
    { fail(&mut state); return save; }
    save.before = state.revision;
    let Some(ticket) = advance(&mut state) else { return save; };
    save.ticket = ticket;
    save.identities = Some(Arc::new(identities));
    state.writes.insert(ticket, bytes);
    save
}

pub(in crate::server::core::plugin_host) struct RegistrySave {
    ticket: u64,
    before: u64,
    after: u64,
    pack: Option<String>,
    identities: Option<Arc<BTreeMap<String, RecordIdentity>>>,
    succeeded: bool,
    finished: bool,
}

impl RegistrySave {
    pub(in crate::server::core::plugin_host) fn matches(&self, registry: &PackRegistry) -> bool {
        let Some(identities) = &self.identities else { return false; };
        if self.finished || registry.packs.len() != identities.len() { return false; }
        let mut seen = std::collections::BTreeSet::new();
        registry.packs.iter().enumerate().all(|(ordinal, record)| seen.insert(record.id.as_str())
            && RecordIdentity::from_record(record, ordinal).as_ref() == identities.get(&record.id))
    }

    pub(in crate::server::core::plugin_host) fn finish(&mut self, succeeded: bool) {
        if self.finished { return; }
        self.finished = true;
        let Ok(mut state) = TABLE.lock() else { return; };
        if self.ticket == 0 || state.writes.remove(&self.ticket).is_none() { return; }
        let exclusive = state.revision == self.ticket;
        self.after = advance(&mut state).unwrap_or(0);
        self.succeeded = succeeded && exclusive && !state.failed && state.writes.is_empty();
        if self.succeeded { state.registry = self.identities.clone(); }
        // 并行写无法证明最终登记内容；失败写已经关闭受影响对象，不无故撤销无关包。
        if !exclusive {
            state.registry = None;
            let ids: Vec<_> = state.entries.keys().cloned().collect();
            for id in ids { revoke(&mut state, &id); }
        }
    }
}

impl Drop for RegistrySave {
    fn drop(&mut self) {
        if !self.finished { self.finish(false); }
    }
}
