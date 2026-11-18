//! Kiro 固定换码和本次用户信息响应的纯校验，不产生发送、身份见证或保存权限。
//! 只认已证字段；令牌响应中的邮箱、来源、身份种子等元数据不参与身份确认。
//! 字符串由私有有界解析器持有并清理；借入原文和解析库内部缓冲不保证擦除。

mod json;

use zeroize::Zeroizing;

use crate::server::errors::GatewayError;

const TOKEN_BYTES: usize = 128 * 1024;
const PROFILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOKEN: usize = 8192;
const MAX_PROFILE: usize = 2048;
const MAX_EMAIL: usize = 400;
const MAX_PLAN: usize = 200;

/// 无 Clone、Debug、序列化或公开字段；只有本次响应中的访问与刷新令牌。
pub(crate) struct TokenResponse {
    access: Zeroizing<String>,
    refresh: Option<Zeroizing<String>>,
    expires: Option<i64>,
    profile: Option<String>,
}

impl TokenResponse {
    pub(crate) fn parse(status: u16, raw: &[u8], now_ms: i64) -> Result<Self, GatewayError> {
        if !(200..300).contains(&status) {
            return Err(token_error());
        }
        let response = json::parse(raw, TOKEN_BYTES).map_err(|_| token_error())?;
        if response.get("error").is_some_and(|value| !value.is_null()) {
            return Err(token_error());
        }
        let access = token_alias(&response, &["accessToken", "access_token"], true)?
            .ok_or_else(token_error)?;
        if access.starts_with("ksk_") {
            return Err(token_error());
        }
        let refresh = token_alias(&response, &["refreshToken", "refresh_token"], false)?;
        let expires = expiry(&response, now_ms)?;
        let profile = profile_alias(&response)?;
        let result = Self {
            access: Zeroizing::new(access.to_string()),
            refresh: refresh.map(|text| Zeroizing::new(text.to_string())),
            expires,
            profile: None,
        };
        if profile.is_some_and(|text| result.contains_secret(text)) {
            return Err(token_error());
        }
        Ok(Self {
            profile: profile.filter(|text| !text.is_empty()).map(str::to_string),
            ..result
        })
    }

    pub(crate) fn access(&self) -> &str {
        self.access.as_str()
    }

    pub(crate) fn refresh(&self) -> Option<&str> {
        self.refresh.as_ref().map(|text| text.as_str())
    }

    /// 毫秒绝对期限；缺读数就是未知，不默认一小时、不继承旧期限。
    pub(crate) fn expires(&self) -> Option<i64> {
        self.expires
    }

    /// 仅为有界响应候选，不能单独授权地区、查询地址或账号身份。
    pub(crate) fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    fn contains_secret(&self, text: &str) -> bool {
        let lower = Zeroizing::new(text.to_lowercase());
        [Some(self.access()), self.refresh()]
            .into_iter()
            .flatten()
            .any(|secret| {
                let clear = Zeroizing::new(secret.to_lowercase());
                let encoded = Zeroizing::new(
                    url::form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>(),
                );
                let encoded_lower = Zeroizing::new(encoded.to_lowercase());
                lower.contains(clear.as_str()) || lower.contains(encoded_lower.as_str())
            })
    }
}

/// 只取固定用户信息查询的邮箱和套餐，不从令牌响应或插件材料补身份。
/// 查询候选与原凭据的归属由持有原输入的 worker/Call 见证，本类型不代替它们。
pub(crate) struct ProfileResponse {
    email: String,
    plan: Option<String>,
}

impl ProfileResponse {
    pub(crate) fn parse(
        status: u16,
        raw: &[u8],
        token: &TokenResponse,
    ) -> Result<Self, GatewayError> {
        if !(200..300).contains(&status) {
            return Err(profile_error());
        }
        let response = json::parse(raw, PROFILE_BYTES).map_err(|_| profile_error())?;
        if response.get("error").is_some_and(|value| !value.is_null()) {
            return Err(profile_error());
        }
        let user = response
            .get("userInfo")
            .filter(|value| value.is_object())
            .ok_or_else(profile_error)?;
        let email = user
            .get("email")
            .and_then(json::SecretJson::as_str)
            .filter(|text| valid_email(text))
            .ok_or_else(profile_error)?;
        if token.contains_secret(email) {
            return Err(profile_error());
        }
        let plan = plan(&response, token)?;
        Ok(Self {
            email: email.to_string(),
            plan,
        })
    }

    pub(crate) fn email(&self) -> &str {
        &self.email
    }

