//! 插件目录、发行版与下载统一读取官网，官方包保持可信签名校验。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) use super::packages::MAX_PACK_BYTES;

use crate::server::core::update::{compare_versions, is_prerelease_version, website, UpdateError};

pub const INDEX_SOURCE: &str = website::PLUGIN_BASE;
pub const FALLBACK_REPO: &str = "Agent2Builder/agent2api-providers-releases";
const QUERY_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PackAsset {
    pub version: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub page: String,
    pub require_signed: bool,
    #[serde(default)]
    pub sha256: Option<String>,
}

pub fn asset_name(pack_id: &str, version: &str, target: &str) -> String {
    format!("{pack_id}-{version}-{target}.zip")
}

#[derive(Clone, Debug)]
pub struct IndexEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub require_signed: bool,
    pub manifest: String,
    pub icon: String,
    pub repo: String,
}

#[derive(Clone, Debug)]
pub struct PluginDetail {
    pub protocol: u32,
    pub repo: String,
    pub tag_template: String,
    pub asset_template: String,
    pub homepage: String,
    pub require_signed: bool,
}

#[derive(Clone, Debug)]
pub struct ReleaseAssets {
    pub version: String,
    pub page: String,
    pub notes: String,
    pub published_at: String,
    pub prerelease: bool,
    pub platforms: Vec<(String, String, u64)>,
    pub digests: std::collections::HashMap<String, String>,
    pub require_signed: bool,
}

impl ReleaseAssets {
    pub fn asset(&self, id: &str, target: &str) -> Option<PackAsset> {
        let (platform, url, size) = self
            .platforms
            .iter()
            .find(|(platform, _, _)| platform == target || platform == super::wasm::manifest::TARGET)?;
        Some(PackAsset {
            version: self.version.clone(),
            name: asset_name(id, &self.version, platform),
            url: url.clone(),
            size: *size,
            page: self.page.clone(),
            require_signed: self.require_signed,
            sha256: self.digests.get(url).cloned(),
        })
    }
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

pub(super) fn waiting_message(state: &crate::server::core::task_state::TaskState) -> String {
    state.waiting_message()
}

pub fn clear_cooldown() {
    super::catalog::clear_cooldown();
}

pub async fn read_index_result() -> Result<Vec<IndexEntry>, UpdateError> {
    let fallback_url = format!("{INDEX_SOURCE}/index.json");
    let response = website::fetch_json_with_fallback(website::CDN_PLUGIN_CATALOG_URL, &fallback_url, QUERY_TIMEOUT_MS).await?;
    if !response.status.is_success() {
        return Err(UpdateError::new(format!(
            "读取插件索引失败：HTTP {}",
            response.status.as_u16()
        )));
    }
    let require_signed = true;
    let document = response.value;
    if document["schema"] != 1 { return Err(UpdateError::new("官网插件目录版本不受支持")); }
    let items = document
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| UpdateError::new("插件索引缺少 plugins 数组"))?;
    if items.len() > 200 { return Err(UpdateError::new("官网插件目录过大")); }
    Ok(items
        .iter()
        .filter_map(|item| {
            let id = text(item, "id");
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return None;
            }
            let name = text(item, "name");
            let manifest = text(item, "manifest");
            Some(IndexEntry {
                name: if name.is_empty() { id.clone() } else { name },
                manifest: if manifest.is_empty() { format!("{id}/manifest.json") } else { manifest },
                id,
                description: text(item, "description"),
                require_signed,
                icon: text(item, "icon"),
                repo: item
                    .get("detail")
                    .and_then(|detail| detail.get("release"))
                    .map(|release| text(release, "repo"))
                    .filter(|repo| !repo.is_empty())
                    .or_else(|| item.get("source").map(|source| text(source, "repo")))
                    .unwrap_or_default(),
            })
        })
        .collect())
}

pub async fn read_index() -> Vec<IndexEntry> {
    read_index_result().await.unwrap_or_default()
}

fn index_path(path: &str) -> Option<String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.split('/').any(|part| part == "..")
    {
        return None;
    }
    Some(format!("{INDEX_SOURCE}/{path}"))
}

