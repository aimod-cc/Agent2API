//! 实际ZIP字节的进程内见证；不从登记表、JSON、路径或调用方摘要构造。
//! 当前表只保留已提交代次，旧安装/冷启动没有见证时不能从sha256缓存补造。

mod archive;
mod compatible_zip;
mod registry;
mod startup;
mod zip_layout;

use std::path::Path;
use std::sync::Arc;

use agent2api_provider_protocol as proto;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use super::super::install::{self, InstalledPack, SignatureState};

pub(in crate::server::core::plugin_host) use archive::{extract, Extracted};
pub(in crate::server::core::plugin_host) use registry::{begin_change, begin_file_change, begin_scope_change, begin_registry_save, try_current, try_current_optional, FileChange, InstallLease, RegistrySave};
pub(in crate::server::core::plugin_host) use registry::{revoke_host_runtime, revoke_provider_runtimes};
pub(super) use registry::{begin_install, PendingInstall};
pub(in crate::server::core::plugin_host) use startup::{StartClaim, StartReceipt, StartSource};

const MAX_FILES: usize = 4096;
const MAX_TOTAL_BYTES: u64 = 128 * 1024 * 1024;

pub(in crate::server::core::plugin_host) fn validate_entry_path(entry: &str) -> Result<(), String> {
    zip_layout::path(entry, false).map(|_| ())
}

/// 所有摘要来自Extracted的同次受限流；不保存原ZIP或可变安装目录的代用字符串。
pub(in crate::server::core::plugin_host) struct VerifiedArtifact {
    pack: String,
    version: String,
    target: String,
    entry: String,
    providers: Vec<String>,
    signature: SignatureState,
    facts: Extracted,
    metadata_bytes: usize,
}

impl VerifiedArtifact {
    pub(super) fn from_prepared(
        facts: Extracted,
        manifest: &proto::PackManifest,
        selected_entry: &str,
        target: &str,
        signature: SignatureState,
        directory: &Path,
    ) -> Result<Option<Arc<Self>>, String> {
        zip_layout::path(selected_entry, false)?;
        let chosen = install::entry_for_target(manifest, target).ok_or(archive::INVALID)?;
        if chosen != selected_entry || !facts.files.contains_key(selected_entry)
            || signature == SignatureState::Invalid
        { return Err(archive::INVALID.into()); }
        let wasm = super::super::wasm::manifest::is_wasm(manifest);
        let target = if wasm { super::super::wasm::manifest::TARGET } else { target };
        // 保留原签名覆盖范围：原生UI文件、Wasm权限清单与可选host.json均加入同一载荷。
        if signature_from_facts(&facts, manifest, target)? != signature { return Err(archive::INVALID.into()); }
        if wasm {
            let entry = facts.files.get(selected_entry).ok_or(archive::INVALID)?;
            if entry.size > 16 * 1024 * 1024 || facts.wasm.is_none() { return Err(archive::INVALID.into()); }
        }
        if !facts.witness_supported { return Ok(None); }
        archive::recheck(&facts, directory)?;
        let metadata_bytes = facts.files.keys().map(|name| name.len() + 96).sum::<usize>()
            + facts.directories.iter().map(|name| name.len() + 64).sum::<usize>()
            + facts.pack.len() + facts.wasm.as_ref().map_or(0, Vec::len)
            + facts.host.as_ref().map_or(0, Vec::len)
            + manifest.id.len() + manifest.version.len() + selected_entry.len() + target.len() + 256
            + manifest.providers.iter().map(|provider| provider.id.as_str().len() + 24).sum::<usize>();
        Ok(Some(Arc::new(Self { pack: manifest.id.clone(), version: manifest.version.clone(), target: target.to_string(),
            entry: selected_entry.to_string(), providers: manifest.providers.iter().map(|provider| provider.id.0.clone()).collect(),
            signature, facts, metadata_bytes })))
    }

    pub(super) fn recheck(&self, directory: &Path) -> Result<(), String> {
        archive::recheck(&self.facts, directory)
    }

