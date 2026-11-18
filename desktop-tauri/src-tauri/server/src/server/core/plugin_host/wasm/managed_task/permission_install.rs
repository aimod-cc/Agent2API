//! 管理端只读取已安装包的有界文件，磁盘摘要不能代替实际运行模块摘要。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{atomic::Ordering, Arc};

use agent2api_provider_protocol as proto;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use super::super::{
    conversation::{CallGate, TaskCallRef},
    manifest,
    runtime::Runtime,
};
use super::{
    declaration::{self, Declaration},
    policy,
};
use crate::server::core::plugin_host::{
    self as host,
    install::{self, InstalledPack, PackRegistry},
    packages::provenance::{self, InstallLease},
};

const MAX_MODULE_BYTES: usize = 16 * 1024 * 1024;
const INVALID: &str = "插件清单无效或完整性校验未通过";

pub(super) struct Installed {
    pub(super) options: manifest::Options,
    pub(super) declaration: Declaration,
    pub(super) declaration_digest: String,
    module_digest: String,
    entry: PathBuf,
}

pub(super) fn find_pack<'a>(
    registry: &'a PackRegistry,
    id: &str,
) -> Result<&'a InstalledPack, String> {
    let mut matches = registry.packs.iter().filter(|pack| pack.id == id);
    let pack = matches.next().ok_or("插件未安装")?;
    if matches.next().is_some() {
        return Err("插件安装登记存在重复，不能确定权限归属".into());
    }
    Ok(pack)
}

fn version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+'))
        && crate::server::core::update::compare_versions(value, value).is_some()
}

