//! CodeArts 显式纳管的宿主完整材料候选；仅格式及材料关联核验，不查询上游或账号库。
//! 成功不代表授权已验证、刷新令牌未消费、Ready、用户确认或发送许可。
//! 字段/身份来源：仓内 wasm/codearts/{credentials,account,dpop}.rs 与原生 codearts/oauth.rs。
//! 只接受平铺单账号或唯一 credential/codearts_provider_credential 包装，不读取文件/路径。

mod json;
mod oauth;

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use zeroize::Zeroizing;

use json::SecretJson;

pub(crate) const MAX_INPUT_BYTES: usize = 128 * 1024;
const MAX_TOKEN_BYTES: usize = 8192;
const MAX_IDENTITY_COMPONENT: usize = 128;

/// 所有错误只带固定种类；不携带原文、字段值、密钥、路径或底层解码诊断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnrollmentInputError {
    InvalidJson,
    DuplicateKey,
    LimitExceeded,
    InvalidShape,
    UnsupportedField,
    MissingMaterial,
    InconsistentAliases,
    WrongProvider,
    InvalidToken,
    InvalidIdentity,
    InvalidExpiration,
    Expired,
    InvalidClient,
    InvalidPkce,
    InvalidDpop,
    InvalidName,
    ClockUnavailable,
}

impl EnrollmentInputError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::InvalidJson => "codearts_enrollment_invalid_json",
            Self::DuplicateKey => "codearts_enrollment_duplicate_key",
            Self::LimitExceeded => "codearts_enrollment_input_limit",
            Self::InvalidShape => "codearts_enrollment_invalid_shape",
            Self::UnsupportedField => "codearts_enrollment_unsupported_field",
            Self::MissingMaterial => "codearts_enrollment_incomplete_material",
            Self::InconsistentAliases => "codearts_enrollment_alias_conflict",
            Self::WrongProvider => "codearts_enrollment_wrong_provider",
            Self::InvalidToken => "codearts_enrollment_invalid_token",
            Self::InvalidIdentity => "codearts_enrollment_invalid_identity",
            Self::InvalidExpiration => "codearts_enrollment_invalid_expiration",
            Self::Expired => "codearts_enrollment_expired_material",
            Self::InvalidClient => "codearts_enrollment_invalid_client",
            Self::InvalidPkce => "codearts_enrollment_invalid_pkce",
            Self::InvalidDpop => "codearts_enrollment_invalid_dpop",
            Self::InvalidName => "codearts_enrollment_invalid_name",
            Self::ClockUnavailable => "codearts_enrollment_clock_unavailable",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::InvalidJson => "CodeArts 凭据不是合法的单账号 JSON，未保留原始诊断",
            Self::DuplicateKey => "CodeArts 凭据含重复字段，未选择或合并其中一份",
            Self::LimitExceeded => "CodeArts 凭据超过允许的字节、层数或节点限额",
            Self::InvalidShape => "CodeArts 凭据只能是平铺对象或一个原有凭据包装，不能混用多份包装",
            Self::UnsupportedField => {
                "CodeArts 纳管材料含未允许的字段，不接受来源、路径、账号记录或许可标记"
            }
            Self::MissingMaterial => {
                "本次 CodeArts 材料不完整，请明确提供全部短期凭据、身份和原 OAuth 材料"
            }
            Self::InconsistentAliases => "CodeArts 同义字段互相矛盾，未选择其中一份或用旧材料补齐",
            Self::WrongProvider => "本次材料不属于 CodeArts，未读取或改动其他提供商账号",
            Self::InvalidToken => "CodeArts 凭据字符、长度或字段类型无效，未公开其内容",
            Self::InvalidIdentity => {
                "CodeArts 域与用户身份不完整、不明确或含受保护内容，未使用尾号或摘要补造身份"
            }
            Self::InvalidExpiration => "CodeArts 必须明确给出合法 RFC3339 到期时间，不能推测有效期",
            Self::Expired => "本次 CodeArts 临时凭据已到期，未沿用旧材料或推测新的期限",
            Self::InvalidClient => "CodeArts 原 OAuth 客户端标识必须明确有效，未填入默认客户端",
            Self::InvalidPkce => {
                "CodeArts 原 PKCE 材料不完整或 SHA256 对应关系不符，未生成替代材料"
            }
            Self::InvalidDpop => {
                "CodeArts 原 P-256 公私钥材料不完整、编码不规范或不对应，未生成替代密钥"
            }
            Self::InvalidName => "CodeArts 名称字段格式无效，未公开原始内容",
            Self::ClockUnavailable => "主程序当前时间无法确认，未把未知有效期当作仍有效",
        }
    }
}

