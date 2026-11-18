//! 固定两个Trae POST；同HTTP的发送和完整有界读取只由独立worker拥有。
//! 取消只封下一次准入，已发请求观察到原绝对截止，绝不丢等待再重发。

use std::time::Duration;

use reqwest::Request;
use serde_json::json;
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::{context::IDE_VERSION, response::SecretJson, ExchangeInput};
use crate::server::errors::GatewayError;

pub(super) const EXCHANGE_URL: &str = "https://api.trae.cn/trae/api/v3/oauth/ExchangeToken";
pub(super) const PROFILE_URL: &str = "https://api.trae.cn/cloudide/api/v3/trae/GetUserInfo";
const BODY_LIMIT: usize = 256 * 1024;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Exchange,
    Profile,
}

impl Kind {
    fn url(self) -> &'static str {
        match self {
            Self::Exchange => EXCHANGE_URL,
            Self::Profile => PROFILE_URL,
        }
    }
    pub(super) fn budget(self) -> Duration {
        match self {
            Self::Exchange => Duration::from_secs(25),
            Self::Profile => Duration::from_secs(15),
        }
    }
    pub(super) fn error(self) -> GatewayError {
        match self {
            Self::Exchange => super::stopped_error(),
            Self::Profile => super::identity_error(),
        }
    }
}

/// 只组装请求，尚未发送；唯一地址、headers和body来自原宿主上下文。
pub(super) fn build(
    input: &ExchangeInput,
    kind: Kind,
    token: Option<&str>,
) -> Result<Request, GatewayError> {
    let context = &input.context.protocol;
    let body = SecretJson::new(match kind {
        Kind::Exchange => {
            if token.is_some()
                || context.verifier.len() != 64
                || input.code.is_empty()
                || context.device_public.is_empty()
                || context.device_private.is_empty()
            {
                return Err(kind.error());
            }
            json!({"ClientID": context.variant.client_id(), "AuthCode": input.code.as_str(),
            "CodeVerifier": context.verifier.as_str(), "IDEVersion": IDE_VERSION, "DeviceInfo": {
                "DeviceID": context.device_id, "MachineID": context.machine_id,
                "PlatformCode": context.variant.platform_code(), "DeviceType": "PC",
                "DeviceName": "", "DeviceModel": "", "ClientVersion": IDE_VERSION,
                "DevicePublicKey": context.device_public, "DeviceBrand": "", "DeviceCPU": "",
                "OSInfo": context.platform, "OSVersion": ""
            }})
        }
        Kind::Profile => {
            if token.is_none_or(|token| !super::response::valid_token(token)) {
                return Err(kind.error());
            }
            json!({"ReqSource": "IDE", "IDEVersion": IDE_VERSION})
        }
    });
    let bytes = Zeroizing::new(serde_json::to_vec(body.value()).map_err(|_| kind.error())?);
    input
        .context
        .client
        .client()
        .map_err(|_| kind.error())?
        .post(kind.url())
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("user-agent", format!("Trae/{IDE_VERSION}"))
        .header("x-cloudide-token", token.unwrap_or(""))
        .body(bytes.to_vec())
        .build()
        .map_err(|_| kind.error())
}

/// 必须已赢同源send CAS。不能在此select Session/Call取消，也不按404或业务码重发。
pub(super) async fn observe(
    input: &ExchangeInput,
    request: Request,
    kind: Kind,
    deadline: Instant,
) -> Result<SecretJson, GatewayError> {
    let operation = async {
        let mut response = input
            .context
            .client
            .client()
            .map_err(|_| kind.error())?
            .execute(request)
            .await
            .map_err(|_| kind.error())?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > BODY_LIMIT as u64)
        {
            return Err(kind.error());
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| kind.error())? {
            if bytes.len().saturating_add(chunk.len()) > BODY_LIMIT {
                return Err(kind.error());
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            // 不暴露URL、授权码、token或上游原文，也不把401/404改成重兑许可。
            return Err(kind.error());
        }
        super::response::decode(&bytes, kind)
    };
    tokio::time::timeout_at(deadline, operation)
        .await
        .map_err(|_| kind.error())?
}