fn entry(value: &str) -> bool {
    value.len() <= 512
        && value.ends_with(".wasm")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/-._".contains(&byte))
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn package_dir(pack: &InstalledPack) -> Result<PathBuf, String> {
    if !declaration::pack_id(&pack.id)
        || !version(&pack.version)
        || !entry(&pack.entry)
        || !declaration::digest_text(&pack.sha256)
    {
        return Err(INVALID.into());
    }
    let root = install::plugins_dir().canonicalize().map_err(|_| INVALID)?;
    let package = root.join(&pack.id).canonicalize().map_err(|_| INVALID)?;
    let dir = package
        .join(&pack.version)
        .canonicalize()
        .map_err(|_| INVALID)?;
    // 不能借包目录或版本目录的链接读取另一个包，也不能退回插件根目录。
    if package.parent() != Some(root.as_path())
        || package.file_name() != Some(std::ffi::OsStr::new(&pack.id))
        || dir.parent() != Some(package.as_path())
        || dir.file_name() != Some(std::ffi::OsStr::new(&pack.version))
        || !dir.is_dir()
    {
        return Err(INVALID.into());
    }
    Ok(dir)
}

fn read_file(dir: &Path, relative: &str, limit: usize) -> Result<(PathBuf, Vec<u8>), String> {
    let path = manifest::checked_entry(dir, relative).map_err(|_| INVALID)?;
    let file = std::fs::File::open(&path).map_err(|_| INVALID)?;
    let metadata = file.metadata().map_err(|_| INVALID)?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(INVALID.into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| INVALID)?;
    if bytes.len() > limit {
        return Err(INVALID.into());
    }
    // 打开前后的路径都必须仍是包内同一路径；普通文件内容另由真实字节摘要核对。
    if manifest::checked_entry(dir, relative).ok().as_ref() != Some(&path) {
        return Err(INVALID.into());
    }
    Ok((path, bytes))
}

/// 复用包签名载荷和内置公钥，但对同一批有界快照验签，避免再次无界读取磁盘。
fn signature_valid(
    pack: &InstalledPack,
    manifest: &proto::PackManifest,
    module_digest: &str,
    wasm_bytes: &[u8],
    host_bytes: Option<&[u8]>,
) -> bool {
    let Some(signature) = &manifest.signature else {
        return !pack.signed;
    };
    if signature.target.as_deref() != Some(manifest::TARGET) {
        return !pack.signed;
    }
    if !signature.algorithm.eq_ignore_ascii_case("ed25519") {
        return false;
    }
    let Some((_, key)) = install::OFFICIAL_PUBKEYS
        .iter()
        .find(|(id, _)| *id == signature.key_id)
    else {
        return false;
    };
    if key.len() != 64 {
        return false;
    }
    let mut public = [0u8; 32];
    for (index, pair) in key.as_bytes().chunks_exact(2).enumerate() {
        let Ok(text) = std::str::from_utf8(pair) else {
            return false;
        };
        let Ok(value) = u8::from_str_radix(text, 16) else {
            return false;
        };
        public[index] = value;
    }
    let Ok(public) = VerifyingKey::from_bytes(&public) else {
        return false;
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(signature.value.trim()) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(&bytes) else {
        return false;
    };
    let mut payload = proto::signing_payload(manifest, manifest::TARGET, module_digest, &[]);
    payload.push_str(&format!("wasm={:x}\n", Sha256::digest(wasm_bytes)));
    if let Some(bytes) = host_bytes {
        payload.push_str(&format!("host={:x}\n", Sha256::digest(bytes)));
    }
    public.verify(payload.as_bytes(), &signature).is_ok()
}

pub(super) fn inspect(pack: &InstalledPack) -> Result<Installed, String> {
    if pack.manifest_providers.len() != 1
        || !matches!(pack.manifest_providers[0].id.as_str(), policy::PROVIDER | policy::INTL_PROVIDER)
    {
        return Err(INVALID.into());
    }
    let dir = package_dir(pack)?;
    let (_, bytes) = read_file(&dir, "pack.json", 64 * 1024)?;
    let pack_manifest: proto::PackManifest =
        declaration::parse_object(&bytes, 64 * 1024).map_err(|_| INVALID)?;
    if pack_manifest.id != pack.id
        || pack_manifest.version != pack.version
        || pack_manifest.providers.len() != 1
        || pack.manifest_providers.len() != 1
        || pack_manifest.providers[0].id != pack.manifest_providers[0].id
        || !matches!(pack_manifest.providers[0].id.as_str(), policy::PROVIDER | policy::INTL_PROVIDER)
        || pack_manifest.entry.len() != 1
        || pack_manifest.entry.get(manifest::TARGET) != Some(&pack.entry)
    {
        return Err(INVALID.into());
    }
    install::validate_manifest(&pack_manifest, proto::PROTOCOL_VERSION).map_err(|_| INVALID)?;
    let (_, wasm_bytes) = read_file(&dir, "wasm.json", 32 * 1024)?;
    let options = manifest::read_bytes(&dir, &pack_manifest, &wasm_bytes).map_err(|_| INVALID)?;
    let declaration = options
        .manifest
        .managed_task
        .as_ref()
        .filter(|item| item.requested())
        .ok_or("插件未声明受控任务权限")?
        .clone();
    if !declaration
        .matches_call(&options.provider, policy::ENTRY)
        .map_err(|_| INVALID)?
    {
        return Err(INVALID.into());
    }
    let (entry, bytes) = read_file(&dir, &pack.entry, MAX_MODULE_BYTES)?;
    if !bytes.starts_with(b"\0asm\x01\0\0\0") {
        return Err(INVALID.into());
    }
    let module_digest = format!("{:x}", Sha256::digest(&bytes));
    let host_bytes = match std::fs::symlink_metadata(dir.join("host.json")) {
        Ok(_) => Some(read_file(&dir, "host.json", 16 * 1024)?.1),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(INVALID.into()),
    };
    if !signature_valid(
        pack,
        &pack_manifest,
        &module_digest,
        &wasm_bytes,
        host_bytes.as_deref(),
    ) {
        return Err(INVALID.into());
    }
    let declaration_digest = declaration.digest().map_err(|_| INVALID)?;
    Ok(Installed {
        options,
        declaration,
        declaration_digest,
        module_digest,
        entry,
    })
}

/// 管理确认时没有调用身份；此来源只锁定同一次已提交安装及其实际启动回执。
/// 无任意构造器/serde，不强持Host/Runtime/Shared，不由摘要字符串恢复。
pub(super) struct InstallationScope {
    lease: Arc<InstallLease>,
    source_digest: [u8; 32],
    declaration: Declaration,
    // digest同时覆盖固定provider/profile/entry、地址与预算，不只比较声明形状。
    rules_digest: String,
}

impl InstallationScope {
    pub(super) fn pack(&self) -> &str {
        self.lease.artifact().pack()
    }

    pub(super) fn same_install(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.lease, &other.lease)
            && self.source_digest == other.source_digest
            && self.declaration == other.declaration
            && self.rules_digest == other.rules_digest
    }

    pub(super) fn matches_call(&self, call: &TaskCallRef, gate: &CallGate<'_>) -> bool {
        gate.matches(call)
            && self.declaration.matches_call(gate.provider(), policy::ENTRY).unwrap_or(false)
            && gate.shared().options.source_digest() == &self.source_digest
            && gate.shared().options.manifest.managed_task.as_ref() == Some(&self.declaration)
            && gate.matches_install(call, &self.lease)
    }

    /// 审批门之后仅一次try；返回守卫用于继续原调用绑定，不查Host/配置/文件。
    pub(super) fn try_hold(&self) -> Result<impl Sized + '_, &'static str> {
        self.lease.try_hold()
    }

    pub(super) fn matches_binding(&self, binding: &super::install_binding::InstallBinding) -> bool {
        binding.matches_scope(self)
    }

    pub(super) fn is_lease(&self, lease: &Arc<InstallLease>) -> bool {
        Arc::ptr_eq(&self.lease, lease)
    }

    pub(super) fn lease_for_gate(
        &self,
        call: &TaskCallRef,
        gate: &CallGate<'_>,
    ) -> Option<Arc<InstallLease>> {
        self.matches_call(call, gate).then(|| self.lease.clone())
    }
}

pub(super) struct Loaded {
    host: Arc<host::Host>,
    runtime: Arc<Runtime>,
    generation: u64,
    lease: Arc<InstallLease>,
}

impl Loaded {
    pub(super) fn module_digest(&self) -> &str {
        self.runtime.module_digest()
    }

    pub(super) fn source_digest(&self) -> &[u8; 32] {
        self.runtime.shared.options.source_digest()
    }

    pub(super) fn installation_scope(&self) -> Option<InstallationScope> {
        let declaration = self.runtime.shared.options.manifest.managed_task.as_ref()?;
        if !declaration
            .matches_call(&self.runtime.shared.options.provider, policy::ENTRY)
            .ok()?
        {
            return None;
        }
        Some(InstallationScope {
            lease: self.lease.clone(),
            source_digest: *self.source_digest(),
            declaration: declaration.clone(),
            rules_digest: declaration.digest().ok()?,
        })
    }

    /// 仅复核同一Loaded的内存来源；不找替代Host、不读磁盘、配置或见证表。
    pub(super) fn with_current_memory<R>(&self, inspect: impl FnOnce() -> R) -> Option<R> {
        let _lifecycle = self.host.lifecycle.try_lock().ok()?;
        let runtime = self.host.embedded.runtime.try_read().ok()?;
        let current = runtime.as_ref()?;
        if !Arc::ptr_eq(current, &self.runtime)
            || !self.host.ready()
            || host::runtime::admission_closed()
            || self.host.generation.load(Ordering::Acquire) != self.generation
            || self.runtime.shared.cancel.is_cancelled()
            || !self
                .lease
                .started_runtime()?
                .matches(&self.host, &self.runtime, self.generation)
        {
            return None;
        }
        Some(inspect())
    }

    /// 仅从已经检查过的Loaded借取来源；不保留Host/Runtime，也不为另一调用找同包实例。
    /// 当前性由真实调用门和一次Lease try共同复核，不能在门内再调用still_current。
    pub(super) fn lease_for_call(&self, call: &TaskCallRef) -> Option<Arc<InstallLease>> {
        if !call.matches_install(&self.lease, &self.host, &self.runtime, self.generation) {
            return None;
        }
        let checked = call.with_gate(|gate| {
            if !gate.matches_install(call, &self.lease) {
                return Err(super::NotSentReason::InvalidBinding);
            }
            self.lease
                .try_hold()
                .map(drop)
                .map_err(|_| super::NotSentReason::InvalidBinding)
        });
        checked.ok()?;
        Some(self.lease.clone())
    }

    /// 只证明本次检查点一致，不保留见证门或授予后续执行资格。
    pub(super) fn still_current(
        &self,
        pack: &InstalledPack,
        installed: &Installed,
        owners: &BTreeMap<String, String>,
    ) -> bool {
        let provider = &installed.options.provider;
        if !pack.enabled
            || !self.lease.matches_record(pack)
            || !pack.providers.get(provider).copied().unwrap_or(true)
            || owners.get(provider).map(String::as_str) != Some(pack.id.as_str())
            || !host::serves_provider(provider)
            || host::registry::is_switching(provider)
            || !host::host_for_pack(&pack.id)
                .is_some_and(|current| Arc::ptr_eq(&current, &self.host))
            || !host::host_for_provider(provider)
                .is_some_and(|current| Arc::ptr_eq(&current, &self.host))
            || self.host.options.exe.canonicalize().ok().as_ref() != Some(&installed.entry)
            || self.host.options.wasm.as_ref().is_none_or(|options| {
                options.provider != *provider
                    || options.source_digest() != installed.options.source_digest()
                    || options.manifest.managed_task.as_ref() != Some(&installed.declaration)
            })
        {
            return false;
        }
        let Ok(_lifecycle) = self.host.lifecycle.lock() else {
            return false;
        };
        let runtime = match self.host.embedded.runtime.try_read() {
            Ok(runtime) => runtime,
            Err(_) => return false,
        };
        let Some(current) = runtime.as_ref() else {
            return false;
        };
        if !self.host.ready()
            || self.host.generation.load(Ordering::Acquire) != self.generation
            || self.runtime.shared.cancel.is_cancelled()
            || self.runtime.module_digest() != installed.module_digest
            || self.runtime.shared.options.provider != *provider
            || self.runtime.shared.options.source_digest() != installed.options.source_digest()
            || self.runtime.shared.options.manifest.managed_task.as_ref()
                != Some(&installed.declaration)
            || !Arc::ptr_eq(current, &self.runtime)
        {
            return false;
        }
        let Some(started) = self.lease.started_runtime() else {
            return false;
        };
        if !started.matches(&self.host, &self.runtime, self.generation) {
            return false;
        }
        // Host/Runtime检查在见证门之前；最后一次try仅检查已提交同对象，门内不反向取锁。
        self.lease.try_hold().is_ok()
    }
}

pub(super) fn loaded(
    pack: &InstalledPack,
    installed: &Installed,
    owners: &BTreeMap<String, String>,
) -> Option<Loaded> {
    // 未commit时无Lease；冷启动/普通低层安装缺新Host回执时不补造运行来源。
    let lease = provenance::try_current(&pack.id).ok()?;
    lease.started_runtime()?;
    let host = host::host_for_pack(&pack.id)?;
    let runtime = host.embedded.runtime.try_read().ok()?.clone()?;
    let generation = host.generation.load(Ordering::Acquire);
    let loaded = Loaded {
        host,
        runtime,
        generation,
        lease,
    };
    loaded
        .still_current(pack, installed, owners)
        .then_some(loaded)
}
