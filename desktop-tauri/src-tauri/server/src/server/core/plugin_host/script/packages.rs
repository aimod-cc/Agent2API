use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path};

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};

use super::storage::{Package, PackageSource};

pub(super) const MAX_PACKAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 128 * 1024 * 1024;

pub fn package_name(name: &str) -> bool {
    if name.len() > 180 || name.is_empty() {
        return false;
    }
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || b"-_.".contains(&ch))
            && !part.starts_with('.')
            && !part.starts_with('-')
    };
    if let Some(scoped) = name.strip_prefix('@') {
        scoped
            .split_once('/')
            .is_some_and(|(scope, package)| valid(scope) && valid(package))
    } else {
        valid(name)
    }
}

pub fn package_id(name: &str) -> String {
    format!(
        "script-{}",
        &format!("{:x}", Sha256::digest(name.as_bytes()))[..24]
    )
}

pub async fn download(name: &str, requested_version: &str) -> Result<Package, String> {
    if !package_name(name) {
        return Err("请输入有效的 npm 插件包名".into());
    }
    if requested_version.len() > 80
        || !requested_version
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b".-_".contains(&ch))
    {
        return Err("插件版本无效".into());
    }
    let selector = if requested_version.is_empty() {
        "latest"
    } else {
        requested_version
    };
    let url = format!(
        "https://registry.npmjs.org/{}/{selector}",
        name.replace('/', "%2f")
    );
    super::operations::stage("package", "正在读取脚本插件版本");
    let Some(bytes) = super::download::npm_metadata(&url).await? else {
        return super::distribution::download(name, requested_version).await;
    };
    let metadata: Value = serde_json::from_slice(&bytes).map_err(|_| "npm 插件信息无法解析")?;
    if metadata["name"].as_str() != Some(name) {
        return Err("npm 返回的插件包名不匹配".into());
    }
    let version = metadata["version"]
        .as_str()
        .filter(|value| {
            value.len() <= 80
                && value
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b".-_".contains(&ch))
        })
        .ok_or("npm 没有返回有效版本")?
        .to_string();
    let tarball = metadata["dist"]["tarball"]
        .as_str()
        .ok_or("npm 没有提供下载地址")?;
    let parsed = url::Url::parse(tarball).map_err(|_| "npm 下载地址无效")?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("registry.npmjs.org")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("脚本插件下载地址不属于 npm 官方源".into());
    }
    let integrity = metadata["dist"]["integrity"]
        .as_str()
        .ok_or("npm 插件缺少完整性校验值")?
        .to_string();
    super::operations::stage("package", "正在下载脚本插件");
    let archive = super::download::bytes(tarball, MAX_PACKAGE_BYTES, None).await?;
    verify_integrity(&archive, &integrity)?;
    store_archive(archive, Some((name.to_string(), version)), None).await
}

pub(super) async fn store_archive(
    archive: Vec<u8>,
    expected: Option<(String, String)>,
    source: Option<PackageSource>,
) -> Result<Package, String> {
    tokio::task::spawn_blocking(move || {
        let root = super::root().join("packages");
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let temporary = tempfile::tempdir_in(&root).map_err(|error| error.to_string())?;
        let expected_root = if matches!(source, Some(PackageSource::Github { .. })) { None } else { Some("package") };
        unpack(&archive, temporary.path(), expected_root)?;
        let (name, version) = validate_manifest(temporary.path(), expected.as_ref())?;
        let id = package_id(&name);
        let directory = format!("{id}/{}", &format!("{:x}", Sha256::digest(&archive))[..24]);
        let destination = root.join(&directory);
        std::fs::create_dir_all(destination.parent().ok_or("插件目录无效")?)
            .map_err(|error| error.to_string())?;
        if destination.exists() {
            validate_manifest(&destination, Some(&(name.clone(), version.clone())))?;
        } else {
            std::fs::rename(temporary.path(), &destination).map_err(|error| error.to_string())?;
        }
        let icon = super::icons::manifest(&destination);
        Ok(Package {
            id, name, version, icon, provider_icons: Default::default(), source, directory, enabled: true,
            installed_at: crate::server::logging::now_ms(),
            providers: Vec::new(), disabled_providers: Vec::new(),
            options: serde_json::json!({}), provider_options: serde_json::json!({}),
            settings_schema: serde_json::json!({}), provider_aliases: Default::default(), error: None,
        })
    })
    .await
    .map_err(|error| format!("解包任务中断：{error}"))?
}

