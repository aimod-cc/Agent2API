use serde_json::json;

use crate::server::config::{self, GithubAccessMode};
use crate::server::core::task_state;
use crate::server::logging;

use super::{source_state, token};

#[derive(Clone, Copy)]
pub enum GithubSettings {
    Update,
    Plugin,
}

impl GithubSettings {
    fn message_for_token(self, retry_at: i64, filled: bool) -> String {
        if !filled {
            return match self {
                Self::Update => "GitHub 接口限流中，请在「更新设置」的「GitHub 令牌」中填写凭证，以提高请求限额。",
                Self::Plugin => "GitHub 接口限流中，请在「插件设置」的「GitHub 令牌」中填写凭证，以提高请求限额。",
            }
            .to_string();
        }
        let seconds = retry_at.saturating_sub(logging::now_ms()).max(0).saturating_add(999) / 1000;
        if seconds > 0 {
            format!("GitHub 接口限流中，请在 {seconds} 秒后重试")
        } else {
            "GitHub 接口限流中，请稍后重试".to_string()
        }
    }

    pub fn describe_error(self, message: &str, retry_at: i64) -> String {
        if !is_rate_limit_error(message) {
            return message.to_string();
        }
        if retry_at <= logging::now_ms() {
            return "上次 GitHub 限流的等待时间已结束，可重新检查".to_string();
        }
        self.message_for_token(retry_at, token::effective_token().is_some())
    }

    pub fn waiting_message(self, state: &task_state::TaskState) -> String {
        if !state.running() && state.retry_at > logging::now_ms() {
            if let Some(error) = state.last_error.as_deref().filter(|error| is_rate_limit_error(error)) {
                return self.describe_error(error, state.retry_at);
            }
        }
        state.waiting_message()
    }
}

pub fn is_rate_limit_error(message: &str) -> bool {
    [
        "GitHub 接口限流中",
        "GitHub 源「",
        "上次 GitHub 限流",
        "GitHub 接口处于限流冷却期",
        "GitHub 接口访问受限",
        "查询插件更新失败：GitHub 返回 HTTP 403",
        "查询插件更新失败：GitHub 返回 HTTP 429",
    ]
    .iter()
    .any(|prefix| message.contains(prefix))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Official,
    Mirror,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub url: String,
    pub original: String,
    pub source: Source,
    pub(super) id: String,
    pub(super) key: String,
    pub(super) context: String,
    pub(super) proxy: Option<serde_json::Value>,
    pub(super) egress: String,
    pub(super) mode: GithubAccessMode,
    mirror_origin: Option<url::Url>,
}

pub fn clear_preferences() {
    source_state::clear();
}

pub fn status() -> serde_json::Value {
    json!(source_state::snapshot())
}

pub fn is_official_api(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("api.github.com")
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
    })
}

pub fn is_resource(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    match url.host_str().unwrap_or("") {
        "api.github.com" => url.path().starts_with("/repos/"),
        "raw.githubusercontent.com" | "codeload.github.com" => true,
        "github.com" | "www.github.com" => url.path().contains("/releases/download/"),
        host => host.ends_with(".githubusercontent.com"),
    }
}

pub fn candidates(value: &str, allow_mirror: bool) -> Vec<Candidate> {
    let mut candidates = configured_candidates(value, allow_mirror);
    if candidates[0].mode == GithubAccessMode::Auto {
        source_state::sort(&mut candidates);
    }
    candidates
}

