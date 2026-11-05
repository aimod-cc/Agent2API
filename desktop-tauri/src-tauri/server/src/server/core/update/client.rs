//! GitHub 请求在源切换时保留选定的出网代理；失效代理不能静默改走直连。

use std::time::Duration;

use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::egress;
use crate::server::core::proxies::{
    resolve_account_proxy, ProxyResolution, ResolvedProxy, CLASH_MIXED_UID,
};
use crate::server::logging;

use super::source::{self, Candidate, Source};
use super::token;
use super::version::UpdateError;
use super::USER_AGENT;

/// GitHub API 要求的头部集合（对应 Node 版 githubHeaders）。
///
/// 令牌来自 `token::effective_token`（「更新设置」里保存的 > 环境变量
/// `GITHUB_TOKEN` / 旧名 `WORKBUDDY_GITHUB_TOKEN`）。配了 token 则限额更高
/// （匿名 60 次/小时，带 token 5000）；两个环境变量名是项目早期（还叫
/// WorkBuddy 网关时）与后来的两套写法，按同一惯例兼容读。
pub fn github_headers() -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "Accept".to_string(),
            "application/vnd.github+json".to_string(),
        ),
        // UA 与下载走同一个常量（mod.rs 的 USER_AGENT）：项目改名时只改一处
        ("User-Agent".to_string(), USER_AGENT.to_string()),
        ("X-GitHub-Api-Version".to_string(), "2022-11-28".to_string()),
    ];
    if let Some(token) = token::effective_token() {
        headers.push(("Authorization".to_string(), format!("Bearer {token}")));
    }
    headers
}

/// 出网候选：`(标签, 代理配置)`；`proxy` 为 None 表示直连。
///
/// 对应 Node 版 resolveEgressCandidates —— Clash 配置不可读时只有一个候选，
/// 行为与纯直连一致（这里不吞掉整条链路：探测失败只在 verbose 里留痕）。
pub(super) fn resolve_egress_candidates() -> Result<Vec<(String, Option<ResolvedProxy>)>, String> {
    // 设置里指定了更新出口：只用它（理由见模块头「候选顺序」）
    if let Some(configured) = config::update_proxy() {
        match resolve_account_proxy(Some(&configured)) {
            Some(ProxyResolution::Resolved(proxy)) if proxy.port.is_some() => {
                let label = if proxy.label.is_empty() {
                    "指定的更新出口".to_string()
                } else {
                    proxy.label.clone()
                };
                return Ok(vec![(label, Some(proxy))]);
            }
            Some(ProxyResolution::Failed(reason)) => {
                return Err(format!("指定的更新出口不可用（{reason}）"));
            }
            _ => return Err("指定的更新出口没有可用端口".to_string()),
        }
    }
    let mut candidates = vec![("直连".to_string(), None)];
    let resolution = resolve_account_proxy(Some(&json!({
        "source": "clash",
        "listenerUid": CLASH_MIXED_UID,
    })));
    match resolution {
        Some(resolution) => {
            if let Some(proxy) = resolution.resolved() {
                if proxy.port.is_some() {
                    let label = if proxy.label.is_empty() {
                        "Clash 混合端口".to_string()
                    } else {
                        proxy.label.clone()
                    };
                    candidates.push((label, Some(proxy.clone())));
                }
            } else if let Some(error) = resolution.error() {
                // Node 在这条分支上静默（try/catch 吞掉），这里留 verbose 便于排障
                logging::verbose(
                    "[Update]",
                    &format!("Clash 出口不可用（{error}），仅用直连"),
                );
            }
        }
        None => {}
    }
    Ok(candidates)
}

pub(super) fn egress_identity() -> Value {
    match resolve_egress_candidates() {
        Ok(candidates) => json!(candidates.into_iter().map(|(_, proxy)| proxy.map(|proxy| {
            json!([proxy.source, proxy.protocol, proxy.host, proxy.port, proxy.username, proxy.password])
        })).collect::<Vec<_>>()),
        Err(message) => json!({ "error": message }),
    }
}

