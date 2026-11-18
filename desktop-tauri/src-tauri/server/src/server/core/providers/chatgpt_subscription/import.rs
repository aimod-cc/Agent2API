//! 用户显式提交的 SIWC 凭证导入。不扫描客户端文件，不读取 Codex 登录态。

use agent2api_provider_protocol::{NormalizeInput, NormalizeSource, NormalizedAccount};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;

use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

use super::{account, credentials, ID};

const MAX_IMPORT_BYTES: usize = 1024 * 1024;

pub(super) async fn normalize(input: NormalizeInput) -> Result<NormalizedAccount, GatewayError> {
    if input.provider != ID {
        return Err(GatewayError::bad_request("ChatGPT 导入的提供商不一致"));
    }
    let payload = match input.source {
        NormalizeSource::Manual { payload } => {
            if payload.get("methodIndex").is_some() {
                return Err(GatewayError::bad_request(
                    "请通过 ChatGPT 授权登录完成添加，不能把登录表单当成令牌",
                ));
            }
            match payload.get("credentialJson").and_then(Value::as_str) {
                Some(text) => parse(text.as_bytes())?,
                None => payload,
            }
        }
        NormalizeSource::FileText { content, .. } => parse(content.as_bytes())?,
        NormalizeSource::FileBase64 { content, .. } => {
            if content.len() > MAX_IMPORT_BYTES * 2 {
                return Err(GatewayError::bad_request("ChatGPT 凭证文件过大"));
            }
            let bytes = STANDARD
                .decode(content.trim())
                .map_err(|_| GatewayError::bad_request("ChatGPT 凭证文件编码无效"))?;
            parse(&bytes)?
        }
        NormalizeSource::Desktop => {
            return Err(GatewayError::bad_request(
                "ChatGPT 不读取其他客户端的登录态，请使用授权登录或显式导入",
            ))
        }
    };
    if payload
        .get("provider")
        .and_then(Value::as_str)
        .is_some_and(|provider| provider != ID)
    {
        return Err(GatewayError::bad_request(
            "这不是 ChatGPT SIWC 路线的凭证，不能导入其他提供商账号",
        ));
    }
    let secret = payload
        .get("pluginSecret")
        .or_else(|| payload.get("secret"))
        .unwrap_or(&payload)
        .clone();
    let proxy = input.proxy.map(|proxy| ResolvedProxy {
        source: "custom".to_string(),
        protocol: proxy.protocol,
        host: proxy.host,
        port: proxy.port,
        username: proxy.username,
        password: proxy.password,
        label: String::new(),
        non_direct: false,
    });
    let credentials = credentials::validate_import(secret, proxy.as_ref()).await?;
    let mut normalized = credentials.normalized()?;
    if let Some(name) = input.name.filter(|name| !name.trim().is_empty()) {
        normalized.public["name"] = Value::String(account::safe_text(&name));
    }
    Ok(normalized)
}

fn parse(bytes: &[u8]) -> Result<Value, GatewayError> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(GatewayError::bad_request("ChatGPT 凭证文件过大"));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| GatewayError::bad_request("ChatGPT 凭证必须是完整的 SIWC JSON"))
}
