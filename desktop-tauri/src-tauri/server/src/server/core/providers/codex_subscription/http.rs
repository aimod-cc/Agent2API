//! Magpie Codex 管理请求的公共传输：只使用本家固定地址与账号选定的出口。
//! 客户端协议版本取参考项目 codex_models.go；不扫描或执行用户安装的 Codex。

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::http::HeaderMap;
use futures::StreamExt;
use serde_json::Value;

use crate::server::core::{egress, proxies::ResolvedProxy};
use crate::server::errors::GatewayError;

pub(super) const ISSUER: &str = "https://auth.openai.com";
pub(super) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub(super) const BASE: &str = "https://chatgpt.com/backend-api/codex";
pub(super) const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub(super) const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const REFERENCE_VERSION: &str = "0.171.0";
const TIMEOUT: Duration = Duration::from_secs(25);

static SEEN_VERSION: OnceLock<Mutex<String>> = OnceLock::new();

pub(super) struct Reply {
    pub status: u16,
    pub body: Value,
}

pub(super) fn client(proxy: Option<&ResolvedProxy>) -> Result<reqwest::Client, GatewayError> {
    // 公共 resolved_client 不跟跳转、失败返回错误，不使用会静默回退的 client_for。
    // 空解析表只是普通 DNS，不覆盖任何上游地址。
    egress::resolved_client(proxy, false, "", &[]).map_err(|_| {
        GatewayError::with_status(502, "Codex 账号出口无法建立，未回退直连")
            .with_code("account_proxy_unavailable")
    })
}

pub(super) fn version() -> String {
    SEEN_VERSION
        .get_or_init(|| Mutex::new(REFERENCE_VERSION.to_string()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn numbers(version: &str) -> Option<Vec<u32>> {
    if version.is_empty() || version.len() > 64 {
        return None;
    }
    let core = version.split('-').next()?;
    let parts = core
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (parts.len() >= 2
        && version
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'-')))
    .then_some(parts)
}

pub(super) fn observe_client(headers: &HeaderMap) {
    let ua = headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let originator = headers
        .get("originator")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !originator.starts_with("codex") && !ua.to_ascii_lowercase().starts_with("codex") {
        return;
    }
    let candidate = headers
        .get("version")
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            ua.split_once('/')
                .and_then(|(_, value)| value.split_whitespace().next())
        });
    let Some(candidate) = candidate.filter(|candidate| numbers(candidate).is_some()) else {
        return;
    };
    let mut seen = SEEN_VERSION
        .get_or_init(|| Mutex::new(REFERENCE_VERSION.to_string()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if numbers(candidate) > numbers(&seen) {
        *seen = candidate.to_string();
    }
}

pub(super) fn client_headers() -> Vec<(String, String)> {
    vec![
        ("originator".into(), "codex_cli_rs".into()),
        // 沿用参考的协议标识，同时明确标明网关，不捏造桌面安装版本或终端。
        (
            "User-Agent".into(),
            format!(
                "codex_cli_rs/{} ({}; {}) Agent2API/{}",
                version(),
                std::env::consts::OS,
                std::env::consts::ARCH,
                env!("CARGO_PKG_VERSION")
            ),
        ),
    ]
}

pub(super) fn account_headers(access: &str, account_id: &str) -> Vec<(String, String)> {
    let mut headers = client_headers();
    headers.push(("Authorization".into(), format!("Bearer {access}")));
    headers.push(("ChatGPT-Account-Id".into(), account_id.to_string()));
    headers
}

pub(super) fn with_headers(
    mut request: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    for (key, value) in headers {
        request = request.header(key, value);
    }
    request.timeout(TIMEOUT)
}

pub(super) async fn request(
    request: reqwest::RequestBuilder,
    limit: usize,
    action: &str,
) -> Result<Reply, GatewayError> {
    let read = async {
        let response = request
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|error| network_error(action, &error))?;
        let status = response.status().as_u16();
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(GatewayError::with_status(
                502,
                format!("Codex {action}响应过大"),
            ));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| network_error(action, &error))?;
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(GatewayError::with_status(
                    502,
                    format!("Codex {action}响应过大"),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = match serde_json::from_slice::<Value>(&bytes) {
            Ok(body) => body,
            Err(_) if !(200..300).contains(&status) => Value::Null,
            Err(_) => {
                return Err(GatewayError::with_status(
                    502,
                    format!("Codex {action}没有返回有效数据"),
                ))
            }
        };
        Ok(Reply { status, body })
    };
    tokio::time::timeout(TIMEOUT, read).await.map_err(|_| {
        GatewayError::with_status(504, format!("Codex {action}超时，请检查账号出口"))
    })?
}

fn network_error(action: &str, error: &reqwest::Error) -> GatewayError {
    // reqwest 错误可能含 OAuth 查询参数或代理口令，管理接口不回显整条错误链。
    GatewayError::with_status(
        if error.is_timeout() { 504 } else { 502 },
        format!(
            "Codex {action}{}，请检查账号出口",
            if error.is_timeout() {
                "超时"
            } else {
                "连接失败"
            }
        ),
    )
}

pub(super) fn token_error_code(body: &Value) -> Option<String> {
    body.get("error")
        .and_then(Value::as_str)
        .or_else(|| body.pointer("/error/code").and_then(Value::as_str))
        .or_else(|| body.get("code").and_then(Value::as_str))
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 80
                && value
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
        })
        .map(str::to_string)
}

pub(super) fn code(body: &Value) -> Option<String> {
    token_error_code(body).or_else(|| {
        body.pointer("/error/type")
            .and_then(Value::as_str)
            .or_else(|| body.pointer("/detail/code").and_then(Value::as_str))
            .or_else(|| body.pointer("/detail/type").and_then(Value::as_str))
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 80
                    && value
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
            })
            .map(str::to_string)
    })
}

pub(super) fn response_error(action: &str, reply: &Reply) -> GatewayError {
    let code = code(&reply.body);
    let suffix = code
        .as_ref()
        .map(|code| format!("（{code}）"))
        .unwrap_or_default();
    let mut error = GatewayError::with_status(
        if reply.status < 400 {
            502
        } else {
            i32::from(reply.status)
        },
        format!("Codex {action}失败，HTTP {}{suffix}", reply.status),
    );
    if let Some(code) = code {
        error = error.with_code(code);
    }
    error
}
