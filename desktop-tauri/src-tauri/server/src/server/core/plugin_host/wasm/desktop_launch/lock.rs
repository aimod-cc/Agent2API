use std::fs::File;
use std::path::Path;

pub struct ClientLock { _files: Vec<File> }

pub fn client_lock(executable: &Path, targets: &std::collections::BTreeMap<String, super::storage::Target>) -> Result<ClientLock, String> {
    use sha2::{Digest, Sha256};
    let executable = super::paths::checked(executable, false)?;
    let mut keys = vec![format!("exe:{}", executable.to_string_lossy().to_ascii_lowercase())];
    for target in targets.values() {
        keys.push(match target {
            super::storage::Target::JsonFile { path, .. } | super::storage::Target::Sqlite { path, .. } => {
                format!("file:{}", super::paths::checked(path, true)?.to_string_lossy().to_ascii_lowercase())
            }
            super::storage::Target::Keyring { target, .. } => format!("keyring:{}", target.to_ascii_lowercase()),
        });
    }
    let mut keys: Vec<_> = keys.iter().map(|key| format!("{:x}", Sha256::digest(key.as_bytes()))).collect();
    keys.sort(); keys.dedup();
    let directory = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
        .ok_or("当前用户的客户端占位目录不可用")?.join("Agent2API/desktop-client-locks");
    super::paths::checked(&directory, true)?;
    std::fs::create_dir_all(&directory).map_err(|_| "客户端切换占位目录不可用")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let mut files = Vec::new();
        for key in keys {
            let path = directory.join(format!("{key}.lock"));
            super::paths::checked(&path, true)?;
            let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(0)
                .open(path).map_err(|_| "该客户端已有切换操作，请等待完成")?;
            files.push(file);
        }
        Ok(ClientLock { _files: files })
    }
    #[cfg(not(windows))]
    {
        let _ = keys;
        Err("当前平台尚未接通本机客户端启动".into())
    }
}
