#[cfg(windows)]
#[path = "windows/platform.rs"]
mod platform;
#[cfg(windows)]
pub use platform::*;

#[cfg(not(windows))]
mod platform {
    use std::path::{Path, PathBuf};
    use tokio_util::sync::CancellationToken;
    use super::super::declaration::HotSwitch;

    #[derive(Clone)]
    pub struct ServiceProcess { pub pid: u32, pub path: PathBuf, pub started: u64 }
    #[derive(Clone)]
    pub struct HotSwitchTargets { pub roots: Vec<ServiceProcess>, pub services: Vec<ServiceProcess> }
    pub enum HotSwitchRespawn { Ready, ClientClosed, Canceled, TimedOut }

    pub fn product(_: &Path) -> Result<(String, [u16; 3]), String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn running_executables(_: &[String]) -> Result<Vec<PathBuf>, String> { Ok(Vec::new()) }
    pub fn running(_: &Path) -> Result<bool, String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn running_has_args(_: &Path, _: &[String]) -> Result<bool, String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn hot_switch_targets(_: &Path, _: Option<&HotSwitch>) -> Result<Option<HotSwitchTargets>, String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn stop_hot_services(_: &Path, _: &HotSwitchTargets) -> Result<(), String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn wait_hot_services_respawn(_: &Path, _: &HotSwitch, _: &HotSwitchTargets, _: &CancellationToken) -> Result<HotSwitchRespawn, String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn close(_: &Path) -> Result<(), String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn start(_: &Path, _: Option<&super::super::proxy::LaunchProxy>, _: &[String]) -> Result<(), String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn read_credential(_: &str) -> Result<Option<Credential>, String> { Err("当前平台尚未接通系统凭证写入".into()) }
    pub fn write_credential(_: &str, _: Option<&Credential>) -> Result<(), String> { Err("当前平台尚未接通系统凭证写入".into()) }
    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    pub struct Credential { pub username: String, pub bytes: Vec<u8>, pub persist: u32 }
    impl Drop for Credential {
        fn drop(&mut self) { use zeroize::Zeroize; self.bytes.zeroize(); }
    }
}
#[cfg(not(windows))]
pub use platform::*;