    fn matches(&self, record: &InstalledPack) -> bool {
        self.pack == record.id && self.version == record.version && self.entry == record.entry
            && hex(&self.facts.archive) == record.sha256 && self.signature.is_signed() == record.signed
            && self.providers == record.manifest_providers.iter().map(|provider| provider.id.0.clone()).collect::<Vec<_>>()
    }

    pub(in crate::server::core::plugin_host) fn wasm_provider(&self) -> Option<&str> {
        if self.target != super::super::wasm::manifest::TARGET || self.providers.len() != 1 || self.facts.wasm.is_none() {
            return None;
        }
        self.providers.first().map(String::as_str)
    }
    pub(in crate::server::core::plugin_host) fn archive_digest(&self) -> &[u8; 32] { &self.facts.archive }
    pub(in crate::server::core::plugin_host) fn pack(&self) -> &str { &self.pack }
    pub(in crate::server::core::plugin_host) fn version(&self) -> &str { &self.version }
    pub(in crate::server::core::plugin_host) fn target(&self) -> &str { &self.target }
    pub(in crate::server::core::plugin_host) fn entry(&self) -> &str { &self.entry }
    pub(in crate::server::core::plugin_host) fn signature(&self) -> SignatureState { self.signature }
    pub(in crate::server::core::plugin_host) fn module_digest(&self) -> Option<&[u8; 32]> {
        self.facts.files.get(&self.entry).map(|file| &file.digest)
    }
    pub(in crate::server::core::plugin_host) fn manifest_bytes(&self) -> &[u8] { &self.facts.pack }
    pub(in crate::server::core::plugin_host) fn wasm_bytes(&self) -> Option<&[u8]> { self.facts.wasm.as_deref() }
    pub(in crate::server::core::plugin_host) fn host_bytes(&self) -> Option<&[u8]> { self.facts.host.as_deref() }
}

fn hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut value = String::with_capacity(64);
    for byte in bytes { let _ = write!(value, "{byte:02x}"); }
    value
}

fn signature_from_facts(facts: &Extracted, manifest: &proto::PackManifest, target: &str) -> Result<SignatureState, String> {
    let Some(signature) = &manifest.signature else { return Ok(SignatureState::Unsigned); };
    if signature.target.as_deref() != Some(target) { return Ok(SignatureState::Unsigned); }
    if !signature.algorithm.eq_ignore_ascii_case("ed25519") { return Err(archive::INVALID.into()); }
    let (_, public) = install::OFFICIAL_PUBKEYS.iter().find(|(id, _)| *id == signature.key_id).ok_or(archive::INVALID)?;
    if public.len() != 64 { return Err(archive::INVALID.into()); }
    let mut key = [0u8; 32];
    for (index, pair) in public.as_bytes().chunks_exact(2).enumerate() {
        key[index] = u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| archive::INVALID)?, 16).map_err(|_| archive::INVALID)?;
    }
    let key = VerifyingKey::from_bytes(&key).map_err(|_| archive::INVALID)?;
    let entry = manifest.entry.get(target).ok_or(archive::INVALID)?;
    let binary = facts.files.get(entry).ok_or(archive::INVALID)?;
    let ui: Vec<_> = facts.files.iter().filter(|(name, _)| name.starts_with("ui/"))
        .map(|(name, file)| (name.clone(), hex(&file.digest))).collect();
    let mut payload = proto::signing_payload(manifest, target, &hex(&binary.digest), &ui);
    if super::super::wasm::manifest::is_wasm(manifest) {
        let wasm = facts.files.get("wasm.json").ok_or(archive::INVALID)?;
        payload.push_str(&format!("wasm={}\n", hex(&wasm.digest)));
    }
    if let Some(host) = facts.files.get("host.json") { payload.push_str(&format!("host={}\n", hex(&host.digest))); }
    if signature.value.len() > 256 { return Err(archive::INVALID.into()); }
    let signature = base64::engine::general_purpose::STANDARD.decode(signature.value.trim()).map_err(|_| archive::INVALID)?;
    let signature = Signature::from_slice(&signature).map_err(|_| archive::INVALID)?;
    key.verify(payload.as_bytes(), &signature).map_err(|_| archive::INVALID)?;
    Ok(SignatureState::Verified)
}