pub(super) fn verify_integrity(bytes: &[u8], value: &str) -> Result<(), String> {
    for item in value.split_whitespace() {
        let Some((algorithm, expected)) = item.split_once('-') else {
            continue;
        };
        let Ok(expected) = base64::engine::general_purpose::STANDARD.decode(expected) else {
            continue;
        };
        let actual = match algorithm {
            "sha512" => Sha512::digest(bytes).to_vec(),
            "sha256" => Sha256::digest(bytes).to_vec(),
            _ => continue,
        };
        if actual == expected {
            return Ok(());
        }
    }
    Err("脚本插件完整性校验失败，未安装或运行".into())
}

fn unpack(bytes: &[u8], directory: &Path, expected_root: Option<&str>) -> Result<(), String> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut root = None;
    let mut total = 0u64;
    let mut seen = HashSet::new();
    for (index, entry) in archive
        .entries()
        .map_err(|error| error.to_string())?
        .enumerate()
    {
        if index >= 4096 {
            return Err("脚本插件文件数量过多".into());
        }
        let mut entry = entry.map_err(|error| error.to_string())?;
        // GitHub 的 git archive 带全局 PAX 提交信息，它不属于源码根目录。
        if entry.header().entry_type().is_pax_global_extensions() {
            if entry.size() > 64 * 1024 { return Err("脚本插件归档元数据过大".into()); }
            continue;
        }
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .into_owned();
        let mut components = path.components();
        let Some(Component::Normal(first)) = components.next() else {
            return Err("脚本插件包含非法根目录".into());
        };
        let first = first.to_string_lossy();
        if first.contains([':', '\\'])
            || expected_root.is_some_and(|expected| first != expected)
            || root.as_ref().is_some_and(|expected: &String| expected != &first)
        {
            return Err("脚本插件必须位于单一合法根目录中".into());
        }
        root.get_or_insert_with(|| first.to_string());
        let relative = components.as_path();
        if relative.as_os_str().is_empty() {
            continue;
        }
        let text = relative.to_string_lossy();
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
            || text.contains([':', '\\'])
        {
            return Err("脚本插件包含越界文件路径".into());
        }
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() || !seen.insert(text.to_ascii_lowercase()) {
            return Err("脚本插件包含链接、重复路径或特殊文件".into());
        }
        total = total.saturating_add(entry.size());
        if total > MAX_UNPACKED_BYTES {
            return Err("脚本插件解包大小超过限制".into());
        }
        let destination = directory.join(relative);
        std::fs::create_dir_all(destination.parent().ok_or("插件文件路径无效")?)
            .map_err(|error| error.to_string())?;
        let mut content = Vec::new();
        entry
            .by_ref()
            .take(MAX_UNPACKED_BYTES + 1)
            .read_to_end(&mut content)
            .map_err(|error| error.to_string())?;
        if content.len() as u64 > MAX_UNPACKED_BYTES {
            return Err("脚本插件文件过大".into());
        }
        std::fs::write(destination, content).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn validate_manifest(
    directory: &Path,
    expected: Option<&(String, String)>,
) -> Result<(String, String), String> {
    let bytes =
        std::fs::read(directory.join("package.json")).map_err(|_| "插件包缺少 package.json")?;
    if bytes.len() > 1024 * 1024 {
        return Err("脚本插件清单过大".into());
    }
    let package: Value = serde_json::from_slice(&bytes).map_err(|_| "脚本插件清单无法解析")?;
    let name = package["name"].as_str().filter(|name| package_name(name))
        .ok_or("脚本插件清单缺少有效包名")?;
    let version = package["version"].as_str().filter(|version| {
        !version.is_empty() && version.len() <= 80
            && version.bytes().all(|ch| ch.is_ascii_alphanumeric() || b".-_+".contains(&ch))
    }).ok_or("脚本插件清单缺少有效版本")?;
    if expected.is_some_and(|(wanted_name, wanted_version)| name != wanted_name || version != wanted_version) {
        return Err("脚本插件清单与请求的版本信息不一致".into());
    }
    let valid_entry = super::dependencies::entries(&package).into_iter().any(|entry| {
        let entry = entry.strip_prefix("./").unwrap_or(&entry);
        !entry.contains([':', '\\'])
            && Path::new(entry).components().all(|part| matches!(part, Component::Normal(_)))
            && directory.join(entry).is_file()
    });
    if !valid_entry {
        return Err("脚本插件缺少有效的 OpenCode v1 入口".into());
    }
    Ok((name.to_string(), version.to_string()))
}
