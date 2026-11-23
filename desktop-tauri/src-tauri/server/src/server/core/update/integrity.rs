use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::server::logging;

use super::{assert_downloadable, website, UpdateError};

struct TrustedDigest {
    value: String,
    size: u64,
    until: i64,
}

fn digests() -> &'static Mutex<HashMap<String, TrustedDigest>> {
    static CACHE: OnceLock<Mutex<HashMap<String, TrustedDigest>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn parse_digest(value: &str) -> Option<String> {
    let hash = value.strip_prefix("sha256:")?;
    (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
}

pub fn remember_website_release(payload: &Value, repository: &str) {
    let Some(tag) = payload.get("tag_name").and_then(Value::as_str) else { return; };
    let Some(assets) = payload.get("assets").and_then(Value::as_array) else { return; };
    let now = logging::now_ms();
    if let Ok(mut cache) = digests().lock() {
        cache.retain(|_, digest| digest.until > now);
        for asset in assets {
            let Some(url) = asset.get("browser_download_url").and_then(Value::as_str) else { continue; };
            let name = asset.get("name").and_then(Value::as_str).unwrap_or("");
            if !website::installer_url(url, repository, tag, name) { continue; }
            let size = asset.get("size").and_then(Value::as_u64).unwrap_or(0);
            if size == 0 || size > super::MAX_INSTALLER_BYTES { continue; }
            let Some(hash) = asset.get("digest").and_then(Value::as_str).and_then(parse_digest) else { continue; };
            cache.insert(url.to_string(), TrustedDigest { value: hash, size, until: now + 30 * 60_000 });
        }
    }
}

fn cached(value: &str) -> Option<(String, u64)> {
    digests().lock().ok().and_then(|cache| {
        cache.get(value).filter(|digest| digest.until > logging::now_ms())
            .map(|digest| (digest.value.clone(), digest.size))
    })
}

pub async fn trusted_digest(value: &str) -> Option<(String, u64)> {
    if !website::download_url(value) { return None; }
    if let Some(digest) = cached(value) { return Some(digest); }
    let url = url::Url::parse(value).ok()?;
    let parts: Vec<_> = url.path_segments()?.collect();
    let (repository, tag) = match parts.as_slice() {
        ["downloads", owner, repo, tag, _name] => (format!("{owner}/{repo}"), *tag),
        ["agent2api", "installers", owner, repo, tag, _hash, _name] => (format!("{owner}/{repo}"), *tag),
        _ => return None,
    };
    if let Ok(feed) = website::fetch_json(website::CDN_RELEASES_URL, 15_000).await {
        if feed.status.is_success() && feed.value.get("repository").and_then(Value::as_str) == Some(repository.as_str()) {
            if let Some(releases) = feed.value.get("releases").and_then(Value::as_array) {
                for release in releases {
                    remember_website_release(release, &repository);
                }
                if let Some(digest) = cached(value) { return Some(digest); }
            }
        }
    }
    let endpoint = format!("{}/repos/{repository}/releases/tags/{tag}", website::API_BASE);
    let response = website::fetch_json(&endpoint, 15_000).await.ok()?;
    if !response.status.is_success() { return None; }
    remember_website_release(&response.value, &repository);
    cached(value)
}

pub async fn download_plan(value: &str) -> Result<Value, UpdateError> {
    assert_downloadable(value)?;
    let (sha256, size) = trusted_digest(value).await
        .ok_or_else(|| UpdateError::new("未取得官网安装包校验值，已拒绝下载，请重新检查更新"))?;
    // 旧版（如 v3.4.2 Android）读取 urls 数组，新版读取 url；同时保留两字段以保证向后兼容。
    Ok(json!({ "url": value, "urls": [value], "sha256": sha256, "size": size }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn download_plan_includes_url_and_legacy_urls_array() {
        let repository = "Agent2Builder/agent2api-releases";
        let tag = "v3.5.0-test-compat";
        let name = "Agent2API_3.5.0-test-compat_android.apk";
        let url = format!("https://agent2api.dev/downloads/{repository}/{tag}/{name}");
        let hex_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let payload = json!({
            "tag_name": tag,
            "assets": [{
                "name": name,
                "browser_download_url": url,
                "size": 1024_u64,
                "digest": format!("sha256:{hex_hash}"),
            }],
        });

        remember_website_release(&payload, repository);
        let plan = download_plan(&url).await.expect("cached release digest should produce download plan");

        assert_eq!(plan["url"], json!(url));
        assert_eq!(plan["urls"], json!([url]));
        assert_eq!(plan["sha256"], json!(hex_hash));
        assert_eq!(plan["size"], json!(1024_u64));
    }
}
