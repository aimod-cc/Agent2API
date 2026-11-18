//! Cline Free 本次完整材料候选：只核格式与字段一致性，不验证 JWT 签名或上游授权。
//! 只接受平铺单账号或唯一 auth 包装；不从旧行、文件路径、Pass 或插件许可补材料。
//! 规则依据：wasm/cline-common/account.rs、refresh.rs，以及原生 cline/credentials.rs。

mod json;
mod material;

use serde_json::{Map, Value};
use zeroize::Zeroizing;

use json::SecretJson;

pub(crate) const MAX_FREE_INPUT_BYTES: usize = 128 * 1024;
const MAX_TOKEN_BYTES: usize = 8192;
const MAX_UID_BYTES: usize = 512;

/// 错误只带固定类别；不保留 JSON、令牌、字段值或底层解码诊断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FreeInputError {
    InvalidJson,
    DuplicateKey,
    LimitExceeded,
    InvalidShape,
    UnsupportedField,
    WrongProvider,
    DesktopReference,
    MissingMaterial,
    InvalidToken,
    ConflictingAliases,
    InvalidIdentity,
    IdentityConflict,
    InvalidExpiration,
    ExpirationConflict,
    InvalidName,
}

impl FreeInputError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::InvalidJson => "cline_free_input_invalid_json",
            Self::DuplicateKey => "cline_free_input_duplicate_key",
            Self::LimitExceeded => "cline_free_input_limit",
            Self::InvalidShape => "cline_free_input_invalid_shape",
            Self::UnsupportedField => "cline_free_input_unsupported_field",
            Self::WrongProvider => "cline_free_input_wrong_provider",
            Self::DesktopReference => "cline_free_input_desktop_reference",
            Self::MissingMaterial => "cline_free_input_incomplete_material",
            Self::InvalidToken => "cline_free_input_invalid_token",
            Self::ConflictingAliases => "cline_free_input_alias_conflict",
            Self::InvalidIdentity => "cline_free_input_invalid_identity",
            Self::IdentityConflict => "cline_free_input_identity_conflict",
            Self::InvalidExpiration => "cline_free_input_invalid_expiration",
            Self::ExpirationConflict => "cline_free_input_expiration_conflict",
            Self::InvalidName => "cline_free_input_invalid_name",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::InvalidJson => "Cline Free 材料不是合法单账号 JSON，未保留原始诊断",
            Self::DuplicateKey => "Cline Free 材料含重复字段，未选择或合并其中一份",
            Self::LimitExceeded => "Cline Free 材料超过允许的字节、层数或节点限额",
            Self::InvalidShape => {
                "Cline Free 材料只能是平铺单账号或唯一 auth 包装，不能混装多份来源"
            }
            Self::UnsupportedField => {
                "Cline Free 材料含未允许的字段，不接受账号记录、路径、代理或管理许可"
            }
            Self::WrongProvider => "本次材料必须明确属于 Cline Free，未处理其他额度池",
            Self::DesktopReference => "Cline Free 完整材料不能是桌面实时引用，未读取本机凭据",
            Self::MissingMaterial => "请在本次材料中明确提供完整访问令牌和刷新令牌，未从旧行补齐",
            Self::InvalidToken => "Cline Free 凭据的类型、字符、前缀或长度无效，未公开其内容",
            Self::ConflictingAliases => "Cline Free 同义字段互相冲突，未选择其中一份",
            Self::InvalidIdentity => {
                "Cline Free 缺少可明确核对的本家 UID，未用邮箱、名称或摘要补造身份"
            }
            Self::IdentityConflict => "Cline Free 本次材料的 UID 字段互相冲突，未替换账号身份",
            Self::InvalidExpiration => "Cline Free 到期时间格式或范围无效，未推测有效期",
            Self::ExpirationConflict => {
                "Cline Free 材料到期时间与本次 JWT 声明不一致，未选一个延长期限"
            }
            Self::InvalidName => "Cline Free 名称字段格式无效，未公开原始内容",
        }
    }
}

/// 私有字段，无 Clone/Debug/serde；成功只表示本次材料一致，不含确认、账号来源或 Ready。
/// 所有自持字符串与 JSON 由私有清理所有者释放；借入 raw 和 serde 内部缓冲不在保证内。
#[must_use]
pub(crate) struct FreeEnrollmentInput {
    identity: Zeroizing<String>,
    secret: SecretJson,
    public: SecretJson,
    expires_at_ms: Option<i64>,
}

