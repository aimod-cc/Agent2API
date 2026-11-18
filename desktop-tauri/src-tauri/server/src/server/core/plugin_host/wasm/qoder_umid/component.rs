use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};

use super::super::{desktop_credentials as files, native};

const VERSION: &str = "1.1.89";
const METADATA: &str = "https://registry.npmjs.org/@qoder-ai%2fqodercli/1.1.89";
const TARBALL: &str = "https://registry.npmjs.org/@qoder-ai/qodercli/-/qodercli-1.1.89.tgz";
const INTEGRITY: &str = "VR7voMgwxr3QgZIX4GjfA/kUV7OyXXPQWJT6jI1uNH+jdlZr2/0tt3127nT2lm0vWWF6GPIBP/CkuPivRczXnw==";

pub(super) struct Component {
    pub(super) executable: PathBuf,
    pub(super) digest: String,
    pub(super) version: String,
    pub(super) source: &'static str,
    pub(super) environment: u32,
}

pub(super) fn root() -> PathBuf { crate::paths::config_dir().join("native-helpers/qoder-umid").join(VERSION) }

fn expected() -> Option<Vec<(&'static str, &'static str)>> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some(vec![
            ("runtime-info.exe", "2ff35e9d275e08dc8ca159b7e75a47e9a62a8b0eb1131f188db15cd514c4956b"),
            ("sgsdk.dll", "6347e22972cee39670d38a7f1a57ebf0ac4ff7c289f6a454e0ac19437a1bde71")]),
        ("linux", "x86_64") => Some(vec![("runtime-info", "e30b307eaae12680d2f4e220d4d34b0ea4f70355a624d6d55ee916e03f7d7478")]),
        ("linux", "aarch64") => Some(vec![("runtime-info", "2df9d2c5aa44cb6c45ad27f493bf3dca2aaf1e067fa0ad6e88c6cad0153329fe")]),
        ("macos", "aarch64") => Some(vec![("runtime-info", "6153434d1073ca6fb98ea737ef67b3a5bf25112695ced4e51c809e6063053446")]),
        ("macos", "x86_64") => Some(vec![("runtime-info", "5fc9bc492a3d29a18f9dcbb4cde154b417ea67d2b3fca48684db06c31c216777")]),
        _ => None,
    }
}

pub(super) fn supported() -> bool { expected().is_some() }

pub(super) fn discover() -> Result<Option<Component>, proto::WireError> {
    let Some(expected) = expected() else { return Ok(None); };
    if let Some(component) = local()? { return Ok(Some(component)); }
    let directory = root();
    if !directory.exists() { return Ok(None); }
    let mut hasher = Sha256::new();
    for (name, hash) in &expected {
        let bytes = files::file(&directory.join(name), 16 * 1024 * 1024)?;
        if format!("{:x}", Sha256::digest(&bytes)) != *hash { return Err(super::invalid()); }
        hasher.update(name.as_bytes());
        hasher.update(hash.as_bytes());
    }
    Ok(Some(Component { executable: directory.join(expected[0].0), digest: format!("{:x}", hasher.finalize()),
        version: VERSION.into(), source: "managed", environment: 4 }))
}

const SIGNED_WINDOWS: &[(&str, &str)] = &[
    ("runtime-info.exe", "4da861ed4471797f84853bd57f3fecd6e3dd90547d40214552b38dc49d6b29bd"),
    ("sgsdk.dll", "cd86784f42cec1783ed0614ded901fc369184f30f245ae57d46c4749b8402034"),
];

fn local() -> Result<Option<Component>, proto::WireError> {
    for resources in super::discovery::roots() {
        let directory = resources.join("umid");
        let executable = directory.join(if cfg!(windows) { "runtime-info.exe" } else { "runtime-info" });
        if !executable.exists() { continue; }
        let hashes = if cfg!(windows) { SIGNED_WINDOWS.to_vec() } else { expected().unwrap_or_default() };
        let mut digest = Sha256::new();
        let valid = hashes.iter().all(|(name, expected)| {
            let Ok(bytes) = files::file(&directory.join(name), 16 * 1024 * 1024) else { return false; };
            let actual = format!("{:x}", Sha256::digest(&bytes));
            digest.update(name.as_bytes());
            digest.update(actual.as_bytes());
            actual == *expected
        });
        if !valid { continue; }
        let manifest = files::object(&resources.join("build-manifest.json"), 64 * 1024).ok();
        let version = manifest.as_ref().and_then(|value| value["productVersion"].as_str()).unwrap_or(VERSION);
        return Ok(Some(Component { executable, digest: format!("{:x}", digest.finalize()),
            version: version.into(), source: "app", environment: 3 }));
    }
    let home = files::home()?;
    for folder in [".qoder", ".qoder-cn"] {
        let bin = home.join(folder).join(".bin");
        let Ok(entries) = std::fs::read_dir(&bin) else { continue; };
        for entry in entries.take(128).flatten() {
            let name = entry.file_name();
            if name.to_str().is_none_or(|name| !name.starts_with("umid-") && !name.starts_with("runtime-info-")) { continue; }
            let path = entry.path();
            let expected = expected().unwrap_or_default();
            let mut digest = Sha256::new();
            let valid = if path.is_file() && !cfg!(windows) {
                files::file(&path, 16 * 1024 * 1024).is_ok_and(|bytes| {
                    let actual = format!("{:x}", Sha256::digest(bytes));
                    digest.update(actual.as_bytes());
                    expected.first().is_some_and(|(_, hash)| actual == *hash)
                })
            } else {
                expected.iter().all(|(name, hash)| files::file(&path.join(name), 16 * 1024 * 1024).is_ok_and(|bytes| {
                    let actual = format!("{:x}", Sha256::digest(bytes));
                    digest.update(actual.as_bytes());
                    actual == *hash
                })) && !expected.is_empty()
            };
            if valid {
                return Ok(Some(Component { executable: if path.is_file() { path } else { path.join(expected[0].0) },
                    digest: format!("{:x}", digest.finalize()), version: VERSION.into(), source: "cli", environment: 4 }));
            }
        }
    }
    Ok(None)
}

