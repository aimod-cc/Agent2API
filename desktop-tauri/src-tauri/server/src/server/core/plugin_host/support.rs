use agent2api_provider_protocol as proto;
use serde::Serialize;

use super::install::{self, InstalledPack};

pub(super) const LEGACY_REASON: &str = "旧原生提供商插件已停止支持，请安装对应的 WASM 插件；已有账号和配置保留。";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackSupportState {
    Supported,
    LegacyUnsupported,
}

pub(super) fn require_wasm(manifest: &proto::PackManifest) -> Result<(), String> {
    if super::wasm::manifest::is_wasm(manifest) {
        Ok(())
    } else {
        Err(LEGACY_REASON.to_string())
    }
}

pub(crate) fn installed_state(pack: &InstalledPack) -> PackSupportState {
    let dir = install::plugins_dir().join(&pack.id).join(&pack.version);
    if super::pack_view::is_wasm_pack(&dir, &pack.entry) {
        PackSupportState::Supported
    } else {
        PackSupportState::LegacyUnsupported
    }
}

pub(super) fn legacy_reason(pack: &InstalledPack) -> Option<String> {
    (installed_state(pack) == PackSupportState::LegacyUnsupported)
        .then(|| LEGACY_REASON.to_string())
}
