use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{update_raw_field, with_raw};

const KEY_GITHUB_ACCESS: &str = "githubAccess";
pub const DEFAULT_GITHUB_MIRROR: &str = "https://gh-proxy.com/";
const MAX_MIRRORS: usize = 6;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GithubAccessMode {
    #[default]
    Auto,
    Official,
    Accelerated,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubMirror {
    pub url: String,
    pub enabled: bool,
    pub supports_api: bool,
}

impl Default for GithubMirror {
    fn default() -> Self {
        Self { url: String::new(), enabled: true, supports_api: true }
    }
}

fn default_mirrors() -> Vec<GithubMirror> {
    [DEFAULT_GITHUB_MIRROR, "https://cdn.gh-proxy.org/", "https://ghproxy.net/"]
        .into_iter()
        .map(|url| GithubMirror {
            url: url.to_string(),
            supports_api: url != "https://ghproxy.net/",
            ..GithubMirror::default()
        })
        .collect()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct GithubAccessSettings {
    pub mode: GithubAccessMode,
    pub mirror_url: String,
    pub mirrors: Vec<GithubMirror>,
}

impl Default for GithubAccessSettings {
    fn default() -> Self {
        Self {
            mode: GithubAccessMode::Auto,
            mirror_url: DEFAULT_GITHUB_MIRROR.to_string(),
            mirrors: default_mirrors(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubAccessPatch {
    pub mode: Option<GithubAccessMode>,
    pub mirror_url: Option<String>,
    pub mirrors: Option<Vec<GithubMirror>>,
}

fn normalize_mirror(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(DEFAULT_GITHUB_MIRROR.to_string());
    }
    if value.len() > 1024 || value.contains('\\') || value.chars().any(char::is_whitespace) {
        return Err("GitHub 加速前缀不是合法地址".to_string());
    }
    let mut url = url::Url::parse(value).map_err(|_| "GitHub 加速前缀不是合法地址".to_string())?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err("GitHub 加速前缀必须是 HTTPS 地址".to_string());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains("://")
        || url.path().to_ascii_lowercase().contains("%3a")
    {
        return Err("GitHub 加速前缀不能包含凭证、查询参数或原始 GitHub 地址".to_string());
    }
    if matches!(
        url.host_str(),
        Some("github.com" | "api.github.com" | "raw.githubusercontent.com")
    ) {
        return Err("GitHub 加速前缀应填写加速站地址，而不是 GitHub 官方地址".to_string());
    }
    let path = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url.to_string())
}

fn normalize_mirrors(mirrors: Vec<GithubMirror>) -> Result<Vec<GithubMirror>, String> {
    if mirrors.len() > MAX_MIRRORS {
        return Err(format!("最多配置 {MAX_MIRRORS} 个 GitHub 加速源"));
    }
    let mut urls = std::collections::HashSet::new();
    mirrors.into_iter().map(|mut mirror| {
        if mirror.url.trim().is_empty() {
            return Err("GitHub 加速源地址不能为空".to_string());
        }
        mirror.url = normalize_mirror(&mirror.url)?;
        if !urls.insert(mirror.url.clone()) {
            return Err("GitHub 加速源地址不能重复".to_string());
        }
        if url::Url::parse(&mirror.url).is_ok_and(|url| {
            matches!(url.host_str(), Some("ghproxy.net" | "ghfast.top"))
        }) {
            mirror.supports_api = false;
        }
        Ok(mirror)
    }).collect()
}

pub fn github_access() -> GithubAccessSettings {
    let raw = with_raw(|raw| raw.get(KEY_GITHUB_ACCESS).cloned()).flatten();
    let mode = raw.as_ref().and_then(|value| value.get("mode")).cloned()
        .and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default();
    let mirror_url = raw.as_ref().and_then(|value| value.get("mirrorUrl")).and_then(Value::as_str)
        .and_then(|value| normalize_mirror(value).ok()).unwrap_or_else(|| DEFAULT_GITHUB_MIRROR.to_string());
    let mirrors = match raw.as_ref().and_then(|value| value.get("mirrors")) {
        Some(value) => serde_json::from_value::<Vec<GithubMirror>>(value.clone()).unwrap_or_default(),
        None if mirror_url != DEFAULT_GITHUB_MIRROR => vec![GithubMirror {
            url: mirror_url.clone(), ..GithubMirror::default()
        }],
        None => default_mirrors(),
    };
    GithubAccessSettings { mode, mirror_url, mirrors: normalize_mirrors(mirrors).unwrap_or_default() }
}

pub fn set_github_access(
    patch: GithubAccessPatch,
) -> Result<(GithubAccessSettings, bool, bool), String> {
    if patch.mode.is_none() && patch.mirror_url.is_none() && patch.mirrors.is_none() {
        return Err("请提供 GitHub 访问方式或加速源".to_string());
    }
    let previous = github_access();
    let mut next = previous.clone();
    if let Some(mode) = patch.mode {
        next.mode = mode;
    }
    if let Some(mirrors) = patch.mirrors {
        next.mirrors = normalize_mirrors(mirrors)?;
        if let Some(mirror) = patch.mirror_url {
            next.mirror_url = normalize_mirror(&mirror)?;
        }
        if !next.mirrors.iter().any(|mirror| mirror.url == next.mirror_url && mirror.enabled) {
            next.mirror_url = next.mirrors.iter().find(|mirror| mirror.enabled)
                .map(|mirror| mirror.url.clone()).unwrap_or_else(|| DEFAULT_GITHUB_MIRROR.to_string());
        }
    } else if let Some(mirror) = patch.mirror_url {
        let url = normalize_mirror(&mirror)?;
        if let Some(existing) = next.mirrors.iter_mut().find(|mirror| mirror.url == url) {
            existing.enabled = true;
        } else {
            let mirror = GithubMirror { url: url.clone(), ..GithubMirror::default() };
            if let Some(existing) = next.mirrors.iter_mut().find(|mirror| mirror.url == next.mirror_url) {
                *existing = mirror;
            } else {
                next.mirrors.push(mirror);
            }
        }
        next.mirrors = normalize_mirrors(next.mirrors)?;
        next.mirror_url = url;
    }
    let changed = next != previous;
    let value: Value = serde_json::to_value(&next).map_err(|error| error.to_string())?;
    let saved = update_raw_field(KEY_GITHUB_ACCESS, value);
    Ok((next, saved, changed))
}
