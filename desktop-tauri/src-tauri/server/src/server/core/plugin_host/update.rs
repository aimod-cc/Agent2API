//! 插件索引、发行版与下载。网络出口和 GitHub 令牌复用软件更新链路。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

mod required;
pub(super) use required::required_release;
pub(crate) use super::packages::MAX_PACK_BYTES;

use crate::server::core::update::client::{
    fetch_candidate, fetch_json, fetch_public, github_headers,
};
use crate::server::core::update::{compare_versions, is_prerelease_version, source};

pub const INDEX_REPO: &str = "Agent2plugin/Agent2plugin";
pub const FALLBACK_REPO: &str = "Agent2Builder/agent2api-providers-releases";
const GITHUB_API: &str = "https://api.github.com";
const QUERY_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PackAsset {
    pub version: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub page: String,
    pub require_signed: bool,
}

pub fn asset_name(pack_id: &str, version: &str, target: &str) -> String {
    format!("{pack_id}-{version}-{target}.zip")
}

#[derive(Clone, Debug)]
pub struct IndexEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub official: bool,
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
    pub require_signed: bool,
}

impl ReleaseAssets {
    pub fn asset(&self, id: &str, target: &str) -> Option<PackAsset> {
        let (_, url, size) = self
            .platforms
            .iter()
            .find(|(platform, _, _)| platform == target)?;
        Some(PackAsset {
            version: self.version.clone(),
            name: asset_name(id, &self.version, target),
            url: url.clone(),
            size: *size,
            page: self.page.clone(),
            require_signed: self.require_signed,
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

pub(super) fn retry_key() -> String {
    format!("pluginGithub:{INDEX_REPO}")
}

pub(super) fn waiting_message(state: &crate::server::core::task_state::TaskState) -> String {
    source::GithubSettings::Plugin.waiting_message(state)
}

pub(super) fn describe_error(message: &str, retry_at: i64) -> String {
    source::GithubSettings::Plugin.describe_error(message, retry_at)
}

pub fn clear_cooldown() {
    super::compatibility::clear_cooldown();
    super::catalog::clear_cooldown();
    if let Err(error) = crate::server::core::task_state::clear_cooldown(&retry_key()) {
        crate::server::logging::verbose("[Plugin]", &error);
    }
}

async fn fetch(url: &str) -> Result<reqwest::Response, String> {
    let state = crate::server::core::task_state::read(&retry_key())?;
    if source::is_official_api(url) && state.retry_at > crate::server::logging::now_ms() {
        return Err(waiting_message(&state));
    }
    let response = fetch_public(url, &github_headers(), Some(QUERY_TIMEOUT_MS))
        .await
        .map_err(|error| describe_error(&error, 0))?;
    let status = response.status().as_u16();
    if source::is_official_api(url) && (status == 403 || status == 429) {
        let retry_at = source::retry_deadline(response.headers(), status);
        let message = source::GithubSettings::Plugin.rate_limit_message(retry_at);
        if let Ok(crate::server::core::task_state::Claim::Acquired(guard)) =
            crate::server::core::task_state::claim(
                &retry_key(),
                60_000,
                true,
                crate::server::core::task_state::ManualBackoff::Respect,
                0,
            )
        {
            let _ = guard.finish(
                false,
                message.clone(),
                None,
                retry_at,
                60_000,
            );
        }
        return Err(message);
    }
    Ok(response)
}

pub async fn read_index_result() -> Result<Vec<IndexEntry>, String> {
    let url = format!("https://raw.githubusercontent.com/{INDEX_REPO}/main/index.json");
    let response = fetch_json(&url, &github_headers(), QUERY_TIMEOUT_MS, |value| {
        value.get("plugins").and_then(Value::as_array).is_some()
    })
    .await
    .map_err(|error| describe_error(&error.message, 0))?;
    if !response.status.is_success() {
        return Err(format!(
            "读取插件索引失败：HTTP {}",
            response.status.as_u16()
        ));
    }
    let require_signed = !response.official;
    let document = response.value;
    let items = document
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| "插件索引缺少 plugins 数组".to_string())?;
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
            Some(IndexEntry {
                name: if name.is_empty() { id.clone() } else { name },
                id,
                description: text(item, "description"),
                official: item.get("official").and_then(Value::as_bool) == Some(true),
                require_signed,
                manifest: text(item, "manifest"),
                icon: text(item, "icon"),
                repo: item
                    .get("source")
                    .map(|source| text(source, "repo"))
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
    Some(format!(
        "https://raw.githubusercontent.com/{INDEX_REPO}/main/{path}"
    ))
}

pub async fn read_detail(path: &str) -> Option<PluginDetail> {
    let response = fetch_json(
        &index_path(path)?,
        &github_headers(),
        QUERY_TIMEOUT_MS,
        |value| value.is_object() && value.get("release").is_some_and(Value::is_object),
    )
    .await
    .ok()?;
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
        require_signed: !response.official,
    })
}

pub async fn read_store_icon(path: &str) -> Option<String> {
    let response = fetch(&index_path(path)?).await.ok()?;
    if !response.status().is_success() || response.content_length().unwrap_or(0) > 2 * 1024 * 1024 {
        return None;
    }
    let mime = match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        _ => return None,
    };
    let bytes = response.bytes().await.ok()?;
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

fn pick_release_assets(
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
            if target.is_empty()
                || !target
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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
    Some(ReleaseAssets {
        prerelease: release.get("prerelease").and_then(Value::as_bool) == Some(true)
            || is_prerelease_version(&version),
        version,
        page: text(release, "html_url"),
        notes: text(release, "body").chars().take(32_000).collect(),
        published_at: text(release, "published_at"),
        platforms,
        require_signed: false,
    })
}

pub async fn release_for(
    repo: &str,
    id: &str,
    detail: Option<&PluginDetail>,
    current: Option<&str>,
) -> Result<Option<ReleaseAssets>, String> {
    release_for_channels(repo, id, detail, current, super::settings::current().channels()).await
}

pub(super) async fn release_for_channels(
    repo: &str,
    id: &str,
    detail: Option<&PluginDetail>,
    current: Option<&str>,
    channels: crate::server::config::UpdateChannels,
) -> Result<Option<ReleaseAssets>, String> {
    if !valid_repo(repo) {
        return Err("插件发布仓库必须是 owner/repo".to_string());
    }
    let custom_tag = detail
        .map(|detail| detail.tag_template != "v{version}")
        .unwrap_or(false);
    let listed = channels.beta || custom_tag;
    let response = match query_releases(
        repo,
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
            .ok_or_else(|| "插件 Release 列表格式错误".to_string())?
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
        assets.require_signed =
            !response.official || detail.is_some_and(|detail| detail.require_signed);
        if !channels.accepts(assets.prerelease) {
            continue;
        }
        if current.is_some()
            && !assets
                .platforms
                .iter()
                .any(|(target, _, _)| *target == super::host_target())
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
    repo: &str,
    suffix: &str,
    listed: bool,
) -> Result<Option<crate::server::core::update::client::JsonResponse>, String> {
    if !valid_repo(repo) {
        return Err("插件发布仓库必须是 owner/repo".to_string());
    }
    let state = crate::server::core::task_state::read(&retry_key())?;
    if state.retry_at > crate::server::logging::now_ms() {
        return Err(waiting_message(&state));
    }
    let url = format!("{GITHUB_API}/repos/{repo}/releases{suffix}");
    let response = fetch_json(&url, &github_headers(), QUERY_TIMEOUT_MS, |value| {
        if listed {
            value.is_array()
        } else {
            value.get("tag_name").and_then(Value::as_str).is_some()
        }
    })
    .await
    .map_err(|error| describe_error(&error.message, 0))?;
    if response.status.as_u16() == 404 {
        return Ok(None);
    }
    if matches!(response.status.as_u16(), 403 | 429) {
        let retry_at = source::retry_deadline(&response.headers, response.status.as_u16());
        return Err(source::GithubSettings::Plugin.rate_limit_message(retry_at));
    }
    if !response.status.is_success() {
        return Err(format!(
            "查询插件更新失败：GitHub 返回 HTTP {}",
            response.status.as_u16()
        ));
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
        .await?
        .and_then(|release| release.asset(pack_id, target)))
}

pub async fn release_assets_in(repo: &str, id: &str) -> Result<Option<ReleaseAssets>, String> {
    release_for(repo, id, None, None).await
}

/// progress 的 total 是已知包大小（未知时为 0），不是硬上限；
/// 元数据、响应声明与逐块累计均受 MAX_PACK_BYTES 限制。
pub async fn download(
    asset: &PackAsset,
    mut progress: impl FnMut(u64, u64),
) -> Result<Vec<u8>, String> {
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
    let mut last_error = "没有可用的插件下载源".to_string();
    for candidate in source::download_candidates(&asset.url, pack_id.is_some(), &headers).await? {
        let started = std::time::Instant::now();
        let response = match fetch_candidate(&candidate, &headers, None).await {
            Ok(response) => response,
            Err(error) => {
                candidate.forget();
                last_error = describe_error(&error, 0);
                continue;
            }
        };
        if candidate.is_rate_limited(&response) {
            candidate.observe("rateLimited", started.elapsed().as_millis() as u64, false);
            return Err(source::GithubSettings::Plugin.rate_limit_message(
                source::retry_deadline(response.headers(), response.status().as_u16()),
            ));
        }
        if response.status().as_u16() != 200 {
            last_error = format!(
                "下载插件包失败：{}返回 HTTP {}",
                candidate.label(),
                response.status().as_u16()
            );
            if source::should_switch(&candidate, response.status())
                || candidate.source == source::Source::Mirror
            {
                candidate.forget();
                continue;
            }
            return Err(last_error);
        }
        candidate.observe("ok", started.elapsed().as_millis() as u64, false);
        match read_download(response, asset, &mut progress).await {
            Ok(buffer) => {
                if asset.require_signed || candidate.source == source::Source::Mirror {
                    let verify = buffer.clone();
                    let id = pack_id.clone();
                    let version = asset.version.clone();
                    let prepared = tokio::task::spawn_blocking(move || {
                        super::packages::prepare(&verify, id.as_deref(), Some(&version), true)
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                    if let Err(error) = prepared {
                        candidate.forget();
                        last_error = format!("加速下载的插件未通过官方签名校验：{error}");
                        continue;
                    }
                }
                candidate.remember();
                return Ok(buffer);
            }
            Err(error) => {
                candidate.forget();
                last_error = describe_error(&error, 0);
            }
        }
    }
    Err(last_error)
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
                { "name": "agent2api-providers-0.2.0-x86_64-pc-windows-msvc.zip",
                  "browser_download_url": "https://example.com/a.zip", "size": 100 },
                { "name": "agent2api-providers-0.2.0-aarch64-apple-darwin.zip",
                  "browser_download_url": "https://example.com/b.zip", "size": 200 }
            ]
        });
        let asset = pick_asset(&release, "agent2api-providers", "x86_64-pc-windows-msvc")
            .expect("应当挑到 Windows 包");
        assert_eq!(asset.version, "0.2.0");
        assert_eq!(asset.size, 100);
        assert_eq!(asset.url, "https://example.com/a.zip");
    }

    /// 找不到当前平台时返回 None（而不是错挑一个别的平台）。
    #[test]
    fn skips_other_platforms() {
        let release = json!({
            "tag_name": "v0.2.0",
            "assets": [
                { "name": "agent2api-providers-0.2.0-aarch64-apple-darwin.zip",
                  "browser_download_url": "https://example.com/b.zip" }
            ]
        });
        assert!(pick_asset(&release, "agent2api-providers", "x86_64-pc-windows-msvc").is_none());
    }

    /// tag 带 `v` 前缀也要认。
    #[test]
    fn strips_v_prefix_from_tag() {
        let release = json!({
            "tag_name": "v1.2.3",
            "assets": [
                { "name": "agent2api-providers-1.2.3-x86_64-unknown-linux-gnu.zip",
                  "browser_download_url": "https://example.com/a.zip" }
            ]
        });
        let asset = pick_asset(&release, "agent2api-providers", "x86_64-unknown-linux-gnu")
            .expect("应当挑到 Linux 包");
        assert_eq!(asset.version, "1.2.3");
    }
}