pub async fn read_detail(path: &str) -> Option<PluginDetail> {
    let response = website::fetch_json(&index_path(path)?, QUERY_TIMEOUT_MS)
        .await.ok()?;
    if !response.status.is_success() {
        return None;
    }
    let document = response.value;
    let release = document.get("release");
    Some(PluginDetail {
        protocol: document
            .get("protocol")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        repo: release.map(|value| text(value, "repo")).unwrap_or_default(),
        tag_template: release
            .map(|value| text(value, "tag"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "v{version}".to_string()),
        asset_template: release
            .map(|value| text(value, "asset"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "{id}-{version}-{target}.zip".to_string()),
        homepage: text(&document, "homepage"),
        require_signed: true,
    })
}

pub async fn read_store_icon(path: &str) -> Option<String> {
    let is_cdn = path.starts_with("https://cdn.agent2api.dev/advertisements/");
    if !is_cdn && !path.starts_with("/plugin-assets/") && !path.starts_with("/api/media/") { return None; }
    if path.contains("..") || path.contains(['?', '#', '\\']) { return None; }
    let mime = match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" if path.starts_with("/plugin-assets/") => "image/svg+xml",
        _ => return None,
    };
    let target = if is_cdn { path.to_string() } else { format!("{}{path}", website::ORIGIN) };
    let bytes = website::fetch_bytes(&target, 2 * 1024 * 1024).await.ok()?;
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    use base64::Engine;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn valid_repo(repo: &str) -> bool {
    let parts: Vec<_> = repo.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
}

fn release_version(release: &Value, template: &str) -> Option<String> {
    let tag = text(release, "tag_name");
    let (prefix, suffix) = template.split_once("{version}")?;
    let version = tag.strip_prefix(prefix)?.strip_suffix(suffix)?;
    compare_versions(version, version)?;
    Some(version.to_string())
}

pub(super) fn pick_release_assets(
    release: &Value,
    id: &str,
    detail: Option<&PluginDetail>,
) -> Option<ReleaseAssets> {
    let tag = detail
        .map(|detail| detail.tag_template.as_str())
        .unwrap_or("v{version}");
    let version = release_version(release, tag)?;
    let template = detail
        .map(|detail| detail.asset_template.as_str())
        .unwrap_or("{id}-{version}-{target}.zip")
        .replace("{id}", id)
        .replace("{version}", &version);
    let (prefix, suffix) = template.split_once("{target}")?;
    let mut platforms: Vec<_> = release
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?;
            let target = name.strip_prefix(prefix)?.strip_suffix(suffix)?;
            if target != super::wasm::manifest::TARGET
                && target != crate::server::core::task_plugins::types::TASK_SCRIPT_TARGET
            {
                return None;
            }
            let url = text(item, "browser_download_url");
            if url.is_empty() {
                return None;
            }
            Some((
                target.to_string(),
                url,
                item.get("size").and_then(Value::as_u64).unwrap_or(0),
            ))
        })
        .collect();
    platforms.sort_by(|left, right| left.0.cmp(&right.0));
    if platforms.is_empty() {
        return None;
    }
    let digests = release.get("assets").and_then(Value::as_array).into_iter().flatten().filter_map(|asset| {
        let value = asset.get("digest")?.as_str()?.strip_prefix("sha256:")?;
        (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| (text(asset, "browser_download_url"), value.to_ascii_lowercase()))
    }).collect();
    Some(ReleaseAssets {
        prerelease: release.get("prerelease").and_then(Value::as_bool) == Some(true)
            || is_prerelease_version(&version),
        version,
        page: website::release_page(&text(release, "html_url")),
        notes: text(release, "body").chars().take(32_000).collect(),
        published_at: text(release, "published_at"),
        platforms,
        digests,
        require_signed: false,
    })
}

pub(super) fn validate_release_urls(release: &Value, assets: &ReleaseAssets, id: &str) -> Result<(), UpdateError> {
    let tag = text(release, "tag_name");
    for (platform, url, _) in &assets.platforms {
        let name = asset_name(id, &assets.version, platform);
        if !website::plugin_url(url, id, &tag, &name) {
            return Err(UpdateError::new("官网插件下载地址与包或版本不一致"));
        }
    }
    Ok(())
}

