use std::time::Duration;

use serde_json::Value;

use crate::server::core::egress;

use super::{client, UpdateError};

pub const ORIGIN: &str = "https://agent2api.dev";
pub const API_BASE: &str = "https://agent2api.dev/api/updates";
pub const PLUGIN_BASE: &str = "https://agent2api.dev/api/plugins";
pub const CDN_ORIGIN: &str = "https://cdn.agent2api.dev";
pub const CDN_FEEDS_BASE: &str = "https://cdn.agent2api.dev/agent2api/feeds";
pub const CDN_APP_INDEX_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/updates/app.json";
pub const CDN_RELEASES_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/updates/releases.json";
pub const CDN_PLUGIN_CATALOG_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/plugins/catalog.json";
pub const CDN_COMMUNITY_CATALOG_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/community/catalog.json";
pub const CDN_COMMUNITY_DISTRIBUTION_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/community/distribution.json";
pub const CDN_COMMUNITY_REGISTRY_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/community/registry.json";
pub const CDN_UPSTREAM_VERSIONS_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/providers/upstream-versions.json";
pub const API_UPSTREAM_VERSIONS_URL: &str = "https://agent2api.dev/api/providers/upstream-versions";
pub const CDN_MODELS_METADATA_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/providers/models-metadata.json";
pub const API_MODELS_METADATA_URL: &str = "https://agent2api.dev/api/providers/models-metadata";
pub const CDN_MODELS_CATALOG_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/providers/models-catalog.json";
pub const API_MODELS_CATALOG_URL: &str = "https://agent2api.dev/api/providers/models-catalog";

