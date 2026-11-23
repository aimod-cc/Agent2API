use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::errors::GatewayError;

pub(crate) const ALICOM4_CODAR_CAPTCHA_ID: &str = "bb7f810ecc9f6d21dcf72bcae120c8c5";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Challenge {
    Geetest { gt: String, challenge: String, success: bool, new_captcha: bool },
    Alicom4 { #[serde(rename = "captchaId")] captcha_id: String },
    External { url: String },
}

pub(crate) fn check_alicom4_response(value: &Value) -> bool {
    let Some(reply) = value.as_object() else { return false; };
    let keys = ["lot_number", "captcha_output", "pass_token", "gen_time"];
    reply.len() == keys.len()
        && reply.iter().all(|(key, value)| {
            keys.contains(&key.as_str())
                && value.as_str().is_some_and(|text| {
                    !text.is_empty() && text.len() <= 8192 && !text.chars().any(char::is_control)
                })
        })
}

impl Challenge {
    pub fn validate(&self) -> Result<(), GatewayError> {
        let valid = match self {
            Self::Geetest { gt, challenge, .. } => !gt.is_empty() && gt.len() <= 128
                && !challenge.is_empty() && challenge.len() <= 256,
            Self::Alicom4 { captcha_id } => captcha_id == ALICOM4_CODAR_CAPTCHA_ID,
            Self::External { url } => url.len() <= 8192 && !url.chars().any(|ch| ch.is_control() || ch.is_whitespace())
                && url::Url::parse(url).is_ok_and(|url|
                url.scheme() == "https" && url.host_str().is_some() && url.username().is_empty()
                    && url.password().is_none()),
        };
        if valid { Ok(()) } else { Err(GatewayError::with_status(502, "插件返回的人机验证信息无效")) }
    }

    pub fn check(&self, value: &Value) -> Result<(), GatewayError> {
        let valid = match self {
            Self::External { .. } => value.is_null(),
            Self::Alicom4 { .. } => check_alicom4_response(value),
            Self::Geetest { challenge, .. } => value["geetest_challenge"].as_str().is_some_and(|text|
                !text.is_empty() && text.len() <= 512 && text.starts_with(challenge))
                && ["geetest_validate", "geetest_seccode"].iter().all(|key|
                    value[*key].as_str().is_some_and(|text| !text.is_empty() && text.len() <= 1024))
                && value.as_object().is_some_and(|object| object.len() == 3),
        };
        if valid { Ok(()) } else { Err(GatewayError::bad_request("请完成当前步骤的人机验证")) }
    }
}
