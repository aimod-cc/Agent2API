//! CodeArts 原材料的固定令牌协议；纯校验/构造，不读取账号、不请求上游、不写槽。
//! 材料来自原 ReadStamp 的借用，但本模块不证明 Ready、原调用有效或发送机会未消费。
//! 请求依据：仓内原生 codearts/oauth.rs；短期三件套签 HMAC 不适用于此令牌 POST。

#[path = "dpop.rs"]
mod dpop;
#[path = "json.rs"]
mod json;

use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::server::core::account_store::credential_version::ReadStamp;

pub(super) const TOKEN_URI: &str = "https://sts.cn-north-4.myhuaweicloud.com/v1/oauth2/tokens";
pub(super) const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const MAX_TOKEN_BYTES: usize = 8192;
const MAX_FORM_BYTES: usize = 32 * 1024;
const ACCESS: &[&str] = &["access_key_id", "accessKeyId", "accessToken"];
const SECRET: &[&str] = &["secret_access_key", "secretAccessKey"];
const SECURITY: &[&str] = &["security_token", "securityToken"];
const REFRESH: &[&str] = &["refresh_token", "refreshToken"];
const EXPIRATION: &[&str] = &["expiration", "expires_at"];
const DOMAIN: &[&str] = &["domain_id", "account_id", "domainId"];
const USER: &[&str] = &["user_id", "principal_id", "userId"];

/// 固定错误不包含原字段、JSON、令牌、签名、私钥或底层解析诊断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProtocolError {
    InvalidMaterial,
    MissingMaterial,
    AliasConflict,
    InvalidToken,
    InvalidIdentity,
    InvalidClient,
    InvalidPkce,
    InvalidDpop,
    InvalidExpiration,
    ExpiredResponse,
    InvalidClock,
    InvalidRandom,
    SigningFailed,
    InvalidHttpStatus,
    UpstreamRejected,
    InvalidJson,
    DuplicateKey,
    LimitExceeded,
    InvalidResponse,
}

impl ProtocolError {
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::InvalidMaterial => "codearts_refresh_material_invalid",
            Self::MissingMaterial => "codearts_refresh_material_missing",
            Self::AliasConflict => "codearts_refresh_alias_conflict",
            Self::InvalidToken => "codearts_refresh_token_material_invalid",
            Self::InvalidIdentity => "codearts_refresh_identity_invalid",
            Self::InvalidClient => "codearts_refresh_client_invalid",
            Self::InvalidPkce => "codearts_refresh_pkce_invalid",
            Self::InvalidDpop => "codearts_refresh_dpop_invalid",
            Self::InvalidExpiration => "codearts_refresh_expiration_invalid",
            Self::ExpiredResponse => "codearts_refresh_response_expired",
            Self::InvalidClock => "codearts_refresh_clock_unavailable",
            Self::InvalidRandom => "codearts_refresh_random_unavailable",
            Self::SigningFailed => "codearts_refresh_proof_failed",
            Self::InvalidHttpStatus => "codearts_refresh_http_status_invalid",
            Self::UpstreamRejected => "codearts_refresh_upstream_rejected",
            Self::InvalidJson => "codearts_refresh_response_json_invalid",
            Self::DuplicateKey => "codearts_refresh_response_duplicate_key",
            Self::LimitExceeded => "codearts_refresh_protocol_limit",
            Self::InvalidResponse => "codearts_refresh_response_invalid",
        }
    }

    pub(super) fn message(self) -> &'static str {
        match self {
            Self::InvalidMaterial => "CodeArts 原凭据形状无效，未重建或补齐换证材料",
            Self::MissingMaterial => "CodeArts 原完整授权材料缺失，未使用旧值或默认材料补齐",
            Self::AliasConflict => "CodeArts 同义字段或兼容位置互相矛盾，未选择其中一份",
            Self::InvalidToken => "CodeArts 令牌材料字符、长度或类型无效，未公开其内容",
            Self::InvalidIdentity => "CodeArts 原域与用户身份不完整或不明确，未推测身份",
            Self::InvalidClient => "CodeArts 原客户端标识必须明确有效，未改用默认客户端",
            Self::InvalidPkce => "CodeArts 原 PKCE 材料不完整或不对应，未生成替代材料",
            Self::InvalidDpop => "CodeArts 原 P-256 公私钥材料不完整或不对应，未生成替代密钥",
            Self::InvalidExpiration => "CodeArts 到期字段不是一致的真实 RFC3339 时间，未推测期限",
            Self::ExpiredResponse => "CodeArts 本次响应凭据在解析时已到期，未沿用旧凭据",
            Self::InvalidClock => "本次宿主时钟无法确认，未生成证明或推测响应有效期",
            Self::InvalidRandom => "本次宿主随机材料无效，未使用默认值或生成替代证明",
            Self::SigningFailed => "CodeArts 本次固定 DPoP 证明构造失败，未切换算法或重试",
            Self::InvalidHttpStatus => "CodeArts 本次 HTTP 状态无效，未从响应正文补造状态",
            Self::UpstreamRejected => "CodeArts 本次令牌响应未被确认成功，未公开错误正文或重发",
            Self::InvalidJson => "CodeArts 本次响应不是完整严格 JSON，未公开原始解析诊断",
            Self::DuplicateKey => "CodeArts 本次响应含重复字段，未选择或合并其中一份",
            Self::LimitExceeded => "CodeArts 本次协议材料超过固定限额，未接受截断材料",
            Self::InvalidResponse => "CodeArts 本次响应结构或错误字段无效，未把部分材料当作成功",
        }
    }
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}
impl std::error::Error for ProtocolError {}