pub(super) async fn install(cancel: tokio_util::sync::CancellationToken) -> Result<Value, proto::WireError> {
    native::authorized_builtin("qoder-intl", super::HELPER)?;
    if !supported() { return Err(super::invalid()); }
    if discover()?.is_some() { return super::status(); }
    let client = crate::server::core::egress::managed::FrozenHttpClient::from_selected(None).await
        .map_err(|_| super::invalid())?;
    let metadata = download(&client, METADATA, 1024 * 1024, &cancel).await?;
    let meta: Value = super::super::managed_task::declaration::parse_object(&metadata, 1024 * 1024)
        .map_err(|_| super::invalid())?;
    if meta["name"] != "@qoder-ai/qodercli" || meta["version"] != VERSION || meta["dist"]["tarball"] != TARBALL
        || meta["dist"]["integrity"] != format!("sha512-{INTEGRITY}") { return Err(super::invalid()); }
    let archive = download(&client, TARBALL, 96 * 1024 * 1024, &cancel).await?;
    if STANDARD.encode(Sha512::digest(&archive)) != INTEGRITY { return Err(super::invalid()); }
    native::authorized_builtin("qoder-intl", super::HELPER)?;
    crate::server::core::blocking::run(move || publish(&archive, &cancel)).await
        .map_err(|_| super::invalid())??;
    super::status()
}

async fn download(client: &crate::server::core::egress::managed::FrozenHttpClient, url: &str, limit: usize,
    cancel: &tokio_util::sync::CancellationToken) -> Result<Vec<u8>, proto::WireError> {
    let work = async {
        let mut response = client.client().map_err(|_| super::invalid())?.get(url).send().await.map_err(|_| super::invalid())?;
        if !response.status().is_success() || response.url().as_str() != url || response.headers().contains_key("location") {
            return Err(super::invalid());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| super::invalid())? {
            if bytes.len().saturating_add(chunk.len()) > limit { return Err(super::invalid()); }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(super::invalid()),
        result = tokio::time::timeout(std::time::Duration::from_secs(180), work) => result.map_err(|_| super::invalid())?,
    }
}

fn publish(archive: &[u8], cancel: &tokio_util::sync::CancellationToken) -> Result<(), proto::WireError> {
    let expected = expected().ok_or_else(super::invalid)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut bundle = None;
    for entry in tar.entries().map_err(|_| super::invalid())?.take(2048) {
        let mut entry = entry.map_err(|_| super::invalid())?;
        if entry.path().map_err(|_| super::invalid())?.as_ref() != Path::new("package/bundle/qodercli.js") { continue; }
        if bundle.is_some() || !entry.header().entry_type().is_file() || entry.size() > 64 * 1024 * 1024 {
            return Err(super::invalid());
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|_| super::invalid())?;
        bundle = Some(String::from_utf8(bytes).map_err(|_| super::invalid())?);
    }
    let bundle = bundle.ok_or_else(super::invalid)?;
    let root = root();
    let parent = root.parent().ok_or_else(super::invalid)?;
    std::fs::create_dir_all(parent).map_err(|_| super::invalid())?;
    let staging = tempfile::Builder::new().prefix(".qoder-umid-").tempdir_in(parent).map_err(|_| super::invalid())?;
    for (name, expected_hash) in expected {
        let bytes = binary(&bundle, expected_hash)?;
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(staging.path().join(name))
            .map_err(|_| super::invalid())?;
        file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|_| super::invalid())?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o700)).map_err(|_| super::invalid())?;
        }
    }
    native::authorized_builtin("qoder-intl", super::HELPER)?;
    if cancel.is_cancelled() || root.exists() { return Err(super::invalid()); }
    std::fs::rename(staging.path(), &root).map_err(|_| super::invalid())?;
    discover()?.ok_or_else(super::invalid)?;
    Ok(())
}

fn binary(bundle: &str, expected: &str) -> Result<Vec<u8>, proto::WireError> {
    for mark in ["\"TVqQ", "\"TVp4", "\"f0VMRgIB", "\"z/rt/"] {
        let mut offset = 0;
        while let Some(relative) = bundle[offset..].find(mark) {
            let start = offset + relative + 1;
            let end = bundle[start..].find('"').map(|length| start + length).ok_or_else(super::invalid)?;
            offset = end + 1;
            if end - start > 24 * 1024 * 1024 { return Err(super::invalid()); }
            let Ok(bytes) = STANDARD.decode(&bundle[start..end]) else { continue; };
            if format!("{:x}", Sha256::digest(&bytes)) == expected { return Ok(bytes); }
        }
    }
    Err(super::invalid())
}
