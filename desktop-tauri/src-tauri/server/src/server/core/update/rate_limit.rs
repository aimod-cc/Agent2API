use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::server::logging;
use crate::server::core::task_state;

use super::source::{self, Candidate, Source};
use super::UpdateError;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RateLimit {
    pub source: String,
    pub label: String,
    pub resource: String,
    pub retry_at: i64,
    pub retry_source: String,
    pub observed_at: i64,
    pub scope: String,
    pub egress: String,
}

pub fn message(limits: &[RateLimit]) -> String {
    let now = logging::now_ms();
    let active: Vec<_> = limits.iter().filter(|limit| limit.retry_at > now).collect();
    let Some(first) = active.iter().min_by_key(|limit| limit.retry_at) else {
        return "上次 GitHub 限流的等待时间已结束，可重新检查".to_string();
    };
    let mut labels = Vec::new();
    for limit in &active {
        if !labels.contains(&limit.label) { labels.push(limit.label.clone()); }
    }
    let label = labels.join("、");
    let seconds = first.retry_at.saturating_sub(now).saturating_add(999) / 1000;
    if first.retry_source == "fallback" {
        format!("GitHub 源「{label}」限流，{seconds} 秒后可重试（上游未提供恢复时间）")
    } else {
        format!("GitHub 源「{label}」限流，请在 {seconds} 秒后重试")
    }
}

pub fn decorate_cached(value: &mut Value, url: &str) {
    let reason = value.get("reason").and_then(Value::as_str).unwrap_or("");
    if !source::is_rate_limit_error(reason) {
        for field in ["rateLimited", "rateLimits", "retryAt", "canRetry", "serverTime"] {
            if let Some(object) = value.as_object_mut() { object.remove(field); }
        }
        return;
    }
    let candidates = source::configured_candidates(url, true);
    let headers = super::client::github_headers();
    let states = task_state::read_all().unwrap_or_default();
    let limits: Vec<_> = candidates.iter().filter_map(|candidate| candidate.stored_limit(&headers, &states))
        .filter(|limit| limit.retry_at > logging::now_ms()).collect();
    let can_retry = limits.len() < candidates.len();
    let retry_at = limits.iter().map(|limit| limit.retry_at).min().unwrap_or(0);
    let mut reason = message(&limits);
    if can_retry && !limits.is_empty() { reason.push_str("；其他线路可重新检查"); }
    value["reason"] = serde_json::json!(reason);
    value["rateLimited"] = serde_json::json!(true);
    value["retryAt"] = serde_json::json!(retry_at);
    value["canRetry"] = serde_json::json!(can_retry || limits.is_empty());
    value["rateLimits"] = serde_json::json!(limits);
    value["serverTime"] = serde_json::json!(logging::now_ms());
}

pub fn retry_info(headers: &HeaderMap, limited: bool) -> Option<(i64, &'static str)> {
    let now = logging::now_ms();
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim);
    let declared_at = headers.get("date").and_then(|value| value.to_str().ok())
        .and_then(|value| chrono::DateTime::parse_from_rfc2822(value).ok())
        .map(|date| date.timestamp_millis());
    let age = header("age").and_then(|value| value.parse::<i64>().ok()).unwrap_or(0).max(0).saturating_mul(1000);
    let adjusted = |timestamp: i64| declared_at.map(|server_now| {
        now.saturating_add(timestamp.saturating_sub(server_now.saturating_add(age)))
    }).unwrap_or(timestamp);
    let after = header("retry-after").and_then(|value| {
        value.parse::<i64>().ok().filter(|seconds| *seconds >= 0)
            .map(|seconds| now.saturating_add(seconds.saturating_mul(1000).saturating_sub(age)))
            .or_else(|| chrono::DateTime::parse_from_rfc2822(value).ok().map(|date| adjusted(date.timestamp_millis())))
    });
    let reset = (header("x-ratelimit-remaining") == Some("0")).then(|| {
        header("x-ratelimit-reset").and_then(|value| value.parse::<i64>().ok())
            .filter(|seconds| *seconds > 0).map(|seconds| adjusted(seconds.saturating_mul(1000)))
    }).flatten();
    match (after, reset) {
        (Some(after), Some(reset)) if reset > after => Some((reset, "rateLimitReset")),
        (Some(after), _) => Some((after, "retryAfter")),
        (None, Some(reset)) => Some((reset, "rateLimitReset")),
        // GitHub 次级限流没有恢复时间时要求至少等待一分钟；已给时间时不额外延长。
        (None, None) if limited => Some((now.saturating_add(60_000), "fallback")),
        _ => None,
    }
}

pub(super) fn egress_key() -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(&super::client::egress_identity()).unwrap_or_default()))
}