type Result<T> = std::result::Result<T, ProtocolError>;

/// 无 Clone/Debug/serde；完整原材料仅用于纯校验和本次固定请求，不构造来源或许可。
/// 原三件套也必须实际完整并同义字段一致，旧访问到期不等于原刷新授权已到期。
#[must_use]
pub(super) struct RefreshMaterial {
    access_key_id: Zeroizing<String>,
    secret_access_key: Zeroizing<String>,
    security_token: Zeroizing<String>,
    refresh_token: Zeroizing<String>,
    client_id: Zeroizing<String>,
    verifier: Zeroizing<String>,
    domain_id: Zeroizing<String>,
    user_id: Zeroizing<String>,
    access_expires_at_ms: i64,
    proof_key: dpop::ProofKey,
}

impl RefreshMaterial {
    /// 只借原读对象的 secret，不接自由 JSON/账号/提供商/URL/代理参数。
    /// 未来调用者必须先取得“严格原 Ready”来源；普通 ReadStamp 不能代替该前置。
    pub(super) fn from_stamp(stamp: &ReadStamp) -> Result<Self> {
        Self::from_original_secret(stamp.secret())
    }

    fn from_original_secret(secret: &Value) -> Result<Self> {
        json::check_original(secret)?;
        let fields = object(secret, ProtocolError::InvalidMaterial)?;
        provider_field(fields)?;
        let access = required(fields, ACCESS, MAX_TOKEN_BYTES)?;
        let key = required(fields, SECRET, MAX_TOKEN_BYTES)?;
        let security = required(fields, SECURITY, MAX_TOKEN_BYTES)?;
        let refresh = required(fields, REFRESH, MAX_TOKEN_BYTES)?;
        for text in [access, key, security, refresh] {
            token(text)?;
        }
        let expiration = required(fields, EXPIRATION, 128)?;
        let expires = expiration_ms(expiration)?;
        check_millis(fields, expires)?;
        let domain = required(fields, DOMAIN, 128)?;
        let user = required(fields, USER, 128)?;
        for text in [domain, user] {
            if !text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(ProtocolError::InvalidIdentity);
            }
        }
        let context_value = fields
            .get("oauth_context")
            .ok_or(ProtocolError::MissingMaterial)?;
        let context = object(context_value, ProtocolError::InvalidClient)?;
        allowed(
            context,
            &["client_id", "pkce_pair", "dpop_key_pair"],
            ProtocolError::InvalidClient,
        )?;
        let client =
            required(context, &["client_id"], 128).map_err(|_| ProtocolError::InvalidClient)?;
        token(client).map_err(|_| ProtocolError::InvalidClient)?;
        let verifier = pkce(
            context
                .get("pkce_pair")
                .ok_or(ProtocolError::MissingMaterial)?,
        )?;
        let proof_key = dpop::ProofKey::from_pair(
            context
                .get("dpop_key_pair")
                .ok_or(ProtocolError::MissingMaterial)?,
        )?;
        // 首次登记已把完整材料写到平铺 secret；兼容容器只核对已有值，不能补缺项。
        for name in ["credential", "codearts_provider_credential", "auth"] {
            if let Some(value) = fields.get(name) {
                let legacy = object(value, ProtocolError::InvalidMaterial)?;
                provider_field(legacy)?;
                for (aliases, expected, maximum) in [
                    (ACCESS, access, MAX_TOKEN_BYTES),
                    (SECRET, key, MAX_TOKEN_BYTES),
                    (SECURITY, security, MAX_TOKEN_BYTES),
                    (REFRESH, refresh, MAX_TOKEN_BYTES),
                    (EXPIRATION, expiration, 128),
                    (DOMAIN, domain, 128),
                    (USER, user, 128),
                ] {
                    matches_optional(legacy, aliases, expected, maximum)?;
                }
                check_millis(legacy, expires)?;
                if legacy
                    .get("oauth_context")
                    .is_some_and(|value| value != context_value)
                {
                    return Err(ProtocolError::AliasConflict);
                }
            }
        }
        let identity = Zeroizing::new(format!("codearts-{domain}-{user}"));
        if identity_protected(domain, fields)
            || identity_protected(user, fields)
            || identity_protected(&identity, fields)
        {
            return Err(ProtocolError::InvalidIdentity);
        }
        Ok(Self {
            access_key_id: Zeroizing::new(access.to_string()),
            secret_access_key: Zeroizing::new(key.to_string()),
            security_token: Zeroizing::new(security.to_string()),
            refresh_token: Zeroizing::new(refresh.to_string()),
            client_id: Zeroizing::new(client.to_string()),
            verifier: Zeroizing::new(verifier.to_string()),
            domain_id: Zeroizing::new(domain.to_string()),
            user_id: Zeroizing::new(user.to_string()),
            access_expires_at_ms: expires,
            proof_key,
        })
    }

    /// 校验短期三件套的纯借用，无HMAC入口；只限未来原宿主协议所有者使用。
    pub(super) fn access_key_id(&self) -> &str {
        &self.access_key_id
    }
    pub(super) fn secret_access_key(&self) -> &str {
        &self.secret_access_key
    }
    pub(super) fn security_token(&self) -> &str {
        &self.security_token
    }
    pub(super) fn domain_id(&self) -> &str {
        &self.domain_id
    }
    pub(super) fn user_id(&self) -> &str {
        &self.user_id
    }
    pub(super) fn access_expires_at_ms(&self) -> i64 {
        self.access_expires_at_ms
    }

    /// 按值构造一次固定请求部件；旧访问凭据已到期也不推断原刷新授权到期。
    /// 时钟/jti必须由本次宿主显式提供，内部不访问环境、系统随机源或后台。
    /// 返回部件不是发送许可；调用门、Ready→Pending和禁止重放仍由未来 worker 负责。
    pub(super) fn build(self, now_ms: i64, jti: [u8; 32]) -> Result<RequestParts> {
        let jti = Zeroizing::new(jti);
        clock(now_ms)?;
        if jti.iter().all(|byte| *byte == 0) {
            return Err(ProtocolError::InvalidRandom);
        }
        // 编码最坏扩成三倍；先预留有界容量，避免旧form在扩容释放时遗留额外明文副本。
        let maximum = self.client_id.len() + self.verifier.len() + self.refresh_token.len() + 128;
        let maximum = maximum.checked_mul(3).ok_or(ProtocolError::LimitExceeded)?;
        if maximum > MAX_FORM_BYTES {
            return Err(ProtocolError::LimitExceeded);
        }
        let mut form = Zeroizing::new(String::with_capacity(maximum));
        for (name, value) in [
            ("client_id", self.client_id.as_str()),
            ("code_verifier", self.verifier.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", self.refresh_token.as_str()),
        ] {
            if !form.is_empty() {
                form.push('&');
            }
            form_encode(name, &mut form);
            form.push('=');
            form_encode(value, &mut form);
        }
        if form.len() > MAX_FORM_BYTES {
            return Err(ProtocolError::LimitExceeded);
        }
        let proof = self.proof_key.token_proof(now_ms, &jti)?;
        Ok(RequestParts { form, proof })
    }
}