impl FreeEnrollmentInput {
    pub(crate) fn parse(raw: &[u8]) -> Result<Self, FreeInputError> {
        let mut budget = json::Budget::new();
        let parsed = budget.parse(raw)?;
        let fields = payload(parsed.value())?;
        allowed(fields, MATERIAL_FIELDS)?;
        check_provider(fields)?;
        let tokens = material::Tokens::read(fields)?;
        let claims = material::claims(&tokens.access, &mut budget)?;
        let info = user_info(fields)?;
        let explicit = alias_text(
            fields,
            &["account", "userId", "accountId", "uid"],
            MAX_UID_BYTES,
            FreeInputError::InvalidIdentity,
        )?;
        let declared = material::uid(claims.value(), &["external_id"])?;
        let info_uid = info
            .map(|info| material::uid_fields(info, &["clineUserId", "external_id"]))
            .transpose()?
            .flatten();
        let uid = material::join_uid(material::join_uid(explicit, declared)?, info_uid)?
            .ok_or(FreeInputError::InvalidIdentity)?;
        // 原 refresh::original_uid 只接受 external_id 或已明确的 usr- 标识；不借 WorkOS sub。
        if declared.is_none() && (!uid.starts_with("usr-") || uid.len() <= 4) {
            return Err(FreeInputError::InvalidIdentity);
        }
        if material::reference(uid) || tokens.protected(uid) {
            return Err(FreeInputError::InvalidIdentity);
        }
        let expires_at_ms = material::expiration(fields.get("expiresAt"), claims.value())?;
        let name = display_name(fields, info, claims.value(), uid, &tokens)?;
        // account 固定为本次 UID，与原 Free account::identity 的首选字段严格相同。
        let identity = Zeroizing::new(uid.to_string());
        let mut secret = SecretJson::object();
        let mut public = SecretJson::object();
        secret.insert("accessToken", Value::String(tokens.access.to_string()))?;
        secret.insert("refreshToken", Value::String(tokens.refresh.to_string()))?;
        secret.insert("account", Value::String(uid.to_string()))?;
        secret.insert("displayName", Value::String(name.to_string()))?;
        // 显式 null 表示未知，不沿用旧行的到期时间，也不补 0 或默认一小时。
        secret.insert(
            "expiresAt",
            expires_at_ms.map(Value::from).unwrap_or(Value::Null),
        )?;
        for (key, text) in [
            ("name", name.as_str()),
            ("displayName", name.as_str()),
            ("uid", uid),
            ("account", uid),
        ] {
            public.insert(key, Value::String(text.to_string()))?;
        }
        let mut tail = Zeroizing::new(
            tokens
                .access
                .chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>(),
        );
        // 短的不透明令牌或刷新凭据恰好落在尾号中时，不把完整凭据当作公开尾号。
        if tokens.protected(&tail) {
            *tail = String::new();
        }
        public.insert("tokenTail", Value::String(tail.to_string()))?;
        public.insert("hasAccessToken", Value::Bool(true))?;
        public.insert("hasRefreshToken", Value::Bool(true))?;
        public.insert("canRefresh", Value::Bool(true))?;
        public.insert("desktopAuth", Value::Bool(false))?;
        public.insert("upstreamValidated", Value::Bool(false))?;
        public.insert("validation", Value::String("format_only".to_string()))?;
        public.insert(
            "expiresAt",
            expires_at_ms.map(Value::from).unwrap_or(Value::Null),
        )?;
        Ok(Self {
            identity,
            secret,
            public,
            expires_at_ms,
        })
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }
    pub(crate) fn uid(&self) -> &str {
        &self.identity
    }
    pub(crate) fn secret(&self) -> &Value {
        self.secret.value()
    }
    pub(crate) fn public(&self) -> &Value {
        self.public.value()
    }
    /// 只返回本次材料的声明；后续真实保存仍须复核当时期限，不构成发送许可。
    pub(crate) fn expires_at_ms(&self) -> Option<i64> {
        self.expires_at_ms
    }
}

// access/refresh 别名来自原 account::access/parse 和 refresh::token_field；UID 的 uid 是
// 本入口明确材料字段，account/userId/accountId 与原 account::identity 同义。
// userInfo 来自 /auth/refresh；metadata.userInfo 来自原 auth 投影，不带其他 metadata。
const MATERIAL_FIELDS: &[&str] = &[
    "provider",
    "pool",
    "desktop",
    "accessToken",
    "access_token",
    "token",
    "refreshToken",
    "refresh_token",
    "account",
    "userId",
    "accountId",
    "uid",
    "expiresAt",
    "name",
    "displayName",
    "tokenType",
    "token_type",
    "userInfo",
    "metadata",
];
const INFO_FIELDS: &[&str] = &[
    "clineUserId",
    "external_id",
    "subject",
    "email",
    "name",
    "firstName",
    "lastName",
];

fn payload(value: &Value) -> Result<&Map<String, Value>, FreeInputError> {
    let fields = value.as_object().ok_or(FreeInputError::InvalidShape)?;
    match fields.get("auth") {
        Some(value) if fields.len() == 1 => value.as_object().ok_or(FreeInputError::InvalidShape),
        Some(_) => Err(FreeInputError::InvalidShape),
        None => Ok(fields),
    }
}