pub async fn release_for(
    repo: &str,
    id: &str,
    detail: Option<&PluginDetail>,
    current: Option<&str>,
) -> Result<Option<ReleaseAssets>, UpdateError> {
    release_for_channels(repo, id, detail, current, super::settings::current().channels()).await
}

pub(super) async fn release_for_channels(
    repo: &str,
    id: &str,
    detail: Option<&PluginDetail>,
    current: Option<&str>,
    channels: crate::server::config::UpdateChannels,
) -> Result<Option<ReleaseAssets>, UpdateError> {
    if !valid_repo(repo) {
        return Err(UpdateError::new("插件发布仓库必须是 owner/repo"));
    }
    if let Ok(feed) = website::fetch_json(website::CDN_PLUGIN_CATALOG_URL, QUERY_TIMEOUT_MS).await {
        if feed.status.is_success() && feed.value["schema"] == 1 {
            if let Some(entries) = feed.value["plugins"].as_array() {
                let mut best: Option<ReleaseAssets> = None;
                let mut matched_plugin = false;
                for entry in entries.iter().filter(|item| item["id"].as_str() == Some(id)) {
                    matched_plugin = true;
                    let items: Vec<&Value> = entry["releases"]
                        .as_array()
                        .map(|list| list.iter().collect())
                        .unwrap_or_else(|| vec![&entry["release"]]);
                    for release in items {
                        if release.is_null() || release.get("draft").and_then(Value::as_bool) == Some(true) {
                            continue;
                        }
                        let Some(mut assets) = pick_release_assets(release, id, detail) else { continue; };
                        validate_release_urls(release, &assets, id)?;
                        assets.require_signed = true;
                        if !channels.accepts(assets.prerelease) { continue; }
                        if current.is_some()
                            && !assets
                                .platforms
                                .iter()
                                .any(|(target, _, _)| *target == super::host_target() || target == super::wasm::manifest::TARGET)
                        {
                            continue;
                        }
                        if best
                            .as_ref()
                            .map(|item| compare_versions(&assets.version, &item.version) == Some(1))
                            .unwrap_or(true)
                        {
                            best = Some(assets);
                        }
                    }
                }
                if matched_plugin {
                    return Ok(best);
                }
            }
        }
    }
    let custom_tag = detail
        .map(|detail| detail.tag_template != "v{version}")
        .unwrap_or(false);
    let listed = channels.beta || custom_tag;
    let response = match query_releases(
        id,
        if listed { "?per_page=100" } else { "/latest" },
        listed,
    )
    .await?
    {
        Some(response) => response,
        None => return Ok(None),
    };
    let document = response.value;
    let list: Vec<&Value> = if listed {
        document
            .as_array()
            .ok_or_else(|| UpdateError::new("插件 Release 列表格式错误"))?
            .iter()
            .collect()
    } else {
        vec![&document]
    };
    let mut best: Option<ReleaseAssets> = None;
    for release in list {
        if release.get("draft").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(mut assets) = pick_release_assets(release, id, detail) else {
            continue;
        };
        validate_release_urls(release, &assets, id)?;
        assets.require_signed = true;
        if !channels.accepts(assets.prerelease) {
            continue;
        }
        if current.is_some()
            && !assets
                .platforms
                .iter()
                .any(|(target, _, _)| *target == super::host_target() || target == super::wasm::manifest::TARGET)
        {
            continue;
        }
        if best
            .as_ref()
            .map(|item| compare_versions(&assets.version, &item.version) == Some(1))
            .unwrap_or(true)
        {
            best = Some(assets);
        }
    }
    Ok(best)
}

async fn query_releases(
    id: &str,
    suffix: &str,
    listed: bool,
) -> Result<Option<website::JsonResponse>, UpdateError> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-') {
        return Err(UpdateError::new("插件标识无效"));
    }
    let url = format!("{INDEX_SOURCE}/{id}/releases{suffix}");
    let response = website::fetch_json(&url, QUERY_TIMEOUT_MS).await?;
    if response.status.as_u16() == 404 {
        return Ok(None);
    }
    if !response.status.is_success() {
        return Err(UpdateError::new(format!(
            "查询插件更新失败：官网返回 HTTP {}",
            response.status.as_u16()
        )));
    }
    if (listed && !response.value.is_array()) || (!listed && response.value.get("tag_name").and_then(Value::as_str).is_none()) {
        return Err(UpdateError::new("官网插件版本信息格式无效"));
    }
    Ok(Some(response))
}

