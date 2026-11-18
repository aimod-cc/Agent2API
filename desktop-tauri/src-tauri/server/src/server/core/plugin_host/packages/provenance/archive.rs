//! 解包和见证使用同一次受限读取；没有按名称另读ZIP或先覆盖再排重的分支。

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use agent2api_provider_protocol as proto;
use sha2::{Digest, Sha256};

use super::{zip_layout, MAX_FILES, MAX_TOTAL_BYTES};

pub(super) const INVALID: &str = "插件包文件不完整、超限或存在路径歧义";

pub(super) struct FileFact {
    pub size: u64,
    pub digest: [u8; 32],
}

pub(in crate::server::core::plugin_host) struct Extracted {
    pub(super) archive: [u8; 32],
    pub(super) files: BTreeMap<String, FileFact>,
    pub(super) directories: BTreeSet<String>,
    pub(super) pack: Vec<u8>,
    pub(super) wasm: Option<Vec<u8>>,
    pub(super) host: Option<Vec<u8>>,
    pub(super) witness_supported: bool,
}

impl Extracted {
    pub(in crate::server::core::plugin_host) fn manifest(&self) -> Result<proto::PackManifest, String> {
        crate::server::core::plugin_host::wasm::managed_task::declaration::parse_object(&self.pack, 64 * 1024)
            .map_err(|_| "插件pack.json格式无效或存在重复字段".into())
    }

    pub(in crate::server::core::plugin_host::packages) fn validate(
        &self, directory: &Path, manifest: &proto::PackManifest,
    ) -> Result<bool, String> {
        use crate::server::core::plugin_host::{compatibility, wasm};
        if let Some(bytes) = &self.host {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Requirements { protocol: u32, #[serde(default)] requires: Vec<String> }
            let host: Requirements = wasm::managed_task::declaration::parse_object(bytes, 16 * 1024)
                .map_err(|_| "插件公共接口要求格式无效")?;
            if host.protocol != proto::PROTOCOL_VERSION
                || host.requires.iter().any(|item| !compatibility::CAPABILITIES.contains(&item.as_str()))
            { return Err("插件公共接口要求不受支持".into()); }
        }
        let is_wasm = wasm::manifest::is_wasm(manifest);
        if is_wasm {
            let bytes = self.wasm.as_ref().ok_or("Wasm插件缺少wasm.json")?;
            wasm::manifest::read_bytes(directory, manifest, bytes)?;
        } else if self.wasm.is_some() { return Err("非Wasm插件不能声明Wasm权限".into()); }
        Ok(is_wasm)
    }

    pub(in crate::server::core::plugin_host::packages) fn signature(
        &self, manifest: &proto::PackManifest, target: &str,
    ) -> Result<crate::server::core::plugin_host::install::SignatureState, String> {
        let target = if crate::server::core::plugin_host::wasm::manifest::is_wasm(manifest) {
            crate::server::core::plugin_host::wasm::manifest::TARGET
        } else { target };
        super::signature_from_facts(self, manifest, target)
    }
}

fn metadata_limit(name: &str) -> Option<usize> {
    match name {
        "pack.json" => Some(64 * 1024),
        "wasm.json" => Some(32 * 1024),
        "host.json" => Some(16 * 1024),
        _ => None,
    }
}

fn ensure_directory(
    root: &Path, relative: &str, strict: bool, names: &mut BTreeMap<PathBuf, String>,
) -> Result<(), String> {
    let mut current = root.to_path_buf();
    let mut prefix = String::new();
    for part in relative.split('/').filter(|part| !part.is_empty()) {
        if !prefix.is_empty() { prefix.push('/'); }
        prefix.push_str(part);
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(INVALID.into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current).map_err(|_| INVALID)?;
            }
            Err(_) => return Err(INVALID.into()),
        }
        let actual = current.canonicalize().map_err(|_| INVALID)?;
        if !actual.starts_with(root) || strict && actual != current
            || names.get(&actual).is_some_and(|name| name != &prefix)
        { return Err(INVALID.into()); }
        names.insert(actual, prefix.clone());
    }
    Ok(())
}

