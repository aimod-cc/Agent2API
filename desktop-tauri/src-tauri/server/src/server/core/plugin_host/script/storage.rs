use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PackageSource {
    Local { path: String },
    Magpie {
        file: String,
        integrity: String,
    },
    Github {
        repository: String,
        #[serde(rename = "ref")]
        reference: String,
        commit: String,
        sha256: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub provider_icons: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PackageSource>,
    pub directory: String,
    pub enabled: bool,
    pub installed_at: i64,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub disabled_providers: Vec<String>,
    #[serde(default)]
    pub options: Value,
    #[serde(default)]
    pub provider_options: Value,
    #[serde(default)]
    pub settings_schema: Value,
    #[serde(default)]
    pub provider_aliases: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub identities: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

pub fn read() -> Result<Registry, String> {
    let path = super::root().join("registry.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "脚本插件登记信息损坏，未用空清单覆盖".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(error) => Err(format!("读取脚本插件登记信息失败：{error}")),
    }
}

pub fn save(registry: &Registry) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(registry).map_err(|error| error.to_string())?;
    atomic_write(&super::root().join("registry.json"), &bytes)?;
    super::super::startup::configure_scripts(registry);
    Ok(())
}

pub fn directory(package: &Package) -> Result<PathBuf, String> {
    let relative = Path::new(&package.directory);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("脚本插件目录无效".into());
    }
    let root = super::root().join("packages");
    let path = root.join(relative);
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let canonical = path
        .canonicalize()
        .map_err(|_| "脚本插件文件缺失，请重新安装".to_string())?;
    if !canonical.starts_with(canonical_root) {
        return Err("脚本插件目录越界".into());
    }
    Ok(canonical)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("文件目录无效")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temp.write_all(bytes)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    temp.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn host_files() -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for (name, content) in super::script_files() {
        hash.update(name.as_bytes());
        hash.update(content.as_bytes());
    }
    let directory = super::root()
        .join("host")
        .join(format!("{:x}", hash.finalize()));
    for (name, content) in super::script_files() {
        let path = directory.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(content.as_bytes()) {
            atomic_write(&path, content.as_bytes())?;
        }
    }
    Ok(directory.join("main.mjs"))
}
