#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::{json_edit, toml_editing, yaml_editing, ConfigFormat};

pub const MAX_CONFIG_SIZE: u64 = 8 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    read_limited(path, MAX_CONFIG_SIZE)
}

pub fn read_limited(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, String> {
    match fs::metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(format!("{} 不是普通文件", path.display()));
            }
            if metadata.len() > limit {
                return Err(format!("{} 超过配置大小上限", path.display()));
            }
            fs::read(path)
                .map(Some)
                .map_err(|e| format!("读取 {} 失败: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok() {
                return Err(format!("{} 是失效的链接，拒绝替换", path.display()));
            }
            Ok(None)
        }
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}

pub fn text(bytes: Option<&[u8]>) -> Result<String, String> {
    bytes
        .map(|bytes| {
            String::from_utf8(bytes.to_vec()).map_err(|_| "配置文件不是 UTF-8 文本".into())
        })
        .unwrap_or_else(|| Ok(String::new()))
}

pub fn parse(format: ConfigFormat, text: &str) -> Result<Value, String> {
    match format {
        ConfigFormat::Toml => toml_editing::parse(text),
        ConfigFormat::Jsonc => json_edit::parse(text, true),
        ConfigFormat::Yaml => yaml_editing::parse(text),
        _ => json_edit::parse(text, false),
    }
}

pub fn render(format: ConfigFormat, original: &str, desired: &Value) -> Result<Vec<u8>, String> {
    let text = match format {
        ConfigFormat::Toml => toml_editing::render(original, desired)?,
        ConfigFormat::Jsonc => json_edit::render(original, desired, true)?,
        ConfigFormat::Yaml => yaml_editing::render(original, desired)?,
        _ => json_edit::render(original, desired, false)?,
    };
    Ok(text.into_bytes())
}

pub fn target(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("配置文件必须使用绝对路径".into());
    }
    if fs::symlink_metadata(path).is_ok() {
        return fs::canonicalize(path).map_err(|e| format!("无法解析配置文件链接: {e}"));
    }
    let mut missing = Vec::new();
    let mut ancestor = path;
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or("配置路径无法解析")?;
        missing.push(name.to_os_string());
        ancestor = ancestor.parent().ok_or("配置路径没有父目录")?;
    }
    let mut result = fs::canonicalize(ancestor).map_err(|e| format!("无法解析配置目录: {e}"))?;
    for part in missing.into_iter().rev() {
        result.push(part);
    }
    Ok(result)
}

pub fn stage(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or("配置路径没有父目录")?;
    fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let temp = parent.join(format!(
        ".agent2api-{}-{}-{}.tmp",
        std::process::id(),
        crate::server::logging::now_ms(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|e| format!("创建配置暂存文件失败: {e}"))?;
    let result = (|| {
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())
                .map_err(|e| format!("保留配置文件权限失败: {e}"))?;
        }
        file.write_all(bytes)
            .map_err(|e| format!("写入配置暂存文件失败: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("同步配置暂存文件失败: {e}"))
    })();
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(temp)
}

pub fn replace(temp: &Path, target: &Path) -> Result<(), String> {
    fs::rename(temp, target).map_err(|e| format!("替换 {} 失败: {e}", target.display()))?;
    #[cfg(unix)]
    {
        if let Some(parent) = target.parent() {
            File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|e| format!("同步配置目录失败: {e}"))?;
        }
    }
    Ok(())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let target = target(path)?;
    let temp = stage(&target, bytes)?;
    let result = replace(&temp, &target);
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[derive(Clone)]
pub struct Change {
    pub path: PathBuf,
    pub target: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

impl Change {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let target = target(&path)?;
        let before = read(&target)?;
        Ok(Self {
            path,
            target,
            after: before.clone(),
            before,
        })
    }

    pub fn unchanged(&self) -> bool {
        self.before == self.after
    }

    pub fn check(&self) -> Result<(), String> {
        if target(&self.path)? != self.target || read(&self.target)? != self.before {
            return Err(format!(
                "{} 已被其他程序修改，请重新生成预览",
                self.path.display()
            ));
        }
        Ok(())
    }
}