pub(in crate::server::core::plugin_host) fn extract(bytes: &[u8], target: &Path) -> Result<Extracted, String> {
    if bytes.is_empty() || bytes.len() > super::super::MAX_PACK_BYTES { return Err(INVALID.into()); }
    let entries = super::compatible_zip::inspect(bytes)?;
    let witness_supported = zip_layout::inspect(bytes).is_ok();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| INVALID)?;
    if archive.len() != entries.len() { return Err(INVALID.into()); }
    let metadata = std::fs::symlink_metadata(target).map_err(|_| INVALID)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() { return Err(INVALID.into()); }
    let root = target.canonicalize().map_err(|_| INVALID)?;
    let mut facts = Extracted { archive: Sha256::digest(bytes).into(), files: BTreeMap::new(),
        directories: BTreeSet::new(), pack: Vec::new(), wasm: None, host: None, witness_supported };
    let mut total = 0u64;
    let mut directory_names = BTreeMap::new();
    let mut buffer = [0u8; 64 * 1024];
    for (index, entry) in entries.iter().enumerate() {
        let mut file = archive.by_index(index).map_err(|_| INVALID)?;
        if file.name() != entry.name
            || file.size() != entry.size || file.compressed_size() != entry.compressed
            || file.crc32() != entry.crc || file.header_start() != entry.header
            || file.central_header_start() != entry.central || file.is_dir() != entry.directory
            || file.encrypted() || file.is_symlink()
        { return Err(INVALID.into()); }
        let name = zip_layout::path(&entry.name, entry.directory)?;
        let limit = metadata_limit(name);
        if limit.is_some_and(|limit| entry.size > limit as u64) { return Err(INVALID.into()); }
        let parent = if entry.directory { name } else { name.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("") };
        ensure_directory(&root, parent, facts.witness_supported, &mut directory_names)?;
        let mut prefix = String::new();
        for part in parent.split('/').filter(|part| !part.is_empty()) {
            if !prefix.is_empty() { prefix.push('/'); }
            prefix.push_str(part);
            facts.directories.insert(prefix.clone());
        }
        let output = root.join(name);
        let mut out = if entry.directory { None } else {
            Some(std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&output).map_err(|_| INVALID)?)
        };
        let actual_output = output.canonicalize().map_err(|_| INVALID)?;
        if actual_output != output {
            if facts.witness_supported || !actual_output.starts_with(&root) { return Err(INVALID.into()); }
        }
        let mut digest = Sha256::new();
        let mut kept = Vec::new();
        let mut actual = 0u64;
        loop {
            let remaining = entry.size.saturating_sub(actual).min(MAX_TOTAL_BYTES.saturating_sub(total));
            let length = buffer.len().min(remaining.saturating_add(1) as usize);
            let count = file.read(&mut buffer[..length]).map_err(|_| INVALID)?;
            if count == 0 { break; }
            if count as u64 > remaining { return Err(INVALID.into()); }
            actual += count as u64;
            total += count as u64;
            digest.update(&buffer[..count]);
            if limit.is_some() { kept.extend_from_slice(&buffer[..count]); }
            if let Some(out) = &mut out { out.write_all(&buffer[..count]).map_err(|_| INVALID)?; }
        }
        if actual != entry.size || total > MAX_TOTAL_BYTES { return Err(INVALID.into()); }
        if entry.directory { continue; }
        if let Some(out) = &mut out {
            out.flush().map_err(|_| INVALID)?;
            if facts.witness_supported && !single_link(&out.metadata().map_err(|_| INVALID)?) { return Err(INVALID.into()); }
        }
        drop(out);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&output, std::fs::Permissions::from_mode(file.unix_mode().unwrap_or(0o644) & 0o777))
                .map_err(|_| INVALID)?;
        }
        facts.files.insert(name.to_string(), FileFact { size: actual, digest: digest.finalize().into() });
        match name {
            "pack.json" => facts.pack = kept,
            "wasm.json" => facts.wasm = Some(kept),
            "host.json" => facts.host = Some(kept),
            _ => {}
        }
    }
    if facts.pack.is_empty() || facts.files.len() > MAX_FILES { return Err(INVALID.into()); }
    Ok(facts)
}

fn single_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

pub(super) fn recheck(facts: &Extracted, directory: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(directory).map_err(|_| INVALID)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() { return Err(INVALID.into()); }
    let root = directory.canonicalize().map_err(|_| INVALID)?;
    let mut pending = vec![(root.clone(), String::new())];
    let mut seen_files = BTreeSet::new();
    let mut seen_dirs = BTreeSet::new();
    let mut total = 0u64;
    while let Some((dir, relative)) = pending.pop() {
        for item in std::fs::read_dir(&dir).map_err(|_| INVALID)? {
            let item = item.map_err(|_| INVALID)?;
            let name = item.file_name().into_string().map_err(|_| INVALID)?;
            let name = if relative.is_empty() { name } else { format!("{relative}/{name}") };
            let path: PathBuf = item.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|_| INVALID)?;
            if metadata.file_type().is_symlink() || path.canonicalize().map_err(|_| INVALID)? != path { return Err(INVALID.into()); }
            if metadata.is_dir() {
                if !facts.directories.contains(&name) || !seen_dirs.insert(name.clone()) { return Err(INVALID.into()); }
                pending.push((path, name));
                continue;
            }
            let expected = facts.files.get(&name).ok_or(INVALID)?;
            if !metadata.is_file() || metadata.len() != expected.size || !seen_files.insert(name) { return Err(INVALID.into()); }
            let mut file = std::fs::File::open(&path).map_err(|_| INVALID)?;
            let opened = file.metadata().map_err(|_| INVALID)?;
            if opened.len() != expected.size || !single_link(&opened) { return Err(INVALID.into()); }
            let mut digest = Sha256::new();
            let mut actual = 0u64;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let remaining = expected.size.saturating_sub(actual).min(MAX_TOTAL_BYTES.saturating_sub(total));
                let length = buffer.len().min(remaining.saturating_add(1) as usize);
                let count = file.read(&mut buffer[..length]).map_err(|_| INVALID)?;
                if count == 0 { break; }
                if count as u64 > remaining { return Err(INVALID.into()); }
                actual += count as u64;
                total += count as u64;
                digest.update(&buffer[..count]);
            }
            if actual != expected.size || <[u8; 32]>::from(digest.finalize()) != expected.digest { return Err(INVALID.into()); }
        }
    }
    if seen_files.len() != facts.files.len() || seen_dirs.len() != facts.directories.len() { return Err(INVALID.into()); }
    Ok(())
}