/// 固定方法、固定端点和固定头；无 Clone/Debug/serde，不携带可修改的地址或代理。
/// 借出的正文/头仍是敏感材料，调用者不得记录或转交插件；应持整体至真实请求结束。
#[must_use]
pub(super) struct RequestParts {
    form: Zeroizing<String>,
    proof: Zeroizing<String>,
}

impl RequestParts {
    pub(super) fn method(&self) -> &'static str {
        "POST"
    }
    pub(super) fn uri(&self) -> &'static str {
        TOKEN_URI
    }
    pub(super) fn form(&self) -> &str {
        &self.form
    }
    pub(super) fn headers(&self) -> [(&'static str, &str); 3] {
        [
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Accept", "application/json"),
            ("DPoP", self.proof.as_str()),
        ]
    }
}

/// 无 Clone/Debug/serde；仅本次 raw 响应的五项候选，不是发送回执、成功或保存许可。
/// 不含 domain/user/OAuth，不解 JWT 补身份，不沿用原刷新令牌，不要求新串必须不同。
#[must_use]
pub(super) struct ResponseCandidate {
    credentials: json::SecretJson,
    http_status: u16,
    expires_at_ms: i64,
}

impl ResponseCandidate {
    /// 原响应完整读完后在锁外解析；状态来自宿主实读 HTTP，不从 JSON 取得。
    /// raw仍属于调用者；本类型仅尽力清理自己拥有的明文及解析中间对象。
    pub(super) fn parse(status: u16, raw: &[u8], now_ms: i64) -> Result<Self> {
        if raw.len() > MAX_RESPONSE_BYTES {
            return Err(ProtocolError::LimitExceeded);
        }
        if !(100..600).contains(&status) {
            return Err(ProtocolError::InvalidHttpStatus);
        }
        if !(200..300).contains(&status) {
            return Err(ProtocolError::UpstreamRejected);
        }
        clock(now_ms)?;
        let parsed = json::parse(raw)?;
        let root = object(parsed.value(), ProtocolError::InvalidResponse)?;
        no_error(root)?;
        let nested = match root.get("credentials") {
            Some(value) => Some(object(value, ProtocolError::InvalidResponse)?),
            None => None,
        };
        let primary = nested.unwrap_or(root);
        no_error(primary)?;
        // 嵌套形态的四项必须在同一 credentials 内完整，不与根对象拼半份三件套。
        let access = required(primary, ACCESS, MAX_TOKEN_BYTES)?;
        let secret = required(primary, SECRET, MAX_TOKEN_BYTES)?;
        let security = required(primary, SECURITY, MAX_TOKEN_BYTES)?;
        let expiration = required(primary, EXPIRATION, 128)?;
        let root_refresh = optional(root, REFRESH, MAX_TOKEN_BYTES)?;
        let nested_refresh = match nested {
            Some(fields) => optional(fields, REFRESH, MAX_TOKEN_BYTES)?,
            None => None,
        };
        if root_refresh
            .zip(nested_refresh)
            .is_some_and(|(left, right)| left != right)
        {
            return Err(ProtocolError::AliasConflict);
        }
        let refresh = root_refresh
            .or(nested_refresh)
            .ok_or(ProtocolError::MissingMaterial)?;
        for text in [access, secret, security, refresh] {
            token(text)?;
        }
        let expires = expiration_ms(expiration)?;
        check_millis(primary, expires)?;
        if nested.is_some() {
            for (aliases, expected, maximum) in [
                (ACCESS, access, MAX_TOKEN_BYTES),
                (SECRET, secret, MAX_TOKEN_BYTES),
                (SECURITY, security, MAX_TOKEN_BYTES),
                (EXPIRATION, expiration, 128),
            ] {
                matches_optional(root, aliases, expected, maximum)?;
            }
            check_millis(root, expires)?;
        }
        if expires <= now_ms {
            return Err(ProtocolError::ExpiredResponse);
        }
        let mut credentials = json::SecretJson::object();
        for (name, text) in [
            ("access_key_id", access),
            ("secret_access_key", secret),
            ("security_token", security),
            ("refresh_token", refresh),
            ("expires_at", expiration),
        ] {
            credentials.insert(name, Value::String(text.to_string()))?;
        }
        credentials.insert("expiresAt", Value::from(expires))?;
        Ok(Self {
            credentials,
            http_status: status,
            expires_at_ms: expires,
        })
    }