/// 字段私有，无 Clone/Debug/serde；只能由本次原文通过宿主核验后构造。
/// 借用仅供同库保存，不能把 secret 或本对象直接序列化到 UI；不携带账号/安装/DB 来源。
#[must_use]
pub(crate) struct EnrollmentInput {
    identity: Zeroizing<String>,
    name: Zeroizing<String>,
    secret: SecretJson,
    public: SecretJson,
    expires_at_ms: i64,
}

impl EnrollmentInput {
    pub(crate) fn parse(raw: &[u8]) -> Result<Self, EnrollmentInputError> {
        let parsed = json::parse(raw)?;
        let fields = payload(parsed.value())?;
        allowed(fields, MATERIAL_FIELDS)?;
        if let Some(provider) = fields.get("provider") {
            if provider.as_str() != Some("codearts") {
                return Err(EnrollmentInputError::WrongProvider);
            }
        }
        let material = Material::read(fields)?;
        // 私钥、verifier与challenge的真实关联在主程序中校验，不信插件输出的Complete。
        let context = oauth::validate(
            fields
                .get("oauth_context")
                .ok_or(EnrollmentInputError::MissingMaterial)?,
        )?;
        let expires_at_ms = expiration(fields, material.expiration)?;
        if expires_at_ms <= now_ms()? {
            return Err(EnrollmentInputError::Expired);
        }
        let identity = Zeroizing::new(format!("codearts-{}-{}", material.domain, material.user));
        if protected(material.domain, fields)
            || protected(material.user, fields)
            || protected(&identity, fields)
        {
            return Err(EnrollmentInputError::InvalidIdentity);
        }
        let name = display_name(fields)?;
        // 输出从已核对的本次字段生成，不拷贝未知键、source、path、private版本或许可标志。
        let mut secret = SecretJson::object();
        for (key, value) in [
            ("access_key_id", material.access),
            ("accessToken", material.access),
            ("secret_access_key", material.secret),
            ("security_token", material.security),
            ("refresh_token", material.refresh),
            ("refreshToken", material.refresh),
            ("expires_at", material.expiration),
            ("expiration", material.expiration),
            ("domain_id", material.domain),
            ("user_id", material.user),
            ("userId", material.user),
        ] {
            secret.insert(key, Value::String(value.to_string()))?;
        }
        secret.insert("expiresAt", Value::from(expires_at_ms))?;
        secret.insert("oauth_context", Value::clone(context))?;
        if fields.contains_key("name") {
            secret.insert("name", Value::String(name.to_string()))?;
        }
        if let Some(value) = optional_text(fields, &["user_name", "userName"], 1024)? {
            // 名称不参与新的授权构造；secret原值保留，展示名另做脱敏。
            secret.insert("user_name", Value::String(value.to_string()))?;
        }
        if let Some(value) = optional_text(fields, &["login_type", "loginType"], 64)? {
            secret.insert("login_type", Value::String(value.to_string()))?;
        }
        let mut public = SecretJson::object();
        for (key, value) in [
            ("name", name.as_str()),
            ("domainId", material.domain),
            ("userId", material.user),
            ("uid", material.user),
            ("identityStatus", "complete"),
            ("validation", "format_only"),
        ] {
            public.insert(key, Value::String(value.to_string()))?;
        }
        public.insert("expiresAt", Value::from(expires_at_ms))?;
        public.insert("accessTokenExpiresAt", Value::from(expires_at_ms))?;
        // true仅表示本次必填材料实际存在，绝不是查询成功、授权有效或Ready。
        public.insert("hasAccessToken", Value::Bool(true))?;
        public.insert("hasRefreshToken", Value::Bool(true))?;
        public.insert("canRefresh", Value::Bool(false))?;
        public.insert("upstreamValidated", Value::Bool(false))?;
        public.insert("refreshRequestSent", Value::Bool(false))?;
        Ok(Self {
            identity,
            name,
            secret,
            public,
            expires_at_ms,
        })
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn secret(&self) -> &Value {
        self.secret.value()
    }
    pub(crate) fn public(&self) -> &Value {
        self.public.value()
    }
    /// confirm仍须在原保存前核对当前时间；prepare时通过不代表等待后仍未到期。
    pub(crate) fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}

// 同次原生材料及仓内已使用镜像。未知字段拒绝；不接受UID猜测、JWT身份或任意包装。
const MATERIAL_FIELDS: &[&str] = &[
    "provider",
    "name",
    "user_name",
    "userName",
    "login_type",
    "loginType",
    "oauth_context",
    "access_key_id",
    "accessKeyId",
    "accessToken",
    "secret_access_key",
    "secretAccessKey",
    "security_token",
    "securityToken",
    "refresh_token",
    "refreshToken",
    "expires_at",
    "expiration",
    "expiresAt",
    "domain_id",
    "account_id",
    "domainId",
    "user_id",
    "principal_id",
    "userId",
];

struct Material<'a> {
    access: &'a str,
    secret: &'a str,
    security: &'a str,
    refresh: &'a str,
    expiration: &'a str,
    domain: &'a str,
    user: &'a str,
}

