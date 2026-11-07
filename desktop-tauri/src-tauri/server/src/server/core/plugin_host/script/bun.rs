use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Value};

pub const VERSION: &str = "1.4.2";

fn asset() -> Result<(&'static str, &'static str), String> {
    let musl = cfg!(target_env = "musl")
        || (cfg!(target_os = "linux")
            && std::fs::read_dir("/lib").ok().is_some_and(|entries| {
                entries
                    .flatten()
                    .any(|entry| entry.file_name().to_string_lossy().starts_with("ld-musl-"))
            }));
    match (std::env::consts::OS, std::env::consts::ARCH, musl) {
        ("windows", "x86_64", _) => Ok((
            "bun-windows-x64-baseline",
            "78c221c2376f79731ccf4e4af0b3bb46d81fefa3296c5abee09ad8a1b21e68c6",
        )),
        ("windows", "aarch64", _) => Ok((
            "bun-windows-aarch64",
            "a7a16b876a305fd1029c66dbd27007b4f6112ae896532f675878731a21e50cfd",
        )),
        ("macos", "x86_64", _) => Ok((
            "bun-darwin-x64",
            "80520d7e17526308c9185d261679ac6d27798d3803a0e9f7ff9121ab8affb012",
        )),
        ("macos", "aarch64", _) => Ok((
            "bun-darwin-aarch64",
            "90987a3a16d7db556d886ac3d551e7b6d3edf0a1cf43acaed622e8676be1d12f",
        )),
        ("linux", "x86_64", false) => Ok((
            "bun-linux-x64-baseline",
            "c678040f14fe0440eb839d37cbd0ce4c051a32da72806ac97de6a6aab6bf728f",
        )),
        ("linux", "aarch64", false) => Ok((
            "bun-linux-aarch64",
            "54328bbc2d9c8e0c9f892c544d66c57a83b84139e34909e5ee81758f1ac8fda7",
        )),
        ("linux", "x86_64", true) => Ok((
            "bun-linux-x64-musl-baseline",
            "76e1db84e98f22f78de0a87e309bfbbf297732847f9720db36750646c85c8c18",
        )),
        ("linux", "aarch64", true) => Ok((
            "bun-linux-aarch64-musl",
            "71760b6c8ea30623b81a4907cb815d48e2ea266f2e73e751534a44a0607950df",
        )),
        _ => Err("Bun 暂无适用于当前系统与处理器的运行时".into()),
    }
}

fn executable() -> &'static str {
    if cfg!(windows) {
        "bun.exe"
    } else {
        "bun"
    }
}
fn installed() -> Result<PathBuf, String> {
    Ok(crate::server::config::config_dir()
        .join("runtimes")
        .join("bun")
        .join(VERSION)
        .join(asset()?.0)
        .join(executable()))
}

pub fn snapshot() -> Value {
    match installed() {
        Ok(path) => {
            json!({ "version": VERSION, "installed": path.is_file(), "path": path, "downloaded": true })
        }
        Err(error) => json!({ "version": VERSION, "installed": false, "error": error }),
    }
}

/// 下载与清理共用同一把互斥锁：删除运行时目录不能与按需下载并发
fn download_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub async fn ensure() -> Result<PathBuf, String> {
    let _guard = download_lock().lock().await;
    let path = installed()?;
    if path.is_file() {
        validate(&path).await?;
        return Ok(path);
    }
    let (target, checksum) = asset()?;
    super::operations::stage("runtime", "正在下载 Bun 运行时");
    let url =
        format!("https://github.com/oven-sh/bun/releases/download/bun-v{VERSION}/{target}.zip");
    let bytes = super::download::bytes(&url, 200 * 1024 * 1024, Some(checksum)).await?;
    let destination = path.clone();
    tokio::task::spawn_blocking(move || extract(&bytes, target, &destination))
        .await
        .map_err(|error| format!("安装 Bun 任务中断：{error}"))??;
    validate(&path).await?;
    Ok(path)
}

/// 删除整个运行时目录（含全部版本），供启动失败后的重置入口调用，
/// 下次使用脚本插件时按需重新下载，目录不存在视为已清理
pub async fn clean() -> Result<(), String> {
    let _guard = download_lock().lock().await;
    let root = crate::server::config::config_dir()
        .join("runtimes")
        .join("bun");
    tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Bun 运行时目录清理失败：{error}")),
    })
    .await
    .map_err(|error| format!("清理 Bun 运行时任务中断：{error}"))?
}

async fn validate(path: &Path) -> Result<(), String> {
    let mut command = tokio::process::Command::new(path);
    command
        .arg("--version")
        .kill_on_drop(true)
        .env_remove("BUN_OPTIONS")
        .env_remove("NODE_OPTIONS")
        .env("DO_NOT_TRACK", "1");
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), command.output())
        .await
        .map_err(|_| "Bun 启动检查超时".to_string())?
        .map_err(|error| format!("无法启动下载的 Bun：{error}"))?;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != VERSION {
        return Err("Bun 运行时无法正常启动，请检查操作系统版本及系统依赖".into());
    }
    Ok(())
}

fn extract(bytes: &[u8], target: &str, destination: &Path) -> Result<(), String> {
    use std::io::{Read, Write};
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let expected = format!("{target}/{}", executable());
    let mut file = zip
        .by_name(&expected)
        .map_err(|_| "Bun 下载包缺少当前平台入口".to_string())?;
    if file.size() > 300 * 1024 * 1024
        || file.is_dir()
        || file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
        return Err("Bun 下载包入口无效".into());
    }
    let directory = destination.parent().ok_or("Bun 安装路径无效")?;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    let copied = std::io::copy(
        &mut file.by_ref().take(300 * 1024 * 1024 + 1),
        &mut temporary,
    )
    .map_err(|error| error.to_string())?;
    if copied > 300 * 1024 * 1024 {
        return Err("Bun 入口超过大小限制".into());
    }
    temporary
        .flush()
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
    }
    temporary
        .persist_noclobber(destination)
        .map_err(|error| error.to_string())?;
    Ok(())
}