    /// 只借五项固定保存候选；未来原 Pending 仍须独立核对来源、响应持有与真实提交。
    pub(super) fn credentials(&self) -> &Value {
        self.credentials.value()
    }
    pub(super) fn http_status(&self) -> u16 {
        self.http_status
    }
    pub(super) fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}

fn provider_field(fields: &Map<String, Value>) -> Result<()> {
    if fields
        .get("provider")
        .is_some_and(|value| value.as_str() != Some("codearts"))
    {
        return Err(ProtocolError::InvalidMaterial);
    }
    Ok(())
}

fn identity_protected(text: &str, fields: &Map<String, Value>) -> bool {
    let contains = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .is_some_and(|secret| !secret.is_empty() && text.contains(secret))
    };
    if [ACCESS, SECRET, SECURITY, REFRESH]
        .iter()
        .any(|aliases| aliases.iter().any(|name| contains(fields.get(*name))))
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
        for name in ["d", "x", "y"] {
            if contains(
                context
                    .and_then(|value| value.get("dpop_key_pair"))
                    .and_then(|value| value.get(part))
                    .and_then(|value| value.get(name)),
            ) {
                return true;
            }
        }
    }
    false
}

fn no_error(fields: &Map<String, Value>) -> Result<()> {
    // STS/OAuth现有错误字段不作为成功材料；缺证据的业务码不猜成0或成功。
    for name in ["error", "error_description", "error_code", "error_msg"] {
        if fields.get(name).is_some_and(|value| !value.is_null()) {
            return Err(ProtocolError::UpstreamRejected);
        }
    }
    // 可选显式失败标志不能被完整凭据掩盖，也不把字符串/数字当布尔成功。
    if fields
        .get("success")
        .is_some_and(|value| value.as_bool() != Some(true))
    {
        return Err(ProtocolError::UpstreamRejected);
    }
    Ok(())
}

