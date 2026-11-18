//! 两令牌/本家 UID/期限的固定关联，全部取本次材料，不读账号或补旧字段。
//! JWT 仅解码声明；不验证签名、issuer、上游授权或刷新凭据是否未消费。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use super::{
    alias_text,
    json::{Budget, SecretJson},
    FreeInputError, MAX_TOKEN_BYTES, MAX_UID_BYTES,
};

const PREFIX: &str = "workos:";

pub(super) struct Tokens {
    pub(super) access: Zeroizing<String>,
    pub(super) refresh: Zeroizing<String>,
}

impl Tokens {
    pub(super) fn read(fields: &Map<String, Value>) -> Result<Self, FreeInputError> {
        Ok(Self {
            access: token(fields, &["accessToken", "access_token", "token"], true)?,
            refresh: token(fields, &["refreshToken", "refresh_token"], false)?,
        })
    }

    pub(super) fn protected(&self, text: &str) -> bool {
        let raw = self
            .access
            .strip_prefix(PREFIX)
            .unwrap_or(self.access.as_str());
        [self.access.as_str(), self.refresh.as_str(), raw]
            .iter()
            .any(|token| !token.is_empty() && text.contains(*token))
    }
}

fn token(
    fields: &Map<String, Value>,
    aliases: &[&str],
    access: bool,
) -> Result<Zeroizing<String>, FreeInputError> {
    let mut found: Option<Zeroizing<String>> = None;
    for name in aliases {
        if let Some(value) = fields.get(*name) {
            let raw = value.as_str().ok_or(FreeInputError::InvalidToken)?;
            // 完整材料采用原 refresh::token_field 严格原文规则，不 trim/Bearer 清洗坏材料。
            if raw.is_empty()
                || raw.len() > MAX_TOKEN_BYTES
                || !raw.is_ascii()
                || raw
                    .bytes()
                    .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
                || token_reference(raw)
            {
                return Err(FreeInputError::InvalidToken);
            }
            let next = if access {
                match raw.strip_prefix(PREFIX) {
                    Some("") => return Err(FreeInputError::InvalidToken),
                    Some(value) if value.starts_with(PREFIX) => {
                        return Err(FreeInputError::InvalidToken)
                    }
                    Some(_) => Zeroizing::new(raw.to_string()),
                    None => Zeroizing::new(format!("{PREFIX}{raw}")),
                }
            } else {
                Zeroizing::new(raw.to_string())
            };
            if next.len() > MAX_TOKEN_BYTES {
                return Err(FreeInputError::InvalidToken);
            }
            if found
                .as_ref()
                .is_some_and(|previous| previous.as_str() != next.as_str())
            {
                return Err(FreeInputError::ConflictingAliases);
            }
            found = Some(next);
        }
    }
    found.ok_or(FreeInputError::MissingMaterial)
}

pub(super) fn claims(token: &str, budget: &mut Budget) -> Result<SecretJson, FreeInputError> {
    let raw = token.strip_prefix(PREFIX).unwrap_or(token);
    if !raw.contains('.') {
        return Ok(SecretJson::null());
    }
    let mut parts = raw.split('.');
    let header = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or(FreeInputError::InvalidToken)?;
    let payload = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or(FreeInputError::InvalidToken)?;
    let signature = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or(FreeInputError::InvalidToken)?;
    if parts.next().is_some() || header.len() + payload.len() + signature.len() > MAX_TOKEN_BYTES {
        return Err(FreeInputError::InvalidToken);
    }
    let encoded = payload.trim_end_matches('=');
    if payload.len() - encoded.len() > 2 {
        return Err(FreeInputError::InvalidToken);
    }
    let mut decoded = Zeroizing::new(Vec::with_capacity(encoded.len()));
    URL_SAFE_NO_PAD
        .decode_vec(encoded, &mut decoded)
        .map_err(|_| FreeInputError::InvalidToken)?;
    let claims = budget.parse(&decoded)?;
    if !claims.value().is_object() {
        return Err(FreeInputError::InvalidToken);
    }
    Ok(claims)
}

pub(super) fn uid<'a>(
    value: &'a Value,
    aliases: &[&str],
) -> Result<Option<&'a str>, FreeInputError> {
    match value.as_object() {
        Some(fields) => uid_fields(fields, aliases),
        None if value.is_null() => Ok(None),
        None => Err(FreeInputError::InvalidIdentity),
    }
}

