use std::time::Duration;
use serde_json::Value;
use crate::server::core::update::client;

#[derive(Clone, Debug)]
pub(super) struct Failure {
    pub message: String,
    pub retry_at: i64,
}

impl From<String> for Failure {
    fn from(message: String) -> Self { Self { message, retry_at: 0 } }
}

async fn read(mut response: reqwest::Response, limit: usize) -> Result<Option<Vec<u8>>, Failure> {
    if response.status() == reqwest::StatusCode::NOT_FOUND { return Ok(None); }
    if !response.status().is_success() {
        let now = crate::server::logging::now_ms();
        let reset = response.headers().get("x-ratelimit-reset").and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok()).and_then(|value| value.checked_mul(1000));
        let retry = response.headers().get("retry-after").and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok()).and_then(|value| value.checked_mul(1000)).map(|value| now.saturating_add(value));
        return Err(Failure { message: format!("插件清单请求返回 HTTP {}", response.status().as_u16()),
            retry_at: if matches!(response.status().as_u16(), 403 | 429) { reset.into_iter().chain(retry).max().unwrap_or(now + 60_000) } else { 0 } });
    }
    if response.content_length().is_some_and(|size| size > limit as u64) {
        return Err("插件清单响应过大".to_string().into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Failure::from("插件清单读取中断".to_string()))? {
        if bytes.len().saturating_add(chunk.len()) > limit { return Err("插件清单响应过大".to_string().into()); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(bytes))
}

pub(super) async fn github_bytes(url: &str, limit: usize) -> Result<Option<Vec<u8>>, Failure> {
    let work = async {
        let response = client::fetch_official(url, &client::github_headers(), Some(45_000)).await
            .map_err(|error| Failure { retry_at: error.retry_at(), message: error.message })?;
        read(response, limit).await
    };
    tokio::time::timeout(Duration::from_secs(60), work).await
        .map_err(|_| Failure::from("GitHub 插件清单读取超时".to_string()))?
}

pub(super) async fn github(url: &str, limit: usize) -> Result<Option<Value>, Failure> {
    github_bytes(url, limit).await?.map(|bytes| serde_json::from_slice(&bytes)
        .map_err(|_| Failure::from("GitHub 插件清单不是有效 JSON".to_string()))).transpose()
}

pub(super) async fn npm(name: &str) -> Result<Option<Value>, Failure> {
    let url = format!("https://registry.npmjs.org/{}/latest", name.replace('/', "%2f"));
    let work = async {
        let headers = vec![("User-Agent".to_string(), "Agent2API-PluginCatalog".to_string())];
        let response = client::fetch_public(&url, &headers, Some(15_000)).await.map_err(Failure::from)?;
        let bytes = read(response, 2 * 1024 * 1024).await?;
        bytes.map(|bytes| serde_json::from_slice(&bytes)
            .map_err(|_| Failure::from("npm 插件信息不是有效 JSON".to_string()))).transpose()
    };
    tokio::time::timeout(Duration::from_secs(20), work).await
        .map_err(|_| Failure::from("npm 插件信息读取超时".to_string()))?
}