pub struct JsonResponse {
    pub value: Value,
    pub status: reqwest::StatusCode,
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn decoded_segments(url: &url::Url) -> Option<Vec<String>> {
    url.path_segments().map(|segments| {
        segments
            .map(|part| percent_decode_segment(part))
            .collect::<Option<Vec<_>>>()
    })?
}

fn percent_decode_segment(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            let hi = (*bytes.get(idx + 1)? as char).to_digit(16)?;
            let lo = (*bytes.get(idx + 2)? as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            idx += 3;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn trusted_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        let origin = url.origin().ascii_serialization();
        let allowed_origin = origin == ORIGIN
            || (origin == CDN_ORIGIN && (url.path().starts_with("/agent2api/") || url.path().starts_with("/advertisements/")));
        allowed_origin
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}

pub fn api_url(value: &str) -> bool {
    trusted_url(value) && url::Url::parse(value).is_ok_and(|url| {
        let origin = url.origin().ascii_serialization();
        if origin == ORIGIN {
            url.path().starts_with("/api/updates/")
                || url.path().starts_with("/api/plugins/")
                || url.path().starts_with("/api/providers/")
        } else if origin == CDN_ORIGIN {
            url.path().starts_with("/agent2api/feeds/") && url.query().is_none()
        } else {
            false
        }
    })
}

pub fn download_url(value: &str) -> bool {
    trusted_url(value) && url::Url::parse(value).is_ok_and(|url| {
        let origin = url.origin().ascii_serialization();
        let valid_path = if origin == ORIGIN {
            url.path().starts_with("/downloads/")
        } else if origin == CDN_ORIGIN {
            url.path().starts_with("/agent2api/installers/") || url.path().starts_with("/agent2api/plugins/")
        } else {
            false
        };
        valid_path && url.query().is_none()
    })
}

pub fn release_page(value: &str) -> String {
    if url::Url::parse(value).is_ok_and(|url| {
        url.origin().ascii_serialization() == ORIGIN && url.username().is_empty() && url.password().is_none() && url.fragment().is_none()
    }) { value.to_string() }
    else { format!("{ORIGIN}/downloads") }
}

pub fn plugin_url(value: &str, id: &str, tag: &str, name: &str) -> bool {
    if !download_url(value) || id.is_empty() || name.is_empty() { return false; }
    let Ok(url) = url::Url::parse(value) else { return false; };
    if url.origin().ascii_serialization() == CDN_ORIGIN {
        let Some(parts) = decoded_segments(&url) else { return false; };
        return matches!(
            parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice(),
            ["agent2api", "plugins", pid, ptag, hash, pname]
                if *pid == id && *ptag == tag && is_sha256_hex(hash) && *pname == name
        );
    }
    let component = |text: &str| url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>();
    url.path() == format!("/downloads/plugins/{}/{}/{}", component(id), component(tag), component(name))
}

pub fn installer_url(value: &str, repository: &str, tag: &str, name: &str) -> bool {
    if !download_url(value) || name.is_empty() || name.contains('/') || name.contains('\\') {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else { return false; };
    if url.origin().ascii_serialization() == CDN_ORIGIN {
        let Some(parts) = decoded_segments(&url) else { return false; };
        return matches!(
            parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice(),
            ["agent2api", "installers", owner, repo, ptag, hash, pname]
                if format!("{owner}/{repo}") == repository && *ptag == tag && is_sha256_hex(hash) && *pname == name
        );
    }
    let component = |text: &str| url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>();
    url.path() == format!("/downloads/{repository}/{}/{}", component(tag), component(name))
}

/// 每次跳转均检查官网与资源类型，避免由可信入口跳到外部下载站。
pub async fn request(
    value: &str,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
) -> Result<reqwest::Response, UpdateError> {
    if !trusted_url(value) { return Err(UpdateError::new("更新只允许访问 Agent2API 官网")); }
    let download = download_url(value);
    let valid: fn(&str) -> bool = if api_url(value) { api_url }
        else if download_url(value) { download_url }
        else { trusted_url };
    let mut last_error = "无法连接官网更新服务".to_string();
    for (_, proxy) in client::resolve_egress_candidates().map_err(UpdateError::new)? {
        let http = egress::resolved_client(proxy.as_ref(), false, "", &[]).map_err(UpdateError::new)?;
        let attempt = async {
            let mut target = url::Url::parse(value).map_err(|_| UpdateError::new("官网更新地址无效"))?;
            for _ in 0..10 {
                let mut builder = http.get(target.clone())
                    .header(reqwest::header::USER_AGENT, super::USER_AGENT);
                if download { builder = builder.header(reqwest::header::ACCEPT_ENCODING, "identity"); }
                if let Some(ms) = timeout_ms { builder = builder.timeout(Duration::from_millis(ms)); }
                for (name, value) in headers {
                    if !name.eq_ignore_ascii_case("authorization") {
                        builder = builder.header(name.as_str(), value.as_str());
                    }
                }
                let response = tokio::time::timeout(Duration::from_secs(12), builder.send()).await
                    .map_err(|_| UpdateError::new("连接官网更新服务超时"))?
                    .map_err(|error| UpdateError::new(format!("无法连接官网更新服务：{}", egress::describe_error_detail(&error))))?;
                if !response.status().is_redirection() { return Ok(response); }
                let location = response.headers().get(reqwest::header::LOCATION).and_then(|value| value.to_str().ok())
                    .ok_or_else(|| UpdateError::new("官网更新跳转缺少地址"))?;
                let next = target.join(location).map_err(|_| UpdateError::new("官网更新跳转地址无效"))?;
                if !valid(next.as_str()) { return Err(UpdateError::new("官网更新请求跳转到未允许的地址")); }
                target = next;
            }
            Err(UpdateError::new("官网更新跳转次数过多"))
        };
        let result = tokio::time::timeout(Duration::from_millis(timeout_ms.unwrap_or(120_000)), attempt).await
            .unwrap_or_else(|_| Err(UpdateError::new("连接官网更新服务超时")));
        match result {
            Ok(response) if !response.status().is_server_error() => return Ok(response),
            Ok(response) => last_error = format!("官网更新服务返回 HTTP {}", response.status().as_u16()),
            Err(error) => last_error = error.message,
        }
    }
    Err(UpdateError::new(last_error))
}

pub async fn fetch_bytes(value: &str, limit: usize) -> Result<Vec<u8>, UpdateError> {
    if !trusted_url(value) || !url::Url::parse(value).is_ok_and(|url| {
        let origin = url.origin().ascii_serialization();
        if origin == ORIGIN {
            url.path().starts_with("/plugin-assets/") || url.path().starts_with("/api/media/")
        } else if origin == CDN_ORIGIN {
            url.path().starts_with("/advertisements/") && url.query().is_none()
        } else {
            false
        }
    }) { return Err(UpdateError::new("官网插件图标地址无效")); }
    let response = request(value, &[], Some(8_000)).await?;
    if !response.status().is_success() { return Err(UpdateError::new("官网插件图标不可用")); }
    read_bytes(response, limit, 8_000).await
}

async fn read_bytes(mut response: reqwest::Response, limit: usize, timeout_ms: u64) -> Result<Vec<u8>, UpdateError> {
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        if response.content_length().is_some_and(|size| size > limit as u64) {
            return Err(UpdateError::new("官网更新信息过大"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| UpdateError::new("读取官网更新信息失败"))? {
            if bytes.len().saturating_add(chunk.len()) > limit { return Err(UpdateError::new("官网更新信息过大")); }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }).await.map_err(|_| UpdateError::new("读取官网更新信息超时"))?
}

pub async fn fetch_json_limit(value: &str, timeout_ms: u64, limit: usize) -> Result<JsonResponse, UpdateError> {
    if !api_url(value) { return Err(UpdateError::new("官网更新地址无效")); }
    let headers = vec![("Accept".to_string(), "application/json".to_string())];
    let response = request(value, &headers, Some(timeout_ms)).await?;
    let status = response.status();
    if !status.is_success() { return Ok(JsonResponse { status, value: Value::Null }); }
    if !response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json")) {
        return Err(UpdateError::new("官网更新信息不是 JSON"));
    }
    let bytes = read_bytes(response, limit, timeout_ms).await?;
    let value = serde_json::from_slice(&bytes).map_err(|_| UpdateError::new("官网更新信息格式无效"))?;
    Ok(JsonResponse { value, status })
}

pub async fn fetch_json(value: &str, timeout_ms: u64) -> Result<JsonResponse, UpdateError> {
    fetch_json_limit(value, timeout_ms, 4 * 1024 * 1024).await
}

pub async fn fetch_json_with_fallback(
    primary_cdn_url: &str,
    fallback_api_url: &str,
    timeout_ms: u64,
) -> Result<JsonResponse, UpdateError> {
    if let Ok(response) = fetch_json(primary_cdn_url, timeout_ms).await {
        if response.status.is_success() {
            return Ok(response);
        }
    }
    fetch_json(fallback_api_url, timeout_ms).await
}
