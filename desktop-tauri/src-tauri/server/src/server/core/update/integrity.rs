use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::server::{config, logging};

use super::{assert_downloadable, client, source, UpdateError};

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

pub fn remember_release(payload: &Value, repository: &str) {
    let Some(assets) = payload.get("assets").and_then(Value::as_array) else {
        return;
    };
    let now = logging::now_ms();
    if let Ok(mut cache) = digests().lock() {
        cache.retain(|_, digest| digest.until > now);
        for asset in assets {
            let Some(url) = asset.get("browser_download_url").and_then(Value::as_str) else {
                continue;
            };
            if !url.starts_with(&format!(
                "https://github.com/{repository}/releases/download/"
            )) {
                continue;
            }
            let Some(hash) = asset
                .get("digest")
                .and_then(Value::as_str)
                .and_then(parse_digest)
            else {
                continue;
            };
            cache.insert(
                url.to_string(),
                TrustedDigest {
                    value: hash,
                    size: asset.get("size").and_then(Value::as_u64).unwrap_or(0),
                    until: now + 30 * 60_000,
                },
            );
        }
    }
}

fn cached(value: &str) -> Option<(String, u64)> {
    digests().lock().ok().and_then(|cache| {
        cache
            .get(value)
            .filter(|digest| digest.until > logging::now_ms())
            .map(|digest| (digest.value.clone(), digest.size))
    })
}

pub async fn trusted_digest(value: &str) -> Option<(String, u64)> {
    if let Some(digest) = cached(value) {
        return Some(digest);
    }
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port_or_known_default() != Some(443)
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let [owner, repo, "releases", "download", tag, _name] = parts.as_slice() else {
        return None;
    };
    let endpoint = format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}");
    let response = client::fetch_official(&endpoint, &client::github_headers(), Some(15_000))
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload: Value = response.json().await.ok()?;
    remember_release(&payload, &format!("{owner}/{repo}"));
    cached(value)
}

pub async fn download_plan(value: &str) -> Result<Value, UpdateError> {
    assert_downloadable(value)?;
    let digest = trusted_digest(value).await;
    let candidates = source::configured_candidates(value, digest.is_some());
    Ok(json!({
        "urls": candidates.iter().map(|candidate| &candidate.url).collect::<Vec<_>>(),
        "sha256": digest.as_ref().map(|(hash, _)| hash),
        "size": digest.as_ref().map(|(_, size)| *size).unwrap_or(0),
        "mode": config::github_access().mode,
        "probeOnDevice": true,
        "mirrorAllowed": digest.is_some(),
    }))
}
