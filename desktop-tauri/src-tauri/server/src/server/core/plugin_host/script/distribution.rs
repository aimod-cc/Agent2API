use serde_json::Value;

use super::storage::{Package, PackageSource};

const BASE: &str = "https://raw.githubusercontent.com/magpie-community/plugins/dist/";
const INDEX_LIMIT: usize = 16 * 1024 * 1024;

/// 官方分发索引（`dist` 分支的 `index.json`）：下载与「检查更新」共用一份读法。
pub(super) async fn index() -> Result<Value, String> {
    let bytes = super::download::github_bytes(&format!("{BASE}index.json"), INDEX_LIMIT).await?;
    let index: Value = serde_json::from_slice(&bytes).map_err(|_| "Magpie 社区索引无法解析")?;
    if index["version"].as_u64() != Some(1) {
        return Err("Magpie 社区分发索引版本暂不兼容".into());
    }
    Ok(index)
}

pub(super) async fn download(name: &str, selector: &str) -> Result<Package, String> {
    super::operations::stage("package", "正在读取 Magpie 社区分发索引");
    let index = index().await?;
    let package = index["packages"].get(name).ok_or("Magpie 社区分发源没有这个插件")?;
    let version = if selector.is_empty() || selector == "latest" {
        package["latest"].as_str().ok_or("Magpie 插件缺少最新版本")?
    } else {
        selector
    };
    if version.is_empty() || !super::source::valid_version(version) {
        return Err("Magpie 插件版本无效".into());
    }
    let release = package["versions"].get(version).ok_or("Magpie 社区分发源没有这个版本")?;
    if release["manifest"]["name"].as_str() != Some(name)
        || release["manifest"]["version"].as_str() != Some(version)
    {
        return Err("Magpie 分发索引的插件身份不一致".into());
    }
    let file = release["file"].as_str().ok_or("Magpie 插件缺少下载文件")?;
    let filename = file.strip_prefix("tgz/").ok_or("Magpie 插件下载地址不属于受控分发目录")?;
    if filename.is_empty() || filename.len() > 240 || filename.starts_with('.')
        || filename.contains("..") || !filename.ends_with(".tgz")
        || !filename.bytes().all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
    {
        return Err("Magpie 插件下载文件名无效".into());
    }
    let integrity = release["integrity"].as_str().filter(|value| {
        value.starts_with("sha512-") && !value.chars().any(char::is_whitespace)
    }).ok_or("Magpie 插件缺少 SHA-512 完整性校验值")?;
    super::operations::stage("package", "正在下载 Magpie 社区脚本插件");
    let archive = super::download::github_bytes(&format!("{BASE}{file}"), super::packages::MAX_PACKAGE_BYTES).await?;
    super::packages::verify_integrity(&archive, integrity)?;
    let source = PackageSource::Magpie { file: file.into(), integrity: integrity.into() };
    super::packages::store_archive(archive, Some((name.into(), version.into())), Some(source)).await
}
