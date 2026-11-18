use std::path::{Component, Path, PathBuf};

use super::declaration::{Client, Identity, Location, Root};
use crate::server::core::agents::files;

fn local_path(path: &Path) -> Result<PathBuf, String> {
    if !cfg!(windows) { return Ok(path.to_path_buf()); }
    let text = path.to_str().ok_or("本机客户端路径无效")?;
    let text = text.strip_prefix("\\\\?\\").unwrap_or(text);
    let bytes = text.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || !matches!(bytes[2], b'\\' | b'/')
        || text[2..].contains([':', '<', '>', '"', '|', '?', '*']) || text.chars().any(char::is_control) {
        return Err("本机客户端必须位于本地磁盘".into());
    }
    for part in text[3..].split(['\\', '/']).filter(|part| !part.is_empty()) {
        let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part == "." || part == ".." || part.ends_with(['.', ' '])
            || matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (base.starts_with("COM") || base.starts_with("LPT")) && base.len() == 4 && matches!(base.as_bytes()[3], b'1'..=b'9') {
            return Err("本机客户端路径无效".into());
        }
    }
    Ok(PathBuf::from(text))
}

pub fn same(a: &Path, b: &Path) -> bool {
    if !cfg!(windows) { return a == b; }
    match (local_path(a), local_path(b)) {
        (Ok(a), Ok(b)) => a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()),
        _ => false,
    }
}

pub fn checked(path: &Path, missing: bool) -> Result<PathBuf, String> {
    let path = local_path(path)?;
    if !path.is_absolute() || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir)) {
        return Err("本机客户端路径无效".into());
    }
    let mut current = Some(path.as_path());
    while let Some(part) = current {
        match std::fs::symlink_metadata(part) {
            Ok(meta) => {
                if meta.file_type().is_symlink() { return Err("本机客户端路径不能经过链接".into()); }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 { return Err("本机客户端路径不能经过重定向目录".into()); }
                }
            }
            Err(error) if missing && error.kind() == std::io::ErrorKind::NotFound => {},
            Err(_) => return Err("本机客户端文件不存在或不可读".into()),
        }
        current = part.parent().filter(|parent| !parent.as_os_str().is_empty());
    }
    local_path(&files::target(&path)?)
}

pub fn root(root: Root, executable: Option<&Path>) -> Result<PathBuf, String> {
    let name = match root {
        Root::Home => "USERPROFILE", Root::AppData => "APPDATA", Root::LocalAppData => "LOCALAPPDATA",
        Root::ProgramFiles => "ProgramFiles", Root::ProgramFilesX86 => "ProgramFiles(x86)",
        Root::Installation => return executable.and_then(Path::parent).map(Path::to_path_buf).ok_or_else(|| "客户端安装目录不可用".into()),
    };
    std::env::var_os(name).map(PathBuf::from).filter(|path| path.is_absolute())
        .ok_or_else(|| "当前用户的客户端目录不可用".into())
}

pub fn location(location: &Location, executable: Option<&Path>) -> Result<PathBuf, String> {
    let root = root(location.root, executable)?;
    let mut path = root;
    for part in location.path.split('/') { path.push(part); }
    checked(&path, true)
}

pub fn identity(client: &Client, executable: &Path) -> Result<(), String> {
    let checked = checked(executable, false)?;
    if !checked.is_file() || !executable.file_name().and_then(|name| name.to_str()).is_some_and(|name|
        client.executables.iter().any(|allowed| name.eq_ignore_ascii_case(allowed))) {
        return Err("所选程序不是插件声明的客户端".into());
    }
    match &client.identity {
        Identity::JsonProduct { path, field, values } => {
            let path = location(&Location { root: Root::Installation, path: path.clone() }, Some(executable))?;
            let bytes = files::read_limited(&path, 256 * 1024)?.ok_or("客户端身份文件不存在")?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "客户端身份文件格式不支持")?;
            if !value[field].as_str().is_some_and(|name| values.iter().any(|allowed| name.eq_ignore_ascii_case(allowed))) {
                return Err("客户端身份不符，未切换凭证".into());
            }
        }
        Identity::PeProduct { values, minimum_version, absent } => {
            for path in absent {
                let path = location(&Location { root: Root::Installation, path: path.clone() }, Some(executable))?;
                if path.exists() { return Err("该程序使用另一种客户端布局，请选择对应客户端类型".into()); }
            }
            let (product, version) = super::windows::product(executable)?;
            if !values.iter().any(|value| product.eq_ignore_ascii_case(value)) || version < *minimum_version {
                return Err("客户端身份或版本不支持，未切换凭证".into());
            }
        }
    }
    Ok(())
}

pub fn detect(client: &Client, override_path: &str) -> Result<Option<PathBuf>, String> {
    if !override_path.is_empty() {
        let path = PathBuf::from(override_path);
        identity(client, &path)?;
        return checked(&path, false).map(Some);
    }
    for path in super::windows::running_executables(&client.executables)? {
        if identity(client, &path).is_ok() { return checked(&path, false).map(Some); }
    }
    for candidate in &client.locations {
        let Ok(path) = location(candidate, None) else { continue; };
        if path.exists() && identity(client, &path).is_ok() { return checked(&path, false).map(Some); }
    }
    Ok(None)
}