impl<'a> Material<'a> {
    fn read(fields: &'a Map<String, Value>) -> Result<Self, EnrollmentInputError> {
        let access = required(
            fields,
            &["access_key_id", "accessKeyId", "accessToken"],
            MAX_TOKEN_BYTES,
        )?;
        let secret = required(
            fields,
            &["secret_access_key", "secretAccessKey"],
            MAX_TOKEN_BYTES,
        )?;
        let security = required(
            fields,
            &["security_token", "securityToken"],
            MAX_TOKEN_BYTES,
        )?;
        let refresh = required(fields, &["refresh_token", "refreshToken"], MAX_TOKEN_BYTES)?;
        for value in [access, secret, security, refresh] {
            if value.bytes().any(|byte| !(0x21..=0x7e).contains(&byte)) {
                return Err(EnrollmentInputError::InvalidToken);
            }
        }
        let expiration = required(fields, &["expires_at", "expiration"], 128).map_err(|error| {
            if error == EnrollmentInputError::InconsistentAliases {
                error
            } else {
                EnrollmentInputError::InvalidExpiration
            }
        })?;
        let domain = required(
            fields,
            &["domain_id", "account_id", "domainId"],
            MAX_IDENTITY_COMPONENT,
        )
        .map_err(identity_error)?;
        let user = required(
            fields,
            &["user_id", "principal_id", "userId"],
            MAX_IDENTITY_COMPONENT,
        )
        .map_err(identity_error)?;
        // 与WASM stable_identity同一无歧义规则：'-'为拼接分隔符，不允许出现在两个分量中。
        if [domain, user].iter().any(|value| {
            !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }) {
            return Err(EnrollmentInputError::InvalidIdentity);
        }
        Ok(Self {
            access,
            secret,
            security,
            refresh,
            expiration,
            domain,
            user,
        })
    }
}

fn identity_error(error: EnrollmentInputError) -> EnrollmentInputError {
    match error {
        EnrollmentInputError::InconsistentAliases => error,
        _ => EnrollmentInputError::InvalidIdentity,
    }
}

fn payload(value: &Value) -> Result<&Map<String, Value>, EnrollmentInputError> {
    let fields = value
        .as_object()
        .ok_or(EnrollmentInputError::InvalidShape)?;
    let first = fields.get("codearts_provider_credential");
    let second = fields.get("credential");
    match (first, second) {
        (Some(_), Some(_)) => Err(EnrollmentInputError::InvalidShape),
        (Some(value), None) | (None, Some(value)) if fields.len() == 1 => {
            value.as_object().ok_or(EnrollmentInputError::InvalidShape)
        }
        (None, None) => Ok(fields),
        _ => Err(EnrollmentInputError::InvalidShape),
    }
}

pub(super) fn allowed(
    fields: &Map<String, Value>,
    keys: &[&str],
) -> Result<(), EnrollmentInputError> {
    if fields.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(EnrollmentInputError::UnsupportedField);
    }
    Ok(())
}

