//! 兼容性只由公共接口决定，不比较插件与主程序的产品版本号。

use std::path::Path;

use agent2api_provider_protocol as proto;
use serde::Deserialize;

use super::install::{self, InstalledPack, PackRegistry};

pub(super) const CAPABILITIES: &[&str] = &[
    "accounts.cas.v1", "catalog.v1", "chat.stream.v1", "login.forms.v1",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Requirements {
    protocol: u32,
    #[serde(default)]
    requires: Vec<String>,
}

pub(super) fn validate(dir: &Path, manifest: &proto::PackManifest) -> Result<(), String> {
    install::validate_manifest(manifest, proto::PROTOCOL_VERSION)?;
    super::wasm::manifest::read(dir, manifest)?;
    let path = dir.join("host.json");
    if !path.exists() { return Ok(()); }
    let bytes = std::fs::read(path).map_err(|_| "插件公共接口要求不可读")?;
    if bytes.len() > 16 * 1024 { return Err("插件公共接口声明过大".into()); }
    let requirements: Requirements = serde_json::from_slice(&bytes).map_err(|_| "插件公共接口要求格式无效")?;
    if requirements.protocol != proto::PROTOCOL_VERSION {
        return Err(format!("插件需要通信协议 {}，当前网关支持 {}", requirements.protocol, proto::PROTOCOL_VERSION));
    }
    for capability in requirements.requires {
        if !CAPABILITIES.contains(&capability.as_str()) {
            return Err(format!("当前网关缺少插件所需公共能力：{capability}"));
        }
    }
    Ok(())
}

pub(super) fn installed_error(pack: &InstalledPack) -> Option<String> {
    let dir = install::plugins_dir().join(&pack.id).join(&pack.version);
    install::read_manifest(&dir).and_then(|manifest| validate(&dir, &manifest)).err()
}

pub(crate) fn provider_error(provider: &str) -> Option<String> {
    let registry = PackRegistry::load();
    let owners = super::ownership::owners(&registry);
    let owner = owners.get(provider)?;
    installed_error(registry.packs.iter().find(|pack| &pack.id == owner)?)
}