pub async fn latest(pack_id: &str, target: &str) -> Result<Option<PackAsset>, String> {
    latest_in(FALLBACK_REPO, pack_id, target).await
}

pub async fn latest_in(
    repo: &str,
    pack_id: &str,
    target: &str,
) -> Result<Option<PackAsset>, String> {
    Ok(release_for(repo, pack_id, None, None)
        .await.map_err(|error| error.message)?
        .and_then(|release| release.asset(pack_id, target)))
}

pub async fn release_assets_in(repo: &str, id: &str) -> Result<Option<ReleaseAssets>, UpdateError> {
    release_for(repo, id, None, None).await
}

/// progress 的 total 是已知包大小（未知时为 0），不是硬上限；
/// 元数据、响应声明与逐块累计均受 MAX_PACK_BYTES 限制。
pub async fn download(
    asset: &PackAsset,
    mut progress: impl FnMut(u64, u64),
) -> Result<Vec<u8>, String> {
    if !website::trusted_url(&asset.url) || !url::Url::parse(&asset.url).is_ok_and(|url| {
        let origin = url.origin().ascii_serialization();
        let valid_path = (origin == website::ORIGIN && url.path().starts_with("/downloads/plugins/"))
            || (origin == website::CDN_ORIGIN && url.path().starts_with("/agent2api/plugins/"));
        valid_path && url.query().is_none()
    }) { return Err("官方插件只能从官网下载，请重新刷新插件列表".to_string()); }
    crate::server::core::update::assert_downloadable(&asset.url).map_err(|error| error.message)?;
    if asset.size > MAX_PACK_BYTES as u64 {
        return Err(format!("插件包超过 {} MiB 上限", MAX_PACK_BYTES / (1024 * 1024)));
    }
    let headers = vec![
        ("User-Agent".to_string(), "agent2api-plugin".to_string()),
        ("Accept".to_string(), "application/octet-stream".to_string()),
    ];
    let version_marker = format!("-{}-", asset.version);
    let pack_id = asset.name.strip_suffix(".zip").and_then(|name| {
        name.split_once(version_marker.as_str())
            .map(|(id, _)| id.to_string())
    });
    let response = website::request(&asset.url, &headers, None).await.map_err(|error| error.message)?;
    if response.status().as_u16() != 200 {
        return Err(format!("下载插件包失败：官网返回 HTTP {}", response.status().as_u16()));
    }
    let buffer = read_download(response, asset, &mut progress).await?;
    if let Some(expected) = asset.sha256.as_ref() {
        if super::install::sha256_hex(&buffer) != *expected {
            return Err("官网下载的插件包 SHA-256 不一致，已拒绝使用".to_string());
        }
    }
    let verify = buffer.clone();
    let version = asset.version.clone();
    tokio::task::spawn_blocking(move || {
        super::packages::prepare(&verify, pack_id.as_deref(), Some(&version), true)
    }).await.map_err(|error| error.to_string())?
        .map_err(|error| format!("官网下载的插件未通过可信签名校验：{error}"))?;
    Ok(buffer)
}

async fn read_download(
    response: reqwest::Response,
    asset: &PackAsset,
    progress: &mut impl FnMut(u64, u64),
) -> Result<Vec<u8>, String> {
    let declared = response.content_length().unwrap_or(0);
    let total = if asset.size > 0 { asset.size } else { declared };
    if total > MAX_PACK_BYTES as u64 || declared > MAX_PACK_BYTES as u64 {
        return Err(format!("插件包超过 {} MiB 上限", MAX_PACK_BYTES / (1024 * 1024)));
    }
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    progress(0, total);
    loop {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(30), stream.next())
            .await
            .map_err(|_| "插件下载长时间没有数据".to_string())?;
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|error| format!("下载中断：{error}"))?;
        if buffer.len().saturating_add(chunk.len()) > MAX_PACK_BYTES {
            return Err(format!("插件包超过 {} MiB 上限", MAX_PACK_BYTES / (1024 * 1024)));
        }
        buffer.extend_from_slice(&chunk);
        progress(buffer.len() as u64, total);
    }
    let received = buffer.len() as u64;
    if received == 0
        || (declared > 0 && received != declared)
        || (asset.size > 0 && received != asset.size)
    {
        return Err("下载到的插件包不完整，已拒绝使用".to_string());
    }
    Ok(buffer)
}

