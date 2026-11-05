use std::time::{Duration, Instant};

use futures::{stream::FuturesUnordered, StreamExt};

use crate::server::config::GithubAccessMode;

use super::source::{self, Candidate};
use super::{client, source_state};

const SAMPLE_BYTES: usize = 64 * 1024;
const PROBE_TIMEOUT_MS: u64 = 6_000;

pub(super) async fn rank(
    url: &str,
    allow_mirror: bool,
    headers: &[(String, String)],
) -> Result<Vec<Candidate>, String> {
    let mut candidates = source::candidates(url, allow_mirror);
    source::ensure_available(&candidates, headers)?;
    if candidates.len() < 2 || candidates[0].mode != GithubAccessMode::Auto {
        return Ok(candidates);
    }
    let gate = source_state::gate(&format!("probe:{}", candidates[0].key));
    let _guard = gate.lock().await;
    source::ensure_available(&candidates, headers)?;
    source_state::sort(&mut candidates);
    if source_state::remembered(&candidates) { return Ok(candidates); }
    let mut pending = FuturesUnordered::new();
    for candidate in &candidates {
        if source_state::state(candidate).is_some_and(|value| value.state != "ok") { continue; }
        pending.push(async move { (candidate, sample(candidate, headers).await) });
    }
    let mut winner = None;
    while let Some((candidate, outcome)) = pending.next().await {
        match outcome {
            Ok(()) => {
                source::ensure_available(&candidates, headers)?;
                candidate.remember();
                winner = Some(candidate.id.clone());
                break;
            }
            Err(error) if error.contains("限流") => return Err(error),
            Err(_) => {}
        }
    }
    drop(pending);
    if let Some(winner) = winner {
        source_state::sort(&mut candidates);
        candidates.sort_by_key(|candidate| u8::from(candidate.id != winner));
    }
    // 轻量探测超时不等于文件不可下载，正式请求仍有自己的超时和校验。
    Ok(candidates)
}

async fn sample(candidate: &Candidate, headers: &[(String, String)]) -> Result<(), String> {
    let started = Instant::now();
    let mut headers = headers.to_vec();
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("range") && !name.eq_ignore_ascii_case("accept-encoding"));
    headers.push(("Range".to_string(), format!("bytes=0-{}", SAMPLE_BYTES - 1)));
    headers.push(("Accept-Encoding".to_string(), "identity".to_string()));
    let result = tokio::time::timeout(Duration::from_millis(PROBE_TIMEOUT_MS), async {
        let response = client::fetch_candidate(candidate, &headers, Some(PROBE_TIMEOUT_MS)).await?;
        let status = response.status();
        if candidate.is_rate_limited(&response) {
            return Err(source::GithubSettings::Update.rate_limit_message(
                source::retry_deadline(response.headers(), status.as_u16()),
            ));
        }
        if !matches!(status.as_u16(), 200 | 206) {
            return Err(format!("{}返回 HTTP {}", candidate.label(), status.as_u16()));
        }
        let partial_size = if status.as_u16() == 206 {
            let range = response.headers().get("content-range").and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("bytes 0-"))
                .and_then(|value| value.split_once('/'))
                .and_then(|(end, total)| end.parse::<usize>().ok().map(|end| (end, total)));
            let Some((end, total)) = range else { return Err("下载源返回的分段内容无效".to_string()); };
            if end >= SAMPLE_BYTES || (total != "*" && !total.parse::<u64>().is_ok_and(|total| total > end as u64)) {
                return Err("下载源返回的分段内容无效".to_string());
            }
            Some(end + 1)
        } else { None };
        let content_type = response.headers().get("content-type").and_then(|value| value.to_str().ok()).unwrap_or("");
        if content_type.contains("text/html") {
            return Err("下载源返回了网页而不是文件内容".to_string());
        }
        let expected = partial_size.or_else(|| response.content_length().and_then(|size| usize::try_from(size).ok()))
            .map(|size| size.min(SAMPLE_BYTES));
        let mut stream = response.bytes_stream();
        let mut received = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "读取下载源内容失败".to_string())?;
            if received == 0 {
                let prefix = String::from_utf8_lossy(&chunk[..chunk.len().min(256)]).trim_start().to_ascii_lowercase();
                if prefix.starts_with("<!doctype html") || prefix.starts_with("<html") {
                    return Err("下载源返回了网页而不是文件内容".to_string());
                }
            }
            received += chunk.len().min(SAMPLE_BYTES - received);
            if received >= SAMPLE_BYTES { break; }
        }
        if received == 0 || expected.is_some_and(|expected| received != expected) {
            return Err("下载源返回的文件内容不完整".to_string());
        }
        Ok(())
    }).await.unwrap_or_else(|_| Err("探测下载源超时".to_string()));
    let elapsed = started.elapsed().as_millis() as u64;
    match &result {
        Ok(()) => candidate.observe("ok", elapsed, false),
        Err(error) => candidate.observe(source::failed_state(error), elapsed, false),
    }
    result
}