impl Candidate {
    pub(super) fn rate_key(&self, headers: &[(String, String)]) -> String {
        let api = source::is_official_api(&self.original);
        let auth = if self.source == Source::Official && api {
            headers.iter().find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.as_str()).unwrap_or("")
        } else { "" };
        let origin = if self.source == Source::Mirror { self.id.as_str() } else { "https://api.github.com" };
        let proxy = &self.proxy;
        let bytes = if api {
            serde_json::to_vec(&(origin, proxy, auth))
        } else {
            let host = url::Url::parse(&self.original).ok().and_then(|url| url.host_str().map(str::to_string));
            serde_json::to_vec(&(origin, proxy, auth, host, super::source_state::resource(&self.original)))
        }.unwrap_or_default();
        format!("githubRate:{:x}", Sha256::digest(bytes))
    }

    pub(super) fn stored_limit(&self, headers: &[(String, String)], states: &std::collections::HashMap<String, task_state::TaskState>) -> Option<RateLimit> {
        let key = self.rate_key(headers);
        let value = states.get(&key)?.value.as_ref()?;
        let mut limit: RateLimit = serde_json::from_value(value.clone()).ok()?;
        if !limit.egress.is_empty() && limit.egress != self.egress { return None; }
        limit.source = self.id.clone();
        limit.label = self.label();
        limit.resource = super::source_state::resource(&self.original).to_string();
        limit.scope = key;
        if limit.retry_source.is_empty() { limit.retry_source = "legacy".to_string(); }
        Some(limit)
    }

    pub fn rate_limit(&self, headers: &[(String, String)]) -> Option<RateLimit> {
        let states = task_state::read_all().ok()?;
        self.stored_limit(headers, &states).filter(|limit| limit.retry_at > logging::now_ms())
    }

    pub fn cooling_error(&self, headers: &[(String, String)]) -> Option<UpdateError> {
        self.rate_limit(headers).map(|limit| UpdateError::rate_limited(vec![limit]))
    }

    fn save_limit(&self, headers: &[(String, String)], response_headers: &HeaderMap, limited: bool) -> Option<RateLimit> {
        let Some((retry_at, retry_source)) = retry_info(response_headers, limited) else {
            if !limited && self.rate_limit(headers).is_some() {
                if let Err(error) = task_state::store_value(&self.rate_key(headers), Value::Null) {
                    logging::verbose("[Update]", &error);
                }
            }
            return None;
        };
        let key = self.rate_key(headers);
        let mut limit = RateLimit {
            source: self.id.clone(), label: self.label(),
            resource: super::source_state::resource(&self.original).to_string(),
            retry_at, retry_source: retry_source.to_string(), observed_at: logging::now_ms(),
            scope: key.clone(), egress: self.egress.clone(),
        };
        if let Err(error) = task_state::update_value(&key, |value| {
            if let Ok(previous) = serde_json::from_value::<RateLimit>(value.clone()) {
                if previous.egress == limit.egress && previous.observed_at > limit.observed_at {
                    limit = previous;
                }
            }
            *value = serde_json::json!(limit);
        }) {
            logging::verbose("[Update]", &error);
        }
        Some(limit)
    }

    pub(super) async fn inspect_response(&self, headers: &[(String, String)], mut response: reqwest::Response) -> Result<reqwest::Response, UpdateError> {
        let status = response.status().as_u16();
        if response.status().is_success() {
            self.save_limit(headers, response.headers(), false);
            return Ok(response);
        }
        if !matches!(status, 403 | 429) { return Ok(response); }
        let response_headers = response.headers().clone();
        let header_limited = status == 429
            || retry_info(&response_headers, false).is_some()
            || response_headers.get("x-ratelimit-remaining").is_some_and(|value| value == "0");
        let mut bytes = Vec::new();
        if !header_limited {
            while let Some(chunk) = response.chunk().await.map_err(|_| UpdateError::new("读取 GitHub 源错误信息失败"))? {
                let remaining = 16_384_usize.saturating_sub(bytes.len());
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if bytes.len() >= 16_384 { break; }
            }
        }
        let detail = serde_json::from_slice::<Value>(&bytes).ok()
            .and_then(|value| value.get("message").and_then(Value::as_str).map(str::to_string))
            .or_else(|| response_headers.get("content-type").and_then(|value| value.to_str().ok())
                .filter(|value| value.starts_with("text/plain"))
                .map(|_| String::from_utf8_lossy(&bytes).into_owned()))
            .unwrap_or_default();
        let lower = detail.to_ascii_lowercase();
        let body_limited = lower.contains("rate limit") || lower.contains("secondary rate")
            || lower.contains("abuse detection");
        if header_limited || body_limited {
            let limit = self.save_limit(headers, &response_headers, true)
                .ok_or_else(|| UpdateError::new("GitHub 源返回限流，但无法读取恢复时间"))?;
            self.observe("rateLimited", 0, false);
            return Err(UpdateError::rate_limited(vec![limit]));
        }
        let detail: String = detail.chars().filter(|character| !character.is_control()).take(300).collect();
        let suffix = if detail.is_empty() { String::new() } else { format!("：{detail}") };
        self.save_limit(headers, &response_headers, false);
        self.observe("failed", 0, false);
        Err(UpdateError::with_status(403, format!("{}拒绝访问（HTTP 403）{suffix}", self.label())))
    }
}

#[derive(Default)]
pub(super) struct Failures {
    last: Option<UpdateError>,
    limits: Vec<RateLimit>,
}

impl Failures {
    pub fn push(&mut self, error: UpdateError) {
        for limit in &error.rate_limits {
            if let Some(previous) = self.limits.iter_mut().find(|previous| previous.scope == limit.scope) {
                if limit.retry_at > previous.retry_at { *previous = limit.clone(); }
            } else { self.limits.push(limit.clone()); }
        }
        self.last = Some(error);
    }

    pub fn error(self) -> UpdateError {
        if !self.limits.is_empty() { UpdateError::rate_limited(self.limits) }
        else { self.last.unwrap_or_else(|| UpdateError::new("没有可用的 GitHub 源")) }
    }
}