pub fn check_result(
    id: &str,
    current: &str,
    release: Option<&ReleaseAssets>,
    error: Option<&str>,
) -> Value {
    let asset = release.and_then(|release| release.asset(id, &super::host_target()));
    let available = asset
        .as_ref()
        .is_some_and(|asset| compare_versions(&asset.version, current) == Some(1));
    json!({
        "id": id, "available": available, "checked": true, "current": current,
        "latest": release.map(|release| &release.version),
        "notes": release.map(|release| &release.notes),
        "publishedAt": release.map(|release| &release.published_at),
        "prerelease": release.map(|release| release.prerelease).unwrap_or(false),
        "page": release.map(|release| &release.page),
        "asset": asset.map(|asset| json!({ "name": asset.name, "url": asset.url, "size": asset.size, "page": asset.page })),
        "error": error.is_some(),
        "reason": error.map(str::to_string).or_else(|| {
            if available { None } else if release.is_some() { Some("已经是最新版本".to_string()) }
            else { Some("当前通道暂无可安装的版本".to_string()) }
        }),
    })
}

#[cfg(test)]
fn pick_asset(release: &Value, id: &str, target: &str) -> Option<PackAsset> {
    pick_release_assets(release, id, None)?.asset(id, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 资产名必须与打包脚本的产出名逐字一致（两处任一处改了命名，
    /// 更新就会变成「找不到对应平台的包」）。
    #[test]
    fn asset_name_matches_pack_script() {
        assert_eq!(
            asset_name("agent2api-providers", "0.1.0", "x86_64-pc-windows-msvc"),
            "agent2api-providers-0.1.0-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn picks_asset_by_exact_name() {
        let release = json!({
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/x/y/releases/tag/v0.2.0",
            "assets": [
                { "name": "agent2api-providers-0.2.0-wasm32-unknown-unknown.zip",
                  "browser_download_url": "https://example.com/a.zip", "size": 100 },
                { "name": "agent2api-providers-0.2.0-aarch64-apple-darwin.zip",
                  "browser_download_url": "https://example.com/b.zip", "size": 200 }
            ]
        });
        let asset = pick_asset(&release, "agent2api-providers", crate::server::core::plugin_host::wasm::manifest::TARGET)
            .expect("应当挑到 Wasm 包");
        assert_eq!(asset.version, "0.2.0");
        assert_eq!(asset.size, 100);
        assert_eq!(asset.url, "https://example.com/a.zip");
    }

    /// 找不到 Wasm 包（例如只有旧平台二进制）时返回 None。
    #[test]
    fn skips_other_platforms() {
        let release = json!({
            "tag_name": "v0.2.0",
            "assets": [
                { "name": "agent2api-providers-0.2.0-aarch64-apple-darwin.zip",
                  "browser_download_url": "https://example.com/b.zip" }
            ]
        });
        assert!(pick_asset(&release, "agent2api-providers", crate::server::core::plugin_host::wasm::manifest::TARGET).is_none());
    }

    /// tag 带 `v` 前缀也要认。
    #[test]
    fn strips_v_prefix_from_tag() {
        let release = json!({
            "tag_name": "v1.2.3",
            "assets": [
                { "name": "agent2api-providers-1.2.3-wasm32-unknown-unknown.zip",
                  "browser_download_url": "https://example.com/a.zip" }
            ]
        });
        let asset = pick_asset(&release, "agent2api-providers", crate::server::core::plugin_host::wasm::manifest::TARGET)
            .expect("应当挑到 Wasm 包");
        assert_eq!(asset.version, "1.2.3");
    }
}
