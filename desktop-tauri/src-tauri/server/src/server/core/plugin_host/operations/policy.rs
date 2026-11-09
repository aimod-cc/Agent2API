use super::super::{install::PackRegistry, settings, update};
use crate::server::config::UpdateChannels;
use crate::server::core::update::compare_versions;

#[derive(Clone, Copy)]
pub(super) enum InstallPolicy {
    Manual,
    Automatic(UpdateChannels),
}

impl InstallPolicy {
    pub(super) fn signed(self) -> bool {
        !matches!(self, Self::Manual)
    }

    pub(super) fn automatic(self) -> bool {
        matches!(self, Self::Automatic(_))
    }

    pub(super) fn validate(self, id: &str, asset: &update::PackAsset) -> Result<(), String> {
        let installed = PackRegistry::load().packs.into_iter().find(|pack| pack.id == id);
        if installed.as_ref().is_some_and(|pack| {
            compare_versions(&asset.version, &pack.version) == Some(-1)
        }) {
            return Err("已安装的插件版本更高，本次下载未安装，避免降级".to_string());
        }
        match self {
            Self::Automatic(channels) => {
                let settings = settings::current();
                let allowed = settings.auto_update && installed.is_some_and(|pack| {
                    pack.enabled && pack.signed
                        && compare_versions(&asset.version, &pack.version) == Some(1)
                });
                if !allowed || !settings.auto_check || settings.channels() != channels {
                    return Err("插件不再满足自动更新条件或通道已改变，本次下载未安装".to_string());
                }
            }
            Self::Manual => {}
        }
        Ok(())
    }
}
