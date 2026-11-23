use serde::{Deserialize, Serialize};

use crate::server::errors::GatewayError;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SmsConfig {
    pub phone_label: String,
    #[serde(default)]
    pub phone_placeholder: String,
    #[serde(default)]
    pub country_codes: Vec<CountryCode>,
    pub phone_min_length: usize,
    pub phone_max_length: usize,
    pub code_min_length: usize,
    pub code_max_length: usize,
    pub resend_after_seconds: u64,
    pub expires_after_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<super::sms::Challenge>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CountryCode {
    pub value: String,
    pub label: String,
}

impl SmsConfig {
    pub fn validate(&self) -> Result<(), GatewayError> {
        if self.phone_label.trim().is_empty() || self.phone_label.len() > 256 || self.phone_label.contains('\0')
            || self.phone_placeholder.len() > 256 || self.phone_placeholder.contains('\0')
            || !(1..=32).contains(&self.phone_min_length)
            || !(self.phone_min_length..=32).contains(&self.phone_max_length)
            || !(1..=16).contains(&self.code_min_length)
            || !(self.code_min_length..=16).contains(&self.code_max_length)
            || !(1..=3600).contains(&self.resend_after_seconds)
            || !(30..=900).contains(&self.expires_after_seconds)
            || self.country_codes.len() > 256
        {
            return Err(GatewayError::with_status(502, "插件返回的短信登录声明无效"));
        }
        let mut codes = std::collections::HashSet::new();
        for code in &self.country_codes {
            if code.value.is_empty() || code.value.len() > 6
                || !code.value.bytes().all(|byte| byte.is_ascii_digit())
                || !codes.insert(&code.value) || code.label.trim().is_empty() || code.label.len() > 128 || code.label.contains('\0')
            {
                return Err(GatewayError::with_status(502, "插件返回的手机区号声明无效"));
            }
        }
        if let Some(challenge) = &self.challenge {
            if !matches!(challenge, super::sms::Challenge::Alicom4 { .. }) {
                return Err(GatewayError::with_status(502, "插件返回的短信初始人机验证不支持"));
            }
            challenge.validate()?;
        }
        Ok(())
    }

    pub fn phone(&self, phone: &str, country_code: &str) -> Result<String, GatewayError> {
        let phone = phone.trim();
        if !(self.phone_min_length..=self.phone_max_length).contains(&phone.len())
            || !phone.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(GatewayError::bad_request("请填写正确的手机号"));
        }
        if !self.country_codes.is_empty() && !self.country_codes.iter().any(|code| code.value == country_code)
            || self.country_codes.is_empty() && !country_code.is_empty()
        {
            return Err(GatewayError::bad_request("请选择有效的手机区号"));
        }
        Ok(phone.to_string())
    }

    pub fn code(&self, code: &str) -> Result<String, GatewayError> {
        let code = code.trim();
        if !(self.code_min_length..=self.code_max_length).contains(&code.len())
            || !code.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(GatewayError::bad_request("请填写正确的验证码"));
        }
        Ok(code.to_string())
    }
}