fn pkce(value: &Value) -> Result<&str> {
    let fields = object(value, ProtocolError::InvalidPkce)?;
    allowed(
        fields,
        &["code_verifier", "code_challenge", "code_challenge_method"],
        ProtocolError::InvalidPkce,
    )?;
    let verifier =
        required(fields, &["code_verifier"], 128).map_err(|_| ProtocolError::InvalidPkce)?;
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
    {
        return Err(ProtocolError::InvalidPkce);
    }
    let method =
        required(fields, &["code_challenge_method"], 16).map_err(|_| ProtocolError::InvalidPkce)?;
    if !matches!(method, "SHA-256" | "S256") {
        return Err(ProtocolError::InvalidPkce);
    }
    let challenge =
        required(fields, &["code_challenge"], 43).map_err(|_| ProtocolError::InvalidPkce)?;
    let decoded = dpop::decode32(challenge, ProtocolError::InvalidPkce)?;
    use sha2::{Digest, Sha256};
    let expected = Zeroizing::new(Sha256::digest(verifier.as_bytes()).to_vec());
    if decoded.as_slice() != expected.as_slice() {
        return Err(ProtocolError::InvalidPkce);
    }
    Ok(verifier)
}

fn form_encode(text: &str, output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(byte as char)
            }
            b' ' => output.push('+'),
            _ => {
                output.push('%');
                output.push(HEX[(byte >> 4) as usize] as char);
                output.push(HEX[(byte & 15) as usize] as char);
            }
        }
    }
}