pub(super) fn required<'a>(
    fields: &'a Map<String, Value>,
    aliases: &[&str],
    maximum: usize,
) -> Result<&'a str, EnrollmentInputError> {
    alias_text(fields, aliases, maximum)?
        .filter(|text| !text.is_empty())
        .ok_or(EnrollmentInputError::MissingMaterial)
}

fn optional_text<'a>(
    fields: &'a Map<String, Value>,
    aliases: &[&str],
    maximum: usize,
) -> Result<Option<&'a str>, EnrollmentInputError> {
    Ok(alias_text(fields, aliases, maximum)?.filter(|text| !text.is_empty()))
}

fn alias_text<'a>(
    fields: &'a Map<String, Value>,
    aliases: &[&str],
    maximum: usize,
) -> Result<Option<&'a str>, EnrollmentInputError> {
    let mut selected = None;
    for name in aliases {
        if let Some(value) = fields.get(*name) {
            let text = value.as_str().ok_or(EnrollmentInputError::InvalidToken)?;
            // 先校验原文，不让trim隐藏控制字符、空白或两份不同别名。
            if text.len() > maximum || text.trim() != text || text.chars().any(char::is_control) {
                return Err(EnrollmentInputError::InvalidToken);
            }
            if selected.is_some_and(|previous| previous != text) {
                return Err(EnrollmentInputError::InconsistentAliases);
            }
            selected = Some(text);
        }
    }
    Ok(selected)
}

fn expiration(fields: &Map<String, Value>, text: &str) -> Result<i64, EnrollmentInputError> {
    let expires = chrono::DateTime::parse_from_rfc3339(text)
        .map_err(|_| EnrollmentInputError::InvalidExpiration)?
        .timestamp_millis();
    if expires <= 0 {
        return Err(EnrollmentInputError::InvalidExpiration);
    }
    if fields
        .get("expiresAt")
        .is_some_and(|value| value.as_i64() != Some(expires))
    {
        return Err(EnrollmentInputError::InconsistentAliases);
    }
    Ok(expires)
}

fn now_ms() -> Result<i64, EnrollmentInputError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| EnrollmentInputError::ClockUnavailable)?;
    i64::try_from(duration.as_millis()).map_err(|_| EnrollmentInputError::ClockUnavailable)
}

fn display_name(fields: &Map<String, Value>) -> Result<Zeroizing<String>, EnrollmentInputError> {
    let explicit =
        optional_text(fields, &["name"], 400).map_err(|_| EnrollmentInputError::InvalidName)?;
    let user_name = optional_text(fields, &["user_name", "userName"], 1024).map_err(|error| {
        if error == EnrollmentInputError::InconsistentAliases {
            error
        } else {
            EnrollmentInputError::InvalidName
        }
    })?;
    let selected = explicit.or(user_name).unwrap_or("CodeArts 账号");
    if selected.chars().count() > 100 {
        return Err(EnrollmentInputError::InvalidName);
    }
    Ok(Zeroizing::new(public_text(selected, fields).to_string()))
}

fn public_text<'a>(text: &'a str, fields: &Map<String, Value>) -> &'a str {
    if text.contains("://") || protected(text, fields) {
        "[已隐藏]"
    } else {
        text
    }
}

fn protected(text: &str, fields: &Map<String, Value>) -> bool {
    let contains = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty() && text.contains(value))
    };
    if [
        "access_key_id",
        "accessKeyId",
        "accessToken",
        "secret_access_key",
        "secretAccessKey",
        "security_token",
        "securityToken",
        "refresh_token",
        "refreshToken",
    ]
    .iter()
    .any(|name| contains(fields.get(*name)))
    {
        return true;
    }
    let context = fields.get("oauth_context");
    for name in ["code_verifier", "code_challenge"] {
        if contains(
            context
                .and_then(|value| value.get("pkce_pair"))
                .and_then(|value| value.get(name)),
        ) {
            return true;
        }
    }
    for part in ["private_key_jwk", "public_key_jwk"] {
        for coordinate in ["d", "x", "y"] {
            if contains(
                context
                    .and_then(|value| value.get("dpop_key_pair"))
                    .and_then(|value| value.get(part))
                    .and_then(|value| value.get(coordinate)),
            ) {
                return true;
            }
        }
    }
    false
}
