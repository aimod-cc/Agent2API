//! 升级程序独立读取官网发布策略，不信任网关传入的仓库或校验值。

use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;

use crate::server::core::update::{integrity, version, website, UpdateError, MAX_INSTALLER_BYTES};

pub(super) struct VerifiedRelease {
    pub sha256: String,
    pub size: u64,
}

pub(super) fn verify(version: &str) -> Result<VerifiedRelease, UpdateError> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
        .map_err(|error| UpdateError::new(format!("无法初始化升级校验：{error}")))?;
    runtime.block_on(fetch(version))
}

async fn get_with_fallback(
    client: &reqwest::Client,
    primary: &str,
    fallback: &str,
    limit: usize,
) -> Result<Vec<u8>, UpdateError> {
    match get(client, primary, limit).await {
        Ok(bytes) => Ok(bytes),
        Err(_) => get(client, fallback, limit).await,
    }
}

async fn fetch(tag: &str) -> Result<VerifiedRelease, UpdateError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("agent2api-service-updater")
        .build().map_err(|error| UpdateError::new(error.to_string()))?;
    let fallback_index_url = format!("{}/app.json", website::API_BASE);
    let index_bytes = get_with_fallback(&client, website::CDN_APP_INDEX_URL, &fallback_index_url, 64 * 1024).await?;
    let index: Value = serde_json::from_slice(&index_bytes)
        .map_err(|_| UpdateError::new("官网更新索引格式异常"))?;
    if index["schema"] != 1 { return Err(UpdateError::new("官网更新索引版本不受支持")); }
    let repository = index["app"]["repo"].as_str()
        .ok_or_else(|| UpdateError::new("官网更新索引缺少仓库"))?;
    if !valid_repo(repository) { return Err(UpdateError::new("官网更新索引的仓库标识无效")); }
    let expected_tag = format!("v{tag}");
    let mut matched_release: Option<Value> = None;
    if let Ok(feed_bytes) = get(&client, website::CDN_RELEASES_URL, 4 * 1024 * 1024).await {
        if let Ok(feed) = serde_json::from_slice::<Value>(&feed_bytes) {
            if feed["repository"].as_str() == Some(repository) {
                matched_release = feed["releases"]
                    .as_array()
                    .and_then(|items| {
                        items.iter().find(|item| {
                            item["draft"] != true && item["tag_name"].as_str() == Some(expected_tag.as_str())
                        })
                    })
                    .cloned();
            }
        }
    }
    let release = match matched_release {
        Some(release) => release,
        None => {
            let url = format!("{}/repos/{repository}/releases/tags/v{tag}", website::API_BASE);
            serde_json::from_slice(&get(&client, &url, 2 * 1024 * 1024).await?)
                .map_err(|_| UpdateError::new("官网发布信息格式异常"))?
        }
    };
    if release["draft"] == true || release["tag_name"].as_str() != Some(expected_tag.as_str()) {
        return Err(UpdateError::new("官网发布版本与待安装版本不符"));
    }
    let name = version::server_bundle_name().ok_or_else(|| UpdateError::new("当前架构没有服务端包"))?;
    let asset = release["assets"].as_array().and_then(|items| items.iter().find(|item| item["name"] == name))
        .ok_or_else(|| UpdateError::new("官网发布没有对应架构的服务端更新包"))?;
    let actual_url = asset["browser_download_url"].as_str().unwrap_or("");
    if !website::installer_url(actual_url, repository, &expected_tag, &name) {
        return Err(UpdateError::new("官网更新包地址与版本不匹配"));
    }
    let size = asset["size"].as_u64().filter(|size| *size > 0 && *size <= MAX_INSTALLER_BYTES)
        .ok_or_else(|| UpdateError::new("官网更新包大小异常"))?;
    let sha256 = asset["digest"].as_str().and_then(integrity::parse_digest)
        .ok_or_else(|| UpdateError::new("未取得官网校验值，未修改现有程序；请稍后重试"))?;
    Ok(VerifiedRelease { sha256, size })
}

fn valid_repo(value: &str) -> bool {
    let parts: Vec<_> = value.split('/').collect();
    parts.len() == 2 && parts.iter().all(|part| !part.is_empty() && *part != "." && *part != ".."
        && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
}

async fn get(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>, UpdateError> {
    if !website::api_url(url) { return Err(UpdateError::new("升级校验只能访问官网")); }
    let response = client.get(url).send().await
        .map_err(|error| UpdateError::new(format!("官网校验请求失败：{error}")))?;
    if !response.status().is_success() {
        return Err(UpdateError::new(format!("官网校验返回 HTTP {}，现有程序未修改", response.status().as_u16())));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| UpdateError::new(error.to_string()))?;
        if bytes.len().saturating_add(chunk.len()) > limit { return Err(UpdateError::new("官网校验响应过大")); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