fn check_provider(fields: &Map<String, Value>) -> Result<(), FreeInputError> {
    if fields
        .get("provider")
        .is_some_and(|value| value.as_str() != Some("cline-free"))
    {
        return Err(FreeInputError::WrongProvider);
    }
    if fields.get("pool").is_some_and(|value| {
        value.as_str().is_none_or(|pool| {
            !pool.eq_ignore_ascii_case("free") && !pool.eq_ignore_ascii_case("cline-free")
        })
    }) {
        // 只取原 Pool::parse 的明确 free 分支，不搬入未知/空值回落 Pass 的规则。
        return Err(FreeInputError::WrongProvider);
    }
    if fields
        .get("desktop")
        .is_some_and(|value| value.as_bool() != Some(false))
    {
        return Err(FreeInputError::DesktopReference);
    }
    for key in ["tokenType", "token_type"] {
        if fields.get(key).is_some_and(|value| {
            value
                .as_str()
                .is_none_or(|kind| !kind.eq_ignore_ascii_case("Bearer"))
        }) {
            return Err(FreeInputError::InvalidToken);
        }
    }
    Ok(())
}

fn user_info(fields: &Map<String, Value>) -> Result<Option<&Map<String, Value>>, FreeInputError> {
    let metadata = match fields.get("metadata") {
        Some(value) => {
            let metadata = value.as_object().ok_or(FreeInputError::InvalidShape)?;
            allowed(metadata, &["userInfo"])?;
            metadata.get("userInfo")
        }
        None => None,
    };
    let info = match (fields.get("userInfo"), metadata) {
        (Some(_), Some(_)) => return Err(FreeInputError::InvalidShape),
        (Some(value), None) | (None, Some(value)) => {
            Some(value.as_object().ok_or(FreeInputError::InvalidShape)?)
        }
        (None, None) => None,
    };
    if let Some(info) = info {
        allowed(info, INFO_FIELDS)?;
        for (key, value) in info {
            let text = value.as_str().ok_or(FreeInputError::InvalidShape)?;
            let limit = if matches!(key.as_str(), "clineUserId" | "external_id") {
                MAX_UID_BYTES
            } else {
                1024
            };
            if text.len() > limit || text.trim() != text || text.chars().any(char::is_control) {
                return Err(FreeInputError::InvalidShape);
            }
        }
    }
    Ok(info)
}

fn allowed(fields: &Map<String, Value>, keys: &[&str]) -> Result<(), FreeInputError> {
    if fields.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(FreeInputError::UnsupportedField);
    }
    Ok(())
}

pub(super) fn alias_text<'a>(
    fields: &'a Map<String, Value>,
    keys: &[&str],
    maximum: usize,
    invalid: FreeInputError,
) -> Result<Option<&'a str>, FreeInputError> {
    let mut found = None;
    for key in keys {
        if let Some(value) = fields.get(*key) {
            let text = value.as_str().ok_or(invalid)?;
            if text.is_empty()
                || text.len() > maximum
                || text.trim() != text
                || text.chars().any(char::is_control)
            {
                return Err(invalid);
            }
            if found.is_some_and(|previous| previous != text) {
                return Err(FreeInputError::ConflictingAliases);
            }
            found = Some(text);
        }
    }
    Ok(found)
}

fn display_name(
    fields: &Map<String, Value>,
    info: Option<&Map<String, Value>>,
    claims: &Value,
    uid: &str,
    tokens: &material::Tokens,
) -> Result<Zeroizing<String>, FreeInputError> {
    let mut explicit = None;
    for key in ["displayName", "name"] {
        if let Some(value) = fields.get(key) {
            let text = value.as_str().ok_or(FreeInputError::InvalidName)?;
            if text.len() > 400
                || text.chars().count() > 100
                || text.trim() != text
                || text.chars().any(char::is_control)
                || material::reference(text)
            {
                return Err(FreeInputError::InvalidName);
            }
            if explicit.is_some_and(|previous| previous != text) {
                return Err(FreeInputError::ConflictingAliases);
            }
            explicit = Some(text);
        }
    }
    let fallback = info
        .map(|info| material::display(info, true))
        .filter(|name| !name.is_empty())
        .or_else(|| {
            claims
                .as_object()
                .map(|claims| material::display(claims, false))
        });
    let selected = explicit
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| fallback.map(|text| text.to_string()))
        .unwrap_or_else(|| uid.to_string());
    let selected = Zeroizing::new(selected);
    let hidden = Zeroizing::new("[已隐藏]".to_string());
    let selected = if material::reference(&selected) || tokens.protected(&selected) {
        if tokens.protected(&hidden) {
            ""
        } else {
            hidden.as_str()
        }
    } else {
        selected.as_str()
    };
    Ok(Zeroizing::new(
        selected
            .chars()
            .filter(|ch| !ch.is_control())
            .take(100)
            .collect(),
    ))
}
