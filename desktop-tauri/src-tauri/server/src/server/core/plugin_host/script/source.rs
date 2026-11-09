use serde_json::Value;
use sha2::{Digest, Sha256};

use super::storage::{Package, PackageSource};

pub(super) enum InstallSource {
    Local { path: std::path::PathBuf },
    Npm { name: String, version: String },
    Magpie { name: String, version: String },
    Github { repository: String, reference: String },
}

pub(super) fn valid_version(value: &str) -> bool {
    value.len() <= 80 && value.bytes().all(|ch| ch.is_ascii_alphanumeric() || b".-_".contains(&ch))
}

impl InstallSource {
    pub fn parse(input: &str, selector: &str) -> Result<Self, String> {
        let input = input.trim();
        let selector = selector.trim();
        if let Some(path) = input.strip_prefix("file:") {
            if !selector.is_empty() { return Err("本地插件不使用版本选择器".into()); }
            let path = std::path::PathBuf::from(path);
            if !path.is_absolute() { return Err("本地插件需要网关电脑上的绝对路径".into()); }
            return Ok(Self::Local { path });
        }
        if let Some(name) = input.strip_prefix("magpie:") {
            if !super::packages::package_name(name) || !valid_version(selector) {
                return Err("Magpie 插件包名或版本无效".into());
            }
            return Ok(Self::Magpie { name: name.into(), version: selector.into() });
        }
        if super::packages::package_name(input) {
            if !valid_version(selector) {
                return Err("插件版本无效".into());
            }
            return Ok(Self::Npm { name: input.into(), version: selector.into() });
        }
        let (repository, embedded_ref) = if let Some(path) = input.strip_prefix("github:") {
            let (repository, reference) = path.split_once('#').unwrap_or((path, ""));
            (repository.to_string(), reference.to_string())
        } else {
            let url = url::Url::parse(input).map_err(|_| "请输入 npm 包名或 GitHub 仓库地址")?;
            if url.scheme() != "https" || url.host_str() != Some("github.com")
                || url.port_or_known_default() != Some(443) || !url.username().is_empty()
                || url.password().is_some() || url.query().is_some()
            {
                return Err("只支持 github:所有者/仓库 或 https://github.com/所有者/仓库".into());
            }
            let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
            if parts.len() < 2 { return Err("GitHub 仓库地址不完整".into()); }
            let repository = format!("{}/{}", parts[0], parts[1]);
            let reference = if parts.len() == 2 {
                url.fragment().unwrap_or("").to_string()
            } else if parts.len() >= 4 && matches!(parts[2], "tree" | "commit") && url.fragment().is_none() {
                parts[3..].join("/")
            } else {
                return Err("请使用 GitHub 仓库地址，并在版本栏指定分支、标签或提交".into());
            };
            (repository, reference)
        };
        let repository = repository.trim_end_matches('/').strip_suffix(".git").unwrap_or(repository.trim_end_matches('/')).to_string();
        let parts: Vec<_> = repository.split('/').collect();
        if parts.len() != 2 || parts.iter().any(|part| {
            part.is_empty() || part.len() > 100 || part.starts_with('.')
                || !part.bytes().all(|ch| ch.is_ascii_alphanumeric() || b"._-".contains(&ch))
        }) {
            return Err("GitHub 仓库标识无效".into());
        }
        if !selector.is_empty() && !embedded_ref.is_empty() && selector != embedded_ref {
            return Err("GitHub 地址中的分支或标签与版本栏不一致".into());
        }
        let reference = if selector.is_empty() { embedded_ref } else { selector.into() };
        if reference.len() > 200 || reference.chars().any(|ch| ch.is_control() || ch.is_whitespace())
            || reference.contains(['\\', '?', '#', '%', ':']) || reference.contains("..")
            || (!reference.is_empty() && reference.split('/').any(|part| part.is_empty() || part == "." || part == ".."))
        {
            return Err("GitHub 分支、标签或提交无效".into());
        }
        Ok(Self::Github { repository, reference })
    }

    pub async fn download(&self) -> Result<Package, String> {
        match self {
            Self::Local { path } => super::local::package(path).await,
            Self::Npm { name, version } => super::packages::download(name, version).await,
            Self::Magpie { name, version } => super::distribution::download(name, version).await,
            Self::Github { repository, reference } => download(repository, reference).await,
        }
    }
}

async fn download(repository: &str, reference: &str) -> Result<Package, String> {
    super::operations::stage("package", "正在解析 GitHub 插件提交");
    let mut endpoint = url::Url::parse(&format!("https://api.github.com/repos/{repository}/commits"))
        .map_err(|_| "GitHub 仓库地址无效")?;
    if reference.is_empty() {
        endpoint.query_pairs_mut().append_pair("per_page", "1");
    } else {
        endpoint.path_segments_mut().map_err(|_| "GitHub 版本地址无效")?.push(reference);
    }
    let bytes = super::download::github_bytes(endpoint.as_str(), 4 * 1024 * 1024).await?;
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| "GitHub 提交信息无法解析")?;
    let commit = if reference.is_empty() { &response[0]["sha"] } else { &response["sha"] }
        .as_str().filter(|sha| sha.len() == 40 && sha.bytes().all(|ch| ch.is_ascii_hexdigit()))
        .ok_or("GitHub 没有返回有效的固定提交")?.to_ascii_lowercase();
    super::operations::stage("package", "正在下载 GitHub 脚本插件");
    // 源码没有 npm integrity；仅从 GitHub 官方源下载固定提交，不接受镜像代换。
    let archive = super::download::github_bytes(
        &format!("https://codeload.github.com/{repository}/tar.gz/{commit}"),
        super::packages::MAX_PACKAGE_BYTES,
    ).await?;
    let source = PackageSource::Github {
        repository: repository.into(), reference: reference.into(), commit,
        sha256: format!("{:x}", Sha256::digest(&archive)),
    };
    super::packages::store_archive(archive, None, Some(source)).await
}
