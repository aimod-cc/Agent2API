use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

pub const WEBSITE_ORIGIN: &str = "https://agent2api.dev";
pub const CDN_ORIGIN: &str = "https://cdn.agent2api.dev";
pub const CDN_ADVERTISEMENTS_URL: &str = "https://cdn.agent2api.dev/agent2api/feeds/advertisements.json";
pub const MAX_FEED_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TTL_MS: i64 = 30 * 60 * 1000;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdCopy {
    pub title: String,
    pub description: String,
    pub link_label: String,
    pub tags: Vec<String>,
    pub note: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Advertisement {
    pub id: String,
    pub category: String,
    pub preset_key: String,
    pub order: u32,
    pub starts_at: Option<i64>,
    pub ends_at: Option<i64>,
    pub url: String,
    pub image_url: String,
    pub image_dark_url: String,
    pub tone: String,
    pub translations: BTreeMap<String, AdCopy>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Feed {
    pub schema_version: u32,
    pub revision: u64,
    pub server_time: i64,
    pub generated_at: i64,
    pub expires_at: i64,
    pub refresh_after_seconds: u64,
    pub enabled: bool,
    pub items: Vec<Advertisement>,
}

impl Feed {
    pub fn validate(&self) -> Result<(), String> {
        let ttl = self.expires_at.saturating_sub(self.server_time);
        if self.schema_version != 1
            || self.server_time < 0
            || !(1..=MAX_TTL_MS).contains(&ttl)
            || !(30..=3600).contains(&self.refresh_after_seconds)
            || self.items.len() > 100
        {
            return Err("官网广告配置格式无效".into());
        }
        let mut ids = HashSet::new();
        let mut presets = HashSet::new();
        for ad in &self.items {
            if !valid_id(&ad.id)
                || !ids.insert(&ad.id)
                || !["provider", "proxy"].contains(&ad.category.as_str())
                || !["gold", "blue", "neutral"].contains(&ad.tone.as_str())
                || ad.order > 10000
                || !public_url(&ad.url)
                || !safe_image(&ad.image_url)
                || !safe_image(&ad.image_dark_url)
                || ad.starts_at.is_some_and(|at| at < 0)
                || ad.ends_at.is_some_and(|at| at < 0)
                || ad
                    .starts_at
                    .zip(ad.ends_at)
                    .is_some_and(|(start, end)| end <= start)
                || !ad.translations.contains_key("zh-Hans")
                || ad.translations.len() > 6
            {
                return Err("官网广告条目格式无效".into());
            }
            if !ad.preset_key.is_empty()
                && (ad.category != "provider"
                    || !valid_id(&ad.preset_key)
                    || !presets.insert(&ad.preset_key))
            {
                return Err("官网广告模板关联无效".into());
            }
            for (locale, copy) in &ad.translations {
                if !["zh-Hans", "zh-Hant", "en", "ja", "ko", "pt-BR"].contains(&locale.as_str())
                    || !valid_text(&copy.title, 80, true)
                    || !valid_text(&copy.description, 600, true)
                    || !valid_text(&copy.link_label, 40, true)
                    || !valid_text(&copy.note, 300, false)
                    || copy.tags.len() > 6
                    || copy.tags.iter().any(|tag| !valid_text(tag, 30, true))
                {
                    return Err("官网广告文案格式无效".into());
                }
            }
        }
        Ok(())
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_text(value: &str, max: usize, required: bool) -> bool {
    (!required || !value.trim().is_empty())
        && value.chars().count() <= max
        && !value
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
}

fn public_url(value: &str) -> bool {
    if value.len() > 2048 {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.host(), Some(url::Host::Domain(domain)) if domain.contains('.') && domain != "localhost")
}

fn safe_image(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if value.len() > 256 {
        return false;
    }
    let path = if let Some(path) = value.strip_prefix("/ad-assets/") {
        path
    } else if let Some(path) = value.strip_prefix("/api/media/") {
        path
    } else if let Some(path) = value
        .strip_prefix(CDN_ORIGIN)
        .and_then(|rest| rest.strip_prefix("/advertisements/"))
    {
        path
    } else {
        return false;
    };
    let Some((name, extension)) = path.rsplit_once('.') else {
        return false;
    };
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && ["png", "jpg", "jpeg", "webp"].contains(&extension)
}

pub fn image_url(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else if path.starts_with(&format!("{CDN_ORIGIN}/")) {
        path.to_string()
    } else {
        format!("{WEBSITE_ORIGIN}{path}")
    }
}