#[derive(Clone)]
pub struct JsonResponse {
    pub value: Value,
    pub status: reqwest::StatusCode,
    pub headers: reqwest::header::HeaderMap,
    pub official: bool,
}

pub async fn fetch_json(
    url: &str,
    headers: &[(String, String)],
    timeout_ms: u64,
    validate: impl Fn(&Value) -> bool + Sync,
) -> Result<JsonResponse, UpdateError> {
    super::selection::fetch_json(url, headers, timeout_ms, &validate).await
}

pub async fn fetch_official(
    url: &str,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
) -> Result<reqwest::Response, UpdateError> {
    fetch_github(url, headers, timeout_ms, false)
        .await
        .map_err(UpdateError::new)
}

async fn fetch_github(
    url: &str,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
    allow_mirror: bool,
) -> Result<reqwest::Response, String> {
    let mut candidates = source::candidates(url, allow_mirror);
    source::ensure_available(&candidates, headers)?;
    if allow_mirror && candidates.len() > 1 && config::github_access().mode == config::GithubAccessMode::Auto {
        candidates = super::probe::rank(url, true, headers).await?;
    }
    let mut last_error = String::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if let Some(message) = candidate.cooling_message(headers) {
            return Err(message);
        }
        let started = std::time::Instant::now();
        let response = match fetch_candidate(candidate, headers, timeout_ms).await {
            Ok(response) => response,
            Err(error) => {
                candidate.observe(source::failed_state(&error), started.elapsed().as_millis() as u64, false);
                last_error = error;
                continue;
            }
        };
        if candidate.is_rate_limited(&response) {
            candidate.observe("rateLimited", started.elapsed().as_millis() as u64, false);
            return Ok(response);
        }
        if (source::should_switch(candidate, response.status())
            || (candidate.source == Source::Mirror && response.status().as_u16() == 403))
            && index + 1 < candidates.len()
        {
            candidate.observe("failed", started.elapsed().as_millis() as u64, false);
            last_error = format!(
                "{}返回 HTTP {}",
                candidate.label(),
                response.status().as_u16()
            );
            continue;
        }
        if response.status().is_success() {
            candidate.observe("ok", started.elapsed().as_millis() as u64, allow_mirror);
        }
        return Ok(response);
    }
    Err(last_error)
}

pub async fn fetch_candidate(
    candidate: &Candidate,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
) -> Result<reqwest::Response, String> {
    if let Some(message) = candidate.cooling_message(headers) {
        return Err(message);
    }
    let mut last_error = String::new();
    for (label, proxy) in resolve_egress_candidates()? {
        let client = egress::client_for_github(proxy.as_ref())?;
        let request = async {
            let mut current = url::Url::parse(&candidate.url).map_err(|error| error.to_string())?;
            for _ in 0..10 {
                let mut builder = client.get(current.clone());
                if let Some(ms) = timeout_ms {
                    builder = builder.timeout(Duration::from_millis(ms));
                }
                let attempt_ms = timeout_ms.map(|ms| ms.min(8_000)).unwrap_or(8_000);
                for (name, value) in headers {
                    if name.eq_ignore_ascii_case("authorization")
                        && (candidate.source != Source::Official
                            || !source::is_official_api(current.as_str()))
                    {
                        continue;
                    }
                    builder = builder.header(name.as_str(), value.as_str());
                }
                let response =
                    tokio::time::timeout(Duration::from_millis(attempt_ms), builder.send())
                        .await
                        .map_err(|_| "连接 GitHub 源超时".to_string())?
                        .map_err(|error| egress::describe_error_detail(&error))?;
                if response.status() == reqwest::StatusCode::NOT_MODIFIED {
                    return Ok(response);
                }
                if !response.status().is_redirection() {
                    candidate.record_rate_limit(headers, &response);
                    return Ok(response);
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| "重定向缺少 Location".to_string())?;
                let next = current.join(location).map_err(|error| error.to_string())?;
                if !candidate.allows_redirect(&next) {
                    return Err("GitHub 源重定向到未允许的地址，已拒绝访问".to_string());
                }
                current = next;
            }
            Err("GitHub 源重定向次数过多".to_string())
        };
        let outcome = if let Some(ms) = timeout_ms {
            tokio::time::timeout(Duration::from_millis(ms.min(8_000)), request)
                .await
                .unwrap_or_else(|_| Err("连接 GitHub 源超时".to_string()))
        } else {
            tokio::time::timeout(Duration::from_secs(12), request)
                .await
                .unwrap_or_else(|_| Err("连接 GitHub 源超时".to_string()))
        };
        match outcome {
            Ok(response) => return Ok(response),
            Err(error) => {
                logging::verbose(
                    "[Update]",
                    &format!("经 {label} 访问{}失败：{error}", candidate.label()),
                );
                last_error = error;
            }
        }
    }
    Err(last_error)
}

