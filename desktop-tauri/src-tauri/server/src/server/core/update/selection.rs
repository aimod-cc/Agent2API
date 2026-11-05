use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::{stream::FuturesUnordered, StreamExt};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::server::config::GithubAccessMode;
use crate::server::logging;

use super::client::{self, JsonResponse};
use super::source::{self, Candidate, Source};
use super::{source_state, UpdateError};

const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
const COALESCE_MS: i64 = 2_000;

struct Cached {
    at: i64,
    response: JsonResponse,
}

fn recent() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) async fn fetch_json(
    url: &str,
    headers: &[(String, String)],
    timeout_ms: u64,
    validate: &(impl Fn(&Value) -> bool + Sync),
) -> Result<JsonResponse, UpdateError> {
    let mut candidates = source::candidates(url, true);
    source::ensure_available(&candidates, headers).map_err(UpdateError::new)?;
    let context = candidates[0].context.clone();
    let cache_key = format!("{:x}", Sha256::digest(serde_json::to_vec(&(url, headers, &context)).unwrap_or_default()));
    let gate = source_state::gate(&cache_key);
    let _guard = gate.lock().await;
    source::ensure_available(&candidates, headers).map_err(UpdateError::new)?;
    if let Some(response) = recent().lock().ok().and_then(|cache| {
        cache.get(&cache_key).filter(|cached| logging::now_ms() - cached.at < COALESCE_MS)
            .filter(|cached| validate(&cached.response.value)).map(|cached| cached.response.clone())
    }) {
        return Ok(response);
    }
    let automatic = candidates[0].mode == GithubAccessMode::Auto;
    if automatic { source_state::sort(&mut candidates); }
    let total = Duration::from_millis(timeout_ms.max(1));
    let response = tokio::time::timeout(total, select(&candidates, headers, timeout_ms, validate))
        .await.map_err(|_| UpdateError::new("查询 GitHub 更新源超时"))??;
    if response.status.is_success() {
        if let Ok(mut cache) = recent().lock() {
            let now = logging::now_ms();
            cache.retain(|_, cached| now - cached.at < COALESCE_MS);
            if cache.len() < 16 {
                cache.insert(cache_key, Cached { at: now, response: response.clone() });
            }
        }
    }
    Ok(response)
}

async fn select(
    candidates: &[Candidate],
    headers: &[(String, String)],
    timeout_ms: u64,
    validate: &(impl Fn(&Value) -> bool + Sync),
) -> Result<JsonResponse, UpdateError> {
    let automatic = candidates[0].mode == GithubAccessMode::Auto;
    let mut last_error = UpdateError::new("没有可用的 GitHub 源");
    if !automatic || candidates.len() == 1 {
        let attempt_ms = (timeout_ms / candidates.len() as u64).max(1);
        for candidate in candidates {
            source::ensure_available(candidates, headers).map_err(UpdateError::new)?;
            match attempt(candidate, headers, attempt_ms, validate).await {
                Ok(response) => { candidate.remember(); return Ok(response); }
                Err(Failure::Terminal(response)) => return Ok(response),
                Err(Failure::Retry(error)) if error.message.contains("限流") => return Err(error),
                Err(Failure::Retry(error)) => last_error = error,
            }
        }
        return Err(last_error);
    }
    let selected = remembered_candidate(candidates);
    let mut remaining = candidates.to_vec();
    if let Some(index) = selected {
        let candidate = remaining.remove(index);
        match attempt(&candidate, headers, (timeout_ms / 2).clamp(1, 12_000), validate).await {
            Ok(response) => { candidate.remember(); return Ok(response); }
            Err(Failure::Terminal(response)) => return Ok(response),
            Err(Failure::Retry(error)) if error.message.contains("限流") => return Err(error),
            Err(Failure::Retry(error)) => last_error = error,
        }
    }
    let mut pending = FuturesUnordered::new();
    for candidate in &remaining {
        if source_state::state(candidate).is_some_and(|value| value.state != "ok") { continue; }
        pending.push(async move { (candidate, attempt(candidate, headers, timeout_ms.min(12_000), validate).await) });
    }
    while let Some((candidate, outcome)) = pending.next().await {
        match outcome {
            Ok(response) => {
                candidate.remember();
                return Ok(response);
            }
            Err(Failure::Terminal(response)) => return Ok(response),
            Err(Failure::Retry(error)) if error.message.contains("限流") => return Err(error),
            Err(Failure::Retry(error)) => last_error = error,
        }
    }
    Err(last_error)
}

fn remembered_candidate(candidates: &[Candidate]) -> Option<usize> {
    candidates.iter().position(|candidate| {
        source_state::state(candidate).is_some_and(|value| value.state == "ok" && value.selected)
    })
}

enum Failure {
    Retry(UpdateError),
    Terminal(JsonResponse),
}

async fn attempt(
    candidate: &Candidate,
    headers: &[(String, String)],
    timeout_ms: u64,
    validate: &(impl Fn(&Value) -> bool + Sync),
) -> Result<JsonResponse, Failure> {
    let started = Instant::now();
    let observed_at = logging::now_ms();
    let result = tokio::time::timeout(Duration::from_millis(timeout_ms.max(1)), async {
        let response = client::fetch_candidate(candidate, headers, Some(timeout_ms)).await
            .map_err(|message| Failure::Retry(UpdateError::new(message)))?;
        let status = response.status();
        let response_headers = response.headers().clone();
        let official = candidate.source == Source::Official && source::is_resource(&candidate.original);
        let empty = || JsonResponse { value: Value::Null, status, headers: response_headers.clone(), official };
        if candidate.is_rate_limited(&response) {
            candidate.observe("rateLimited", started.elapsed().as_millis() as u64, false);
            return Err(Failure::Terminal(empty()));
        }
        if !status.is_success() {
            if candidate.source == Source::Official && !source::should_switch(candidate, status) {
                candidate.observe("failed", started.elapsed().as_millis() as u64, false);
                return Err(Failure::Terminal(empty()));
            }
            let state = if matches!(status.as_u16(), 400 | 403 | 404 | 405) { "unsupported" } else { "failed" };
            candidate.observe(state, started.elapsed().as_millis() as u64, false);
            return Err(Failure::Retry(UpdateError::new(format!("{}返回 HTTP {}", candidate.label(), status.as_u16()))));
        }
        if response.content_length().is_some_and(|size| size > MAX_JSON_BYTES as u64) {
            return Err(Failure::Retry(UpdateError::new("更新源返回的内容过大")));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| Failure::Retry(UpdateError::new("读取更新源内容失败")))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_JSON_BYTES {
                return Err(Failure::Retry(UpdateError::new("更新源返回的内容过大")));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Failure::Retry(UpdateError::new(format!("{}返回的 JSON 无效", candidate.label()))))?;
        if !validate(&value) {
            return Err(Failure::Retry(UpdateError::new(format!("{}返回的数据格式无效", candidate.label()))));
        }
        candidate.observe("ok", started.elapsed().as_millis() as u64, false);
        Ok(JsonResponse { value, status, headers: response_headers, official })
    }).await.unwrap_or_else(|_| Err(Failure::Retry(UpdateError::new("查询 GitHub 更新源超时"))));
    if let Err(Failure::Retry(error)) = &result {
        if source_state::state(candidate).is_none_or(|state| state.checked_at < observed_at || state.state == "ok") {
            candidate.observe(source::failed_state(&error.message), started.elapsed().as_millis() as u64, false);
        }
    }
    result
}