pub(super) fn uid_fields<'a>(
    fields: &'a Map<String, Value>,
    aliases: &[&str],
) -> Result<Option<&'a str>, FreeInputError> {
    alias_text(
        fields,
        aliases,
        MAX_UID_BYTES,
        FreeInputError::InvalidIdentity,
    )
}

pub(super) fn join_uid<'a>(
    left: Option<&'a str>,
    right: Option<&'a str>,
) -> Result<Option<&'a str>, FreeInputError> {
    match (left, right) {
        (Some(left), Some(right)) if left != right => Err(FreeInputError::IdentityConflict),
        (Some(left), _) => Ok(Some(left)),
        (_, right) => Ok(right),
    }
}

pub(super) fn expiration(
    value: Option<&Value>,
    claims: &Value,
) -> Result<Option<i64>, FreeInputError> {
    let material = value
        .filter(|value| !value.is_null())
        .map(expiry)
        .transpose()?;
    let jwt = match claims.get("exp") {
        Some(value) if !value.is_null() => Some(valid_expiry(
            value
                .as_i64()
                .filter(|seconds| *seconds > 0)
                .and_then(|seconds| seconds.checked_mul(1000))
                .ok_or(FreeInputError::InvalidExpiration)?,
        )?),
        Some(_) | None => None,
    };
    match (material, jwt) {
        (Some(material), Some(jwt)) if material != jwt => Err(FreeInputError::ExpirationConflict),
        (Some(material), _) => Ok(Some(material)),
        (_, jwt) => Ok(jwt),
    }
}

fn expiry(value: &Value) -> Result<i64, FreeInputError> {
    let at = match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text)
            if !text.is_empty()
                && text.len() <= 64
                && text.trim() == text
                && !text.chars().any(char::is_control) =>
        {
            text.parse::<i64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc3339(text)
                    .ok()
                    .map(|date| date.timestamp_millis())
            })
        }
        _ => None,
    };
    valid_expiry(at.ok_or(FreeInputError::InvalidExpiration)?)
}

fn valid_expiry(at: i64) -> Result<i64, FreeInputError> {
    // JSON/JS 精确整数及 Chrono 都须能表达；合法但已过期的时间留给实际保存门复核。
    if at <= 0
        || at >= (1_i64 << 53)
        || chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at).is_none()
    {
        return Err(FreeInputError::InvalidExpiration);
    }
    Ok(at)
}

fn token_reference(text: &str) -> bool {
    let lower = Zeroizing::new(text.to_ascii_lowercase());
    text.contains('\\')
        || text.starts_with('/')
        || lower.contains("://")
        || [
            "file:", "data:", "desktop:", "path:", "http:", "https:", "ftp:",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        || text.as_bytes().get(1) == Some(&b':')
            && text.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
}

pub(super) fn reference(text: &str) -> bool {
    text.contains('/') || token_reference(text)
}

// 与 account::display 同序：email → 姓名 → 外部UID。只有材料 userInfo 可用 name；
// WorkOS sub/subject 不充当 Cline UID，解出的其他 JWT 字段从不进入 secret/public。
pub(super) fn display(fields: &Map<String, Value>, allow_name: bool) -> Zeroizing<String> {
    let text = |key: &str| {
        fields
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
    };
    let email = text("email");
    if !email.is_empty() {
        return Zeroizing::new(email.to_string());
    }
    let first = text("firstName");
    let last = text("lastName");
    if !first.is_empty() || !last.is_empty() {
        let name = if first.is_empty() {
            last.to_string()
        } else if last.is_empty() {
            first.to_string()
        } else if cjk(first) || cjk(last) {
            format!("{last}{first}")
        } else {
            format!("{first} {last}")
        };
        return Zeroizing::new(name);
    }
    if allow_name && !text("name").is_empty() {
        return Zeroizing::new(text("name").to_string());
    }
    let id = ["external_id", "clineUserId"]
        .iter()
        .map(|key| text(key))
        .find(|id| !id.is_empty())
        .unwrap_or("");
    Zeroizing::new(id.to_string())
}

fn cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(ch as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf
        | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0xac00..=0xd7af)
    })
}
