//! SIWC 管理请求共用出网与体积限制；不自动跟随重定向，不把凭证写进错误。

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::{egress, proxies};
use crate::server::errors::GatewayError;

use super::{diagnostic, TOKEN_URL, USER_AGENT};

pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_AUTH_BYTES: usize = 1024 * 1024;

pub(super) fn client(proxy: Option<&ResolvedProxy>) -> Result<reqwest::Client, GatewayError> {
    if proxy.is_some_and(|proxy| {
        proxy.host.trim().is_empty()
            || proxy.port.is_none_or(|port| port == 0)
            || !matches!(proxy.protocol.as_str(), "http" | "https" | "socks5")
    }) {
        return Err(GatewayError::bad_request(
            "ChatGPT 账号代理无效，未回退直连",
        ));
    }
    // 复用共享的安全出网构造器；构造失败不能悄悄回退直连或允许携令牌跟转。
    let endpoint = proxy
        .map(proxies::chain::endpoint)
        .transpose()
        .map_err(|_| GatewayError::bad_request("ChatGPT 账号代理无法连接，未回退直连"))?;
    egress::resolved_client(endpoint.as_ref(), false, "", &[])
        .map_err(|_| GatewayError::with_status(503, "无法创建 ChatGPT 出站连接"))
}

pub(super) fn proxy_from_options(
    value: Option<&Value>,
) -> Result<Option<ResolvedProxy>, GatewayError> {
    let value = value.unwrap_or(&Value::Null);
    if value.is_null() || value.get("source").and_then(Value::as_str) == Some("direct") {
        return Ok(None);
    }
    ResolvedProxy::from_json(value)
        .map_err(|_| GatewayError::bad_request("ChatGPT 登录代理无效，未回退直连"))
}

pub(super) async fn send(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, GatewayError> {
    request
        .header("User-Agent", USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            GatewayError::with_status(
                502,
                if error.is_timeout() {
                    "ChatGPT 请求超时，请检查代理或稍后重试"
                } else {
                    "ChatGPT 连接失败，请检查代理或网络设置"
                },
            )
        })
}

pub(super) async fn json(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Value, GatewayError> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(GatewayError::with_status(502, "OpenAI 响应超过大小限制"));
    }
    let read = async {
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| GatewayError::with_status(502, "读取 OpenAI 响应失败"))?
        {
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(GatewayError::with_status(502, "OpenAI 响应超过大小限制"));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| GatewayError::with_status(502, "OpenAI 返回了无法解析的响应"))
    };
    tokio::time::timeout(REQUEST_TIMEOUT, read)
        .await
        .map_err(|_| GatewayError::with_status(502, "读取 OpenAI 响应超时"))?
}

#[derive(Deserialize)]
pub(super) struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: String,
    pub expires_in: f64,
    #[serde(default)]
    pub scope: String,
}

pub(super) async fn tokens(
    client: &reqwest::Client,
    form: &[(&str, &str)],
) -> Result<Tokens, GatewayError> {
    let response = send(
        client
            .post(TOKEN_URL)
            .header("Accept", "application/json")
            .form(form),
    )
    .await?;
    if response.status() != reqwest::StatusCode::OK {
        let stage = if form.iter().any(|(key, value)| *key == "grant_type" && *value == "refresh_token") {
            "token_refresh"
        } else {
            "token_exchange"
        };
        let secrets: Vec<_> = form.iter().filter_map(|(key, value)| {
            matches!(*key, "code" | "code_verifier" | "refresh_token" | "client_id" | "redirect_uri").then_some(*value)
        }).collect();
        return Err(diagnostic::failure(response, stage, &secrets).await);
    }
    let body = json(response, MAX_AUTH_BYTES).await?;
    serde_json::from_value(body)
        .map_err(|_| GatewayError::with_status(502, "OpenAI 未返回完整的登录凭证"))
}

pub(super) async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, GatewayError> {
    let response = send(client.get(url).header("Accept", "application/json")).await?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(diagnostic::failure(response, "identity_keys", &[]).await);
    }
    json(response, MAX_AUTH_BYTES).await
}