    pub(crate) fn plan(&self) -> Option<&str> {
        self.plan.as_deref()
    }
}

fn valid_token(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_TOKEN
        && !text.eq_ignore_ascii_case("Bearer")
        && !text.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}

fn token_alias<'a>(
    response: &'a json::SecretJson,
    aliases: &[&str],
    required: bool,
) -> Result<Option<&'a str>, GatewayError> {
    let mut found = None;
    for alias in aliases {
        if let Some(value) = response.get(alias) {
            // 只有缺字段可表示没有刷新令牌；显式 null、空串和坏别名均拒绝。
            let text = value
                .as_str()
                .filter(|text| valid_token(text))
                .ok_or_else(token_error)?;
            if found.is_some_and(|old| old != text) {
                return Err(token_error());
            }
            found = Some(text);
        }
    }
    if required && found.is_none() {
        return Err(token_error());
    }
    Ok(found)
}

fn expiry(response: &json::SecretJson, now_ms: i64) -> Result<Option<i64>, GatewayError> {
    let mut seconds = None;
    let mut null = false;
    for alias in ["expiresIn", "expires_in"] {
        if let Some(value) = response.get(alias) {
            if value.is_null() {
                null = true;
                continue;
            }
            let next = value
                .as_i64()
                .filter(|seconds| *seconds > 0)
                .ok_or_else(token_error)?;
            if seconds.is_some_and(|old| old != next) {
                return Err(token_error());
            }
            seconds = Some(next);
        }
    }
    if null && seconds.is_some() {
        return Err(token_error());
    }
    let Some(seconds) = seconds else {
        return Ok(None);
    };
    if now_ms <= 0 {
        return Err(token_error());
    }
    let expires = seconds
        .checked_mul(1000)
        .and_then(|delta| now_ms.checked_add(delta))
        .filter(|expires| {
            *expires > now_ms
                && *expires < (1_i64 << 53)
                && chrono::DateTime::from_timestamp_millis(*expires).is_some()
        })
        .ok_or_else(token_error)?;
    Ok(Some(expires))
}

fn profile_alias(response: &json::SecretJson) -> Result<Option<&str>, GatewayError> {
    let mut found = None;
    let mut null = false;
    for alias in ["profileArn", "profile_arn"] {
        if let Some(value) = response.get(alias) {
            if value.is_null() {
                null = true;
                continue;
            }
            let text = value
                .as_str()
                .filter(|text| {
                    text.len() <= MAX_PROFILE
                        && !text.chars().any(|ch| ch.is_control() || ch.is_whitespace())
                })
                .ok_or_else(token_error)?;
            if found.is_some_and(|old| old != text) {
                return Err(token_error());
            }
            found = Some(text);
        }
    }
    if null && found.is_some() {
        return Err(token_error());
    }
    Ok(found)
}

fn valid_email(text: &str) -> bool {
    if text.is_empty()
        || text.len() > MAX_EMAIL
        || text.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return false;
    }
    text.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && !domain.is_empty() && !domain.contains('@')
    })
}

fn plan(
    response: &json::SecretJson,
    token: &TokenResponse,
) -> Result<Option<String>, GatewayError> {
    let Some(subscription) = response
        .get("subscriptionInfo")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    if !subscription.is_object() {
        return Err(profile_error());
    }
    let Some(title) = subscription
        .get("subscriptionTitle")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let title = title
        .as_str()
        .filter(|text| text.len() <= MAX_PLAN && !text.chars().any(char::is_control))
        .ok_or_else(profile_error)?;
    if token.contains_secret(title) {
        return Err(profile_error());
    }
    // 与插件 usage::plan_name 一致：折叠词间空白，Kiro 固定拼写，其余只大写首字母。
    let normalized = title
        .split_whitespace()
        .map(|word| {
            if word.eq_ignore_ascii_case("kiro") {
                "Kiro".to_string()
            } else {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            }
        })
        .collect::<Vec<String>>()
        .join(" ");
    if normalized.len() > MAX_PLAN || token.contains_secret(&normalized) {
        return Err(profile_error());
    }
    Ok((!normalized.is_empty()).then_some(normalized))
}

fn token_error() -> GatewayError {
    GatewayError::with_status(
        502,
        "Kiro本次授权响应未明确成功、凭据或期限无效或冲突，未接受本次凭据",
    )
    .with_code("kiro_token_response_invalid")
}

fn profile_error() -> GatewayError {
    GatewayError::with_status(
        502,
        "Kiro本次用户信息响应未明确成功、邮箱或套餐无效，未确认账号身份",
    )
    .with_code("kiro_profile_response_invalid")
}
