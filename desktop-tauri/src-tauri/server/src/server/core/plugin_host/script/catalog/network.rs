use std::time::Duration;
use serde_json::Value;
use crate::server::core::update::client;

#[derive(Clone, Debug)]
pub(super) struct Failure {
    pub message: String,
    /// 技术细节（如 serde 的行列定位），只在错误详情弹窗里展示，不进横幅文案
    pub detail: Option<String>,
    pub retry_at: i64,
}

impl From<String> for Failure {
    fn from(message: String) -> Self { Self { message, detail: None, retry_at: 0 } }
}

impl Failure {
    /// 报错末尾附上仓库，单一仓库读失败时用户能直接定位是哪家
    pub(super) fn with_repository(self, repository: &str) -> Self {
        if repository.is_empty() { return self; }
        Self { message: format!("{}（{repository}）", self.message), detail: self.detail, retry_at: self.retry_at }
    }
}

/// 清单不是合法 JSON 的哨兵文案：read() 按它把「仓库自带损坏的清单」归入
/// 忽略生命周期，与网络类失败（值得重试）分开
pub(super) const INVALID_MANIFEST: &str = "GitHub 插件清单不是有效 JSON";

/// 解析清单 JSON：GitHub raw 上的 package.json 有的是 Windows 作者存的，
/// 带 UTF-8 BOM 时 serde 按「expected value」直接拒绝，先剥掉再解析
fn parse_manifest(bytes: &[u8]) -> serde_json::Result<Value> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    serde_json::from_slice(bytes)
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
            detail: None,
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
            .map_err(|error| Failure { retry_at: error.retry_at(), message: error.message, detail: None })?;
        read(response, limit).await
    };
    tokio::time::timeout(Duration::from_secs(60), work).await
        .map_err(|_| Failure::from("GitHub 插件清单读取超时".to_string()))?
}

pub(super) async fn github(url: &str, limit: usize) -> Result<Option<Value>, Failure> {
    github_bytes(url, limit).await?.map(|bytes| parse_manifest(&bytes)
        .map_err(|error| Failure {
            message: INVALID_MANIFEST.to_string(),
            detail: Some(error.to_string()),
            retry_at: 0,
        })).transpose()
}

pub(super) async fn npm(name: &str) -> Result<Option<Value>, Failure> {
    let url = format!("https://registry.npmjs.org/{}/latest", name.replace('/', "%2f"));
    let work = async {
        let headers = vec![("User-Agent".to_string(), "Agent2API-PluginCatalog".to_string())];
        let response = client::fetch_public(&url, &headers, Some(15_000)).await.map_err(Failure::from)?;
        let bytes = read(response, 2 * 1024 * 1024).await?;
        bytes.map(|bytes| parse_manifest(&bytes).map_err(|error| Failure {
            message: "npm 插件信息不是有效 JSON".to_string(),
            detail: Some(error.to_string()),
            retry_at: 0,
        })).transpose()
    };
    tokio::time::timeout(Duration::from_secs(20), work).await
        .map_err(|_| Failure::from("npm 插件信息读取超时".to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_prefixed_manifest_parses() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"name":"demo"}"#);
        let value = parse_manifest(&bytes).unwrap();
        assert_eq!(value["name"], "demo");
    }

    #[test]
    fn plain_manifest_unchanged() {
        let value = parse_manifest(br#"{"name":"demo"}"#).unwrap();
        assert_eq!(value["name"], "demo");
    }

    #[test]
    fn invalid_json_still_rejected() {
        assert!(parse_manifest(b"<html>blocked</html>").is_err());
        assert!(parse_manifest(b"{\"name\":\"demo\",}").is_err());
    }
}
