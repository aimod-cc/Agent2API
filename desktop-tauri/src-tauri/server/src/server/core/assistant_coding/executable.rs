use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use super::{config::{self, CodingConfig}, CodingError};

#[derive(Clone)]
pub struct Executable {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub display: String,
}

pub fn resolve(config: &CodingConfig) -> Result<Executable, CodingError> {
    if !config.executable.is_empty() { return candidate(PathBuf::from(&config.executable)); }
    let mut directories: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).filter(|p| p.is_absolute()).collect();
    let home = config::user_home();
    directories.extend([home.join(".local/bin"), home.join(".cargo/bin"), home.join(".bun/bin"), home.join("AppData/Roaming/npm")]);
    if !cfg!(windows) { directories.extend([PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")]); }
    for directory in directories {
        for name in if cfg!(windows) { &["codex.exe", "codex.cmd", "codex"][..] } else { &["codex"][..] } {
            let path = directory.join(name);
            if path.is_file() {
                if let Ok(executable) = candidate(path) { return Ok(executable); }
            }
        }
    }
    Err(CodingError::new(400, "未找到本机 Codex。请先安装官方 Codex，或在设置中指定程序路径"))
}

fn candidate(path: PathBuf) -> Result<Executable, CodingError> {
    if !path.is_absolute() || !path.is_file() { return Err(CodingError::new(400, "Codex 程序路径无效")); }
    let canonical = std::fs::canonicalize(&path).unwrap_or(path.clone());
    let extension = canonical.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    if cfg!(windows) && extension == "exe" {
        return Ok(Executable { display: config::display_path(&canonical), program: canonical, args: Vec::new() });
    }
    if let Some(binary) = npm_binary(&canonical).or_else(|| npm_binary(&path)) {
        return Ok(Executable { display: config::display_path(&binary), program: binary, args: Vec::new() });
    }
    if cfg!(windows) && extension != "exe" {
        return Err(CodingError::new(400, "请选择 Codex 的 exe，或官方 npm 安装目录中的 codex.cmd"));
    }
    Ok(Executable { display: config::display_path(&canonical), program: canonical, args: Vec::new() })
}

fn npm_binary(path: &Path) -> Option<PathBuf> {
    let (package, target) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => ("codex-win32-x64", "x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => ("codex-win32-arm64", "aarch64-pc-windows-msvc"),
        ("macos", "x86_64") => ("codex-darwin-x64", "x86_64-apple-darwin"),
        ("macos", "aarch64") => ("codex-darwin-arm64", "aarch64-apple-darwin"),
        ("linux", "x86_64") => ("codex-linux-x64", "x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => ("codex-linux-arm64", "aarch64-unknown-linux-musl"),
        _ => return None,
    };
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    for parent in path.ancestors().skip(1).take(5) {
        for root in [parent.join("node_modules/@openai/codex"), parent.to_path_buf()] {
            if !root.join("package.json").is_file() { continue; }
            for vendor in [root.join(format!("node_modules/@openai/{package}/vendor")),
                root.parent()?.join(format!("{package}/vendor")), root.join("vendor")] {
                for directory in ["bin", "codex"] {
                    let binary = vendor.join(target).join(directory).join(name);
                    if binary.is_file() { return Some(binary); }
                }
            }
        }
    }
    None
}

impl Executable {
    pub fn command(&self, config: &CodingConfig) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.program);
        command.args(&self.args).env("CODEX_HOME", config.home()).kill_on_drop(true);
        command.current_dir(config::user_home());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        #[cfg(unix)]
        command.process_group(0);
        command
    }

    pub async fn version(&self, config: &CodingConfig) -> Result<String, CodingError> {
        let output = tokio::time::timeout(Duration::from_secs(8), self.command(config).arg("--version")
            .stdin(Stdio::null()).stderr(Stdio::null()).output()).await
            .map_err(|_| CodingError::new(504, "读取 Codex 版本超时"))?
            .map_err(|e| CodingError::new(500, format!("无法启动 Codex：{e}")))?;
        if !output.status.success() { return Err(CodingError::new(400, "Codex 版本检查失败，请确认程序路径")); }
        let version = String::from_utf8_lossy(&output.stdout).trim().chars().take(160).collect::<String>();
        if !version.to_ascii_lowercase().contains("codex") { return Err(CodingError::new(400, "所选程序不是 Codex")); }
        if let Some(number) = version.split_whitespace().find(|part| part.starts_with(|c: char| c.is_ascii_digit())) {
            let mut components = number.split('.').filter_map(|part| part.split('-').next()?.parse::<u32>().ok());
            if let (Some(major), Some(minor)) = (components.next(), components.next()) {
                if major == 0 && minor < 144 { return Err(CodingError::new(400, "本机 Codex 版本过旧，请升级到 0.144 或更新版本后再连接")); }
            }
        }
        Ok(version)
    }
}