/// 非 GitHub 公网端点只复用出网代理，不改写地址或发送 GitHub 凭证。
pub async fn fetch_public(
    url: &str,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
) -> Result<reqwest::Response, String> {
    if source::is_resource(url) {
        return fetch_github(url, headers, timeout_ms, true).await;
    }
    let host = url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
    fetch_via_candidates("[Models]", url, headers, timeout_ms)
        .await
        .map_err(|detail| format!("无法连接 {host}：{detail}"))
}

/// 候选出口依次尝试的共用实现：返回第一个连上的响应，全部失败时把最后一条
/// 错误链作为 `Err(String)` 交给调用方去拼自己的文案（见上面两个函数）。
async fn fetch_via_candidates(
    tag: &str,
    url: &str,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
) -> Result<reqwest::Response, String> {
    let candidates = resolve_egress_candidates()?;
    let multiple = candidates.len() > 1;
    let mut last_error: Option<reqwest::Error> = None;
    for (label, proxy) in &candidates {
        let client = egress::client_for(proxy.as_ref());
        let mut builder = client.get(url);
        // 请求级总超时只给 API 探测用（响应是小 JSON）。reqwest 0.12 的
        // `.timeout()` 覆盖到 body 读完为止 —— 下载传 None，安装包几十上百 MB
        // 不可能 30 秒内下完；卡死场景由 egress 客户端的 read_timeout 兜底
        if let Some(timeout_ms) = timeout_ms {
            builder = builder.timeout(Duration::from_millis(timeout_ms));
        }
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("authorization") {
                continue;
            }
            builder = builder.header(name.as_str(), value.as_str());
        }
        match builder.send().await {
            Ok(response) => {
                if multiple && proxy.is_some() {
                    logging::verbose(tag, &format!("经 {label} 访问成功"));
                }
                return Ok(response);
            }
            Err(error) => {
                logging::verbose(
                    tag,
                    &format!(
                        "经 {label} 访问失败: {}",
                        egress::describe_error_detail(&error)
                    ),
                );
                last_error = Some(error);
            }
        }
    }
    Err(match &last_error {
        Some(error) => egress::describe_error_detail(error),
        None => "未知错误".to_string(),
    })
}

/// 出网候选的展示形态（供日志/排障；不含凭证）。
/// 对应 Node 版 workbuddy-update.mjs 内部的 `resolveEgressCandidates()` 结果形态
/// （Node 侧为内部细节，这里作为排障口暴露，保留）。
#[allow(dead_code)]
pub fn describe_candidates() -> Value {
    let candidates = match resolve_egress_candidates() {
        Ok(candidates) => candidates,
        Err(error) => return json!({ "error": error }),
    };
    let list: Vec<Value> = candidates
        .into_iter()
        .map(|(label, proxy)| {
            json!({
                "label": label,
                "proxy": egress::describe_public(proxy.as_ref()),
            })
        })
        .collect();
    Value::Array(list)
}
