use futures::StreamExt;
use sha2::{Digest, Sha256};

use crate::server::core::update::client;

pub async fn bytes(
    url: &str,
    limit: usize,
    expected_sha256: Option<&str>,
) -> Result<Vec<u8>, String> {
    let headers = vec![(
        "User-Agent".to_string(),
        "Agent2API-script-plugins".to_string(),
    )];
    let response = client::fetch_public(url, &headers, None).await?;
    read(response, limit, expected_sha256).await
}

pub(super) async fn npm_metadata(url: &str) -> Result<Option<Vec<u8>>, String> {
    let headers = vec![("User-Agent".to_string(), "Agent2API-script-plugins".to_string())];
    let response = client::fetch_public(url, &headers, None).await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    read(response, 4 * 1024 * 1024, None).await.map(Some)
}

pub(super) async fn github_bytes(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let headers = client::github_headers();
    let response = client::fetch_official(url, &headers, None).await.map_err(|error| error.message)?;
    read(response, limit, None).await
}

async fn read(
    response: reqwest::Response,
    limit: usize,
    expected_sha256: Option<&str>,
) -> Result<Vec<u8>, String> {
    if !response.status().is_success() {
        return Err(format!("下载失败：HTTP {}", response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err("下载文件超过允许大小".into());
    }
    let mut result = Vec::new();
    let mut stream = response.bytes_stream();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
    while let Some(chunk) = tokio::time::timeout_at(deadline, stream.next())
        .await
        .map_err(|_| "下载等待超时".to_string())?
    {
        let chunk = chunk.map_err(|error| format!("读取下载内容失败：{error}"))?;
        if result.len().saturating_add(chunk.len()) > limit {
            return Err("下载文件超过允许大小".into());
        }
        result.extend_from_slice(&chunk);
        super::operations::download_progress(result.len());
    }
    if let Some(expected) = expected_sha256 {
        if format!("{:x}", Sha256::digest(&result)) != expected {
            return Err("下载文件校验失败，未安装或运行".into());
        }
    }
    Ok(result)
}
