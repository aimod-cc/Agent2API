use serde::Serialize;

use crate::server::core::system_info;

use super::{selfupdate, systemd, version};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCapability {
    pub deployment: &'static str,
    pub mode: &'static str,
    pub package_kind: &'static str,
    pub install_supported: bool,
    pub reason: Option<&'static str>,
}

/// 运行形态与安装能力分别判断：服务端关闭自动安装后，仍然要取服务端包。
pub fn current() -> UpdateCapability {
    let deployment = system_info::deployment();
    let server = deployment != "desktop";
    let (mode, reason) = match deployment {
        "desktop" => ("desktop", None),
        "docker" if selfupdate::supported() => ("docker", None),
        "docker" => ("manual", Some("disabled")),
        "systemd" => match systemd::unavailable_reason() {
            None => ("systemd", None),
            reason => ("manual", reason),
        },
        "windows-service" => match super::windows_service::unavailable_reason() {
            None => ("windows-service", None),
            reason => ("manual", reason),
        },
        _ => ("manual", Some("unmanaged")),
    };
    let unsupported_arch = server && version::server_bundle_name().is_none();
    UpdateCapability {
        deployment,
        mode: if unsupported_arch { "manual" } else { mode },
        package_kind: if server { "server" } else { "desktop" },
        install_supported: !unsupported_arch && mode != "manual",
        reason: if unsupported_arch { Some("unsupported_arch") } else { reason },
    }
}
