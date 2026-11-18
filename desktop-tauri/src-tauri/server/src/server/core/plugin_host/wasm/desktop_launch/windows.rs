#[cfg(windows)]
#[path = "windows/platform.rs"]
mod platform;
#[cfg(windows)]
pub use platform::*;

#[cfg(not(windows))]
mod platform {
    use std::path::{Path, PathBuf};
    pub fn product(_: &Path) -> Result<(String, [u16; 3]), String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn running_executables(_: &[String]) -> Result<Vec<PathBuf>, String> { Ok(Vec::new()) }
    pub fn running(_: &Path) -> Result<bool, String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn close(_: &Path) -> Result<(), String> { Err("当前平台尚未接通本机客户端启动".into()) }
    pub fn start(_: &Path) -> Result<(), String> { Err("当前平台尚未接通本机客户端启动".into()) }
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