fn expiration_ms(text: &str) -> Result<i64> {
    let expires = chrono::DateTime::parse_from_rfc3339(text)
        .map_err(|_| ProtocolError::InvalidExpiration)?
        .timestamp_millis();
    if expires <= 0 {
        return Err(ProtocolError::InvalidExpiration);
    }
    Ok(expires)
}

fn check_millis(fields: &Map<String, Value>, expires: i64) -> Result<()> {
    if fields
        .get("expiresAt")
        .is_some_and(|value| value.as_i64() != Some(expires))
    {
        return Err(ProtocolError::AliasConflict);
    }
    Ok(())
}

fn clock(now_ms: i64) -> Result<()> {
    if now_ms <= 0 || chrono::DateTime::from_timestamp_millis(now_ms).is_none() {
        return Err(ProtocolError::InvalidClock);
    }
    Ok(())
}

fn token(text: &str) -> Result<()> {
    if text.is_empty()
        || text.len() > MAX_TOKEN_BYTES
        || !text.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(ProtocolError::InvalidToken);
    }
    Ok(())
}

fn matches_optional(
    fields: &Map<String, Value>,
    aliases: &[&str],
    expected: &str,
    maximum: usize,
) -> Result<()> {
    if optional(fields, aliases, maximum)?.is_some_and(|text| text != expected) {
        return Err(ProtocolError::AliasConflict);
    }
    Ok(())
}

pub(self) fn required<'a>(
    fields: &'a Map<String, Value>,
    aliases: &[&str],
    maximum: usize,
) -> Result<&'a str> {
    optional(fields, aliases, maximum)?.ok_or(ProtocolError::MissingMaterial)
}

fn optional<'a>(
    fields: &'a Map<String, Value>,
    aliases: &[&str],
    maximum: usize,
) -> Result<Option<&'a str>> {
    let mut found = None;
    for name in aliases {
        if let Some(value) = fields.get(*name) {
            let text = value.as_str().ok_or(ProtocolError::InvalidToken)?;
            if text.is_empty()
                || text.len() > maximum
                || text.trim() != text
                || text.chars().any(char::is_control)
            {
                return Err(ProtocolError::InvalidToken);
            }
            if found.is_some_and(|previous| previous != text) {
                return Err(ProtocolError::AliasConflict);
            }
            found = Some(text);
        }
    }
    Ok(found)
}

pub(self) fn object(value: &Value, error: ProtocolError) -> Result<&Map<String, Value>> {
    value.as_object().ok_or(error)
}

pub(self) fn allowed(
    fields: &Map<String, Value>,
    names: &[&str],
    error: ProtocolError,
) -> Result<()> {
    if fields.keys().any(|key| !names.contains(&key.as_str())) {
        return Err(error);
    }
    Ok(())
}