pub(super) fn configured_candidates(value: &str, allow_mirror: bool) -> Vec<Candidate> {
    let settings = config::github_access();
    let context = source_state::context_key_for(&settings);
    let key = source_state::request_key(value, &context);
    let proxy = config::update_proxy();
    let egress = super::rate_limit::egress_key();
    let official = Candidate {
        url: value.to_string(),
        original: value.to_string(),
        source: Source::Official,
        id: "official".to_string(),
        key: key.clone(),
        context: context.clone(),
        proxy: proxy.clone(),
        egress: egress.clone(),
        mode: settings.mode,
        mirror_origin: None,
    };
    if !allow_mirror || settings.mode == GithubAccessMode::Official || !is_resource(value) {
        return vec![official];
    }
    let mut candidates = vec![official];
    for mirror in &settings.mirrors {
        if !mirror.enabled || (is_official_api(value) && !mirror.supports_api) {
            continue;
        }
        candidates.push(Candidate {
            url: format!("{}{value}", mirror.url),
            original: value.to_string(),
            source: Source::Mirror,
            id: mirror.url.clone(),
            key: key.clone(),
            context: context.clone(),
            proxy: proxy.clone(),
            egress: egress.clone(),
            mode: settings.mode,
            mirror_origin: url::Url::parse(&mirror.url).ok(),
        });
    }
    if settings.mode == GithubAccessMode::Accelerated {
        candidates.sort_by_key(|candidate| {
            if candidate.id == settings.mirror_url { 0 }
            else if candidate.source == Source::Mirror { 1 }
            else { 2 }
        });
    }
    candidates
}

pub fn ensure_available(candidates: &[Candidate], headers: &[(String, String)]) -> Result<(), super::UpdateError> {
    let mut limits = Vec::new();
    let states = task_state::read_all().unwrap_or_default();
    for candidate in candidates {
        match candidate.stored_limit(headers, &states).filter(|limit| limit.retry_at > logging::now_ms()) {
            Some(limit) => limits.push(limit),
            None => return Ok(()),
        }
    }
    Err(if limits.is_empty() { super::UpdateError::new("没有可用的 GitHub 源") }
        else { super::UpdateError::rate_limited(limits) })
}

pub async fn download_candidates(
    value: &str,
    allow_mirror: bool,
    headers: &[(String, String)],
) -> Result<Vec<Candidate>, super::UpdateError> {
    super::probe::rank(value, allow_mirror, headers).await
}

impl Candidate {
    pub fn label(&self) -> String {
        match self.source {
            Source::Official => "官方源".to_string(),
            Source::Mirror => self.mirror_origin.as_ref()
                .and_then(|url| url.host_str()).unwrap_or("加速源").to_string(),
        }
    }

    pub fn allows_redirect(&self, value: &url::Url) -> bool {
        if !value.username().is_empty() || value.password().is_some() {
            return false;
        }
        if self.source == Source::Mirror {
            if value.scheme() != "https" {
                return false;
            }
            let official = if is_official_api(&self.original) {
                is_official_api(value.as_str())
            } else {
                is_resource(value.as_str())
            };
            return official
                || self.mirror_origin.as_ref().is_some_and(|base| {
                    value.host_str() == base.host_str()
                        && value.port_or_known_default() == base.port_or_known_default()
                });
        }
        if is_official_api(&self.original) {
            return is_official_api(value.as_str());
        }
        if is_resource(&self.original) {
            return is_resource(value.as_str());
        }
        url::Url::parse(&self.original).is_ok_and(|original| {
            value.scheme() == original.scheme()
                && value.host_str() == original.host_str()
                && value.port_or_known_default() == original.port_or_known_default()
        })
    }

    pub fn remember(&self) {
        source_state::record(self, "ok", None, true);
    }

    pub fn forget(&self) {
        source_state::record(self, "failed", None, false);
    }

    pub(crate) fn observe(&self, state: &str, elapsed_ms: u64, selected: bool) {
        source_state::record(self, state, Some(elapsed_ms), selected);
    }

}

pub fn should_switch(candidate: &Candidate, status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || (candidate.source == Source::Mirror && matches!(status.as_u16(), 400 | 404 | 405 | 416))
}

pub(crate) fn failed_state(message: &str) -> &'static str {
    if message.contains("限流") { "rateLimited" }
    else if message.contains("超时") { "timeout" }
    else if message.contains("格式") || message.contains("JSON") || message.contains("内容") { "invalid" }
    else { "failed" }
}
