//! GET 逐跳处理重定向，以保留真实失败目标并限制跨来源凭据转发。

use std::time::Duration;

use futures::StreamExt as _;
use reqwest::{header, StatusCode, Url};

use super::{
    apply_auth, client_with_policy, parse_base_url, redact_url, status_error, transport_error,
    WebDavAuth, TRANSFER_TIMEOUT_SECS,
};

const MAX_REDIRECTS: usize = 10;

fn with_redirect_context(message: String, urls: &[Url]) -> String {
    if urls.len() < 2 {
        return message;
    }
    let route = urls
        .iter()
        .map(|url| redact_url(url.as_str()))
        .collect::<Vec<_>>()
        .join(" → ");
    format!("{message}。已跟随 {} 次重定向（{route}）", urls.len() - 1)
}

fn redirect_error(reason: &str, status: StatusCode, urls: &[Url]) -> String {
    let url = urls.last().map(Url::as_str).unwrap_or("");
    with_redirect_context(
        format!(
            "WebDAV GET 重定向失败（HTTP {status}：{reason}）: {}",
            redact_url(url)
        ),
        urls,
    )
}

pub(super) async fn get_bytes(
    url: &str,
    auth: &WebDavAuth,
    max_bytes: usize,
    user_agent: &str,
) -> Result<Option<Vec<u8>>, String> {
    let http = client_with_policy(reqwest::redirect::Policy::none(), user_agent)?;
    let mut current = parse_base_url(url)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(TRANSFER_TIMEOUT_SECS);
    let mut urls = vec![current.clone()];
    let mut send_auth = true;
    let response = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(with_redirect_context(
                format!(
                    "WebDAV GET失败（请求超时）: {}",
                    redact_url(current.as_str())
                ),
                &urls,
            ));
        }
        let request = http.get(current.clone()).timeout(remaining);
        let request = if send_auth {
            apply_auth(request, auth)
        } else {
            request
        };
        let response = request.send().await.map_err(|error| {
            with_redirect_context(transport_error("GET", current.as_str(), &error), &urls)
        })?;
        let status = response.status();
        if !status.is_redirection() {
            break response;
        }
        if !matches!(
            status,
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        ) {
            return Err(redirect_error("不支持此重定向状态", status, &urls));
        }
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| redirect_error("缺少有效的 Location 地址", status, &urls))?;
        let next = current
            .join(location)
            .map_err(|_| redirect_error("Location 地址无效", status, &urls))?;
        if !matches!(next.scheme(), "http" | "https") {
            return Err(redirect_error("目标只支持 http/https", status, &urls));
        }
        if !next.username().is_empty() || next.password().is_some() {
            return Err(redirect_error(
                "目标地址不能内嵌用户名或密码",
                status,
                &urls,
            ));
        }
        if current.scheme() == "https" && next.scheme() == "http" {
            return Err(redirect_error("拒绝从 HTTPS 降级到 HTTP", status, &urls));
        }
        if urls.contains(&next) {
            return Err(redirect_error("检测到循环跳转", status, &urls));
        }
        if urls.len() > MAX_REDIRECTS {
            return Err(redirect_error("超过最大重定向次数（10 次）", status, &urls));
        }
        // 凭据只属于原始来源；跨来源后即使跳回也不自动重新附加。
        send_auth &= current.origin() == next.origin();
        current = next;
        urls.push(current.clone());
    };

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        let message = if urls.len() > 1 {
            format!(
                "WebDAV GET失败（重定向目标返回 HTTP {status}）: {}",
                redact_url(current.as_str())
            )
        } else {
            status_error("GET", status, current.as_str())
        };
        return Err(with_redirect_context(message, &urls));
    }
    if let Some(length) = response.content_length() {
        if length > max_bytes as u64 {
            return Err(with_redirect_context(
                format!(
                    "远端文件超过上限（{length} 字节 > {max_bytes} 字节）: {}",
                    redact_url(current.as_str()),
                ),
                &urls,
            ));
        }
    }

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            with_redirect_context(transport_error("读取响应", current.as_str(), &error), &urls)
        })?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(with_redirect_context(
                format!(
                    "远端文件超过上限（{max_bytes} 字节）: {}",
                    redact_url(current.as_str()),
                ),
                &urls,
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(bytes))
}
