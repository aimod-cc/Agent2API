//! 固定CodeArts授权投影与允许改动。完整pluginSecret（未知字段也算）参与比较；
//! 顶层兼容凭据、身份来源另存一份，避免旧原生写者只改顶层却被忽略。
//! 无provider注册表/网络/凭据有效期推测；只校验存储形状与精确内容。

use serde_json::{Map, Value};

use super::{bounded, Error};
use crate::server::core::account_store::{credential_version, plugin_accounts, plugin_public, state::StoredAccount};

pub(super) const PROVIDER: &str = "codearts";

// 来源：原生CodeArts credentials.rs::from_payload/to_record_value及存量身份/公开投影。
const MATERIAL_KEYS: &[&str] = &[
    "access_key_id", "secret_access_key", "security_token", "expires_at", "expiration",
    "refresh_token", "oauth_context", "accessToken", "refreshToken", "expiresAt",
    "domain_id", "account_id", "user_id", "principal_id", "user_name", "principal_urn", "login_type",
    "domainId", "userId", "userName", "loginType", "accessKeyId", "secretAccessKey", "securityToken",
];
const CONTAINERS: &[&str] = &["codearts_provider_credential", "credential", "auth"];
const IDENTITY_KEYS: &[&str] = &[
    "domain_id", "account_id", "user_id", "principal_id", "user_name", "principal_urn", "login_type",
    "domainId", "userId", "userName", "loginType", "uid", "desktop", "pluginIdentity", "pluginIdentityContract",
    "needsRelogin", "needsReloginReason", "userAgent",
];
const WRITABLE_KEYS: &[&str] = &[
    "access_key_id", "secret_access_key", "security_token", "expires_at", "expiration",
    "refresh_token", "oauth_context", "accessToken", "refreshToken", "expiresAt",
    "accessKeyId", "secretAccessKey", "securityToken",
];
const PUBLIC_KEYS: &[&str] = &[
    "tokenTail", "hasAccessToken", "hasRefreshToken", "hasToken", "hasCredentials", "canRefresh", "available",
    "expiresAt", "expiresAtMs", "tokenExpireAt", "tokenExpiresAt", "accessTokenExpiresAt", "refreshTokenExpiresAt",
    "needsRelogin", "needsReloginReason",
];

pub(super) struct Projection {
    // None与显式对象严格不同；从原生结构迁为pluginSecret也必须推进代次。
    plugin: Option<Value>,
    native: Value,
    identity: Value,
    public_identity: Value,
    secret: Value,
}

impl PartialEq for Projection {
    fn eq(&self, other: &Self) -> bool {
        // secret是兼容底稿，不把其中旧原生行的运营字段额外纳入版本条件。
        self.plugin == other.plugin && self.native == other.native
            && self.identity == other.identity && self.public_identity == other.public_identity
    }
}

impl Projection {
    pub(super) fn read(record: &StoredAccount) -> Result<Self, Error> {
        let fields = record.fields();
        let plugin = match fields.get("pluginSecret").filter(|value| !value.is_null()) {
            Some(value) if value.is_object() => {
                bounded::check(value, bounded::MAX_SECRET_BYTES)?;
                reject_reserved(value)?;
                Some(value.clone())
            }
            Some(_) => return Err(Error::InvalidRecord),
            None => None,
        };
        let mut native = select(fields, MATERIAL_KEYS);
        for name in CONTAINERS {
            if let Some(value) = fields.get(*name) {
                native.insert((*name).to_string(), value.clone());
            }
        }
        let native = Value::Object(native);
        bounded::check(&native, bounded::MAX_SECRET_BYTES)?;
        reject_reserved(&native)?;
        let identity = Value::Object(select(fields, IDENTITY_KEYS));
        let public_identity = match fields.get("pluginPublic").filter(|value| !value.is_null()) {
            Some(Value::Object(public)) => Value::Object(select(public, IDENTITY_KEYS)),
            Some(_) => return Err(Error::InvalidRecord),
            None => Value::Null,
        };
        // 旧原生行只借出固定授权投影；其余未知字段留在事务内原记录，绝不整行回灌。
        let secret = plugin.as_ref().unwrap_or(&native).clone();
        Ok(Self { plugin, native, identity, public_identity, secret })
    }

    pub(super) fn secret(&self) -> &Value {
        &self.secret
    }
}

/// 候选值本身不带任何许可；公开构造只做有界形状校验，授权来自外层ReadStamp。
pub(in crate::server::core::account_store) struct Change {
    credentials: Map<String, Value>,
    public: Map<String, Value>,
}

impl Change {
    pub(in crate::server::core::account_store) fn new(credentials: &Value, public: &Value) -> Result<Self, Error> {
        bounded::check(credentials, bounded::MAX_SECRET_BYTES)?;
        bounded::check(public, bounded::MAX_PUBLIC_BYTES)?;
        reject_reserved(credentials)?;
        reject_reserved(public)?;
        let credentials = credentials.as_object().ok_or(Error::InvalidInput)?;
        let public = public.as_object().ok_or(Error::InvalidInput)?;
        if credentials.len() + public.len() == 0 || credentials.keys().any(|key| !WRITABLE_KEYS.contains(&key.as_str())) {
            return Err(Error::InvalidInput);
        }
        for (key, value) in credentials {
            let valid = if key == "oauth_context" {
                value.is_null() || value.is_object()
            } else if key == "expiresAt" {
                value.is_null() || value.as_i64().is_some_and(|value| value > 0)
            } else {
                value.is_null() || value.as_str().is_some_and(|value| value.len() <= 32 * 1024 && !value.contains('\0'))
            };
            if !valid { return Err(Error::InvalidInput); }
        }
        for (key, value) in public {
            if !PUBLIC_KEYS.contains(&key.as_str()) || !public_scalar(key, value) {
                return Err(Error::InvalidInput);
            }
        }
        let mut credentials = credentials.clone();
        align_aliases(&mut credentials)?;
        Ok(Self { credentials, public: public.clone() })
    }

    pub(super) fn apply(self, record: &mut StoredAccount) -> Result<(), Error> {
        let before = Projection::read(record)?;
        let mut next = before.secret.as_object().cloned().ok_or(Error::InvalidRecord)?;
        // 只覆盖明确列出的CodeArts字段；未知secret内容取最新原行，不由候选整份替换。
        for (key, value) in &self.credentials {
            next.insert(key.clone(), value.clone());
        }
        // 固定兼容容器中已经存在的材料同步更新，避免下次原生解析仍读到旧嵌套令牌。
        for container in CONTAINERS {
            if let Some(Value::Object(fields)) = next.get_mut(*container) {
                for (key, value) in &self.credentials {
                    if *container != "auth" || fields.contains_key(key) { fields.insert(key.clone(), value.clone()); }
                }
            } else if next.contains_key(*container) && *container != "auth" {
                return Err(Error::InvalidRecord);
            }
        }
        let next = Value::Object(next);
        bounded::check(&next, bounded::MAX_SECRET_BYTES)?;
        let mut public = match record.get("pluginPublic").filter(|value| !value.is_null()) {
            Some(Value::Object(fields)) => fields.clone(),
            Some(_) => return Err(Error::InvalidRecord),
            None => Map::new(),
        };
        for (name, value) in &self.public { public.insert(name.clone(), value.clone()); }
        let plugin_backed = before.plugin.is_some();
        // 只在原行已有的旧原生形态更新材料；不凭空创建pluginSecret切断旧字段读取。
        for (key, value) in &self.credentials {
            if !plugin_backed || record.get(key).is_some() { record.set(key, value.clone()); }
        }
        for container in CONTAINERS {
            if let Some(Value::Object(mut fields)) = record.get(container).cloned() {
                for (key, value) in &self.credentials {
                    if *container != "auth" || fields.contains_key(key) { fields.insert(key.clone(), value.clone()); }
                }
                record.set(container, Value::Object(fields));
            } else if record.get(container).is_some() && *container != "auth" {
                return Err(Error::InvalidRecord);
            }
        }
        if plugin_backed { record.set("pluginSecret", next.clone()); }
        plugin_accounts::credential_version_mirror(record, &next);
        plugin_public::credential_fields(record, &mut public);
        // 老公开投影可能保留了旧的较大到期值；本合同只接受本次真实给出的值，不取max。
        if let Some(expiry) = self.credentials.get("expiresAt") {
            for name in ["expiresAt", "expiresAtMs", "tokenExpireAt", "tokenExpiresAt", "accessTokenExpiresAt"] {
                if public.contains_key(name) || name == "expiresAt" { public.insert(name.to_string(), expiry.clone()); }
            }
        }
        public.retain(|key, _| !plugin_public::is_private_field(key));
        let public = Value::Object(public);
        bounded::check(&public, bounded::MAX_PUBLIC_BYTES)?;
        validate_public(&public, &next, &before.native, &self.public)?;
        record.set("pluginPublic", public);
        // 登录状态是明确允许的必要标量，不让调用者借此改启停/名称/代理等属性。
        for name in ["needsRelogin", "needsReloginReason"] {
            if let Some(value) = self.public.get(name) { record.set(name, value.clone()); }
        }
        Ok(())
    }
}

fn align_aliases(fields: &mut Map<String, Value>) -> Result<(), Error> {
    // 与原生to_record_value及旧MIRROR_KEYS一致，短期材料的各拼法不能留两套不同值。
    for group in [
        &["access_key_id", "accessToken", "accessKeyId"][..],
        &["secret_access_key", "secretAccessKey"][..],
        &["security_token", "securityToken"][..],
        &["refresh_token", "refreshToken"][..],
        &["expires_at", "expiration"][..],
    ] {
        let Some(value) = group.iter().find_map(|key| fields.get(*key)).cloned() else { continue };
        if group.iter().any(|key| fields.get(*key).is_some_and(|other| other != &value)) {
            return Err(Error::InvalidInput);
        }
        for key in group { fields.insert((*key).to_string(), value.clone()); }
    }
    // 不把未知/非法有效期写成0。ISO字段与通用毫秒字段同时给出时必须同源一致。
    if let Some(value) = fields.get("expires_at") {
        let expires = match value {
            Value::Null => Value::Null,
            Value::String(text) if text.is_empty() => Value::Null,
            Value::String(text) => {
                let at = chrono::DateTime::parse_from_rfc3339(text).map_err(|_| Error::InvalidInput)?
                    .timestamp_millis();
                if at <= 0 { return Err(Error::InvalidInput); }
                Value::from(at)
            }
            _ => return Err(Error::InvalidInput),
        };
        if fields.get("expiresAt").is_some_and(|other| other != &expires) {
            return Err(Error::InvalidInput);
        }
        fields.insert("expiresAt".to_string(), expires);
    } else if let Some(value) = fields.get("expiresAt").cloned() {
        let native = match value.as_i64() {
            Some(at) => Value::String(chrono::DateTime::from_timestamp_millis(at)
                .ok_or(Error::InvalidInput)?.to_rfc3339()),
            None if value.is_null() => Value::Null,
            _ => return Err(Error::InvalidInput),
        };
        fields.insert("expires_at".to_string(), native.clone());
        fields.insert("expiration".to_string(), native);
    }
    bounded::check(&Value::Object(fields.clone()), bounded::MAX_SECRET_BYTES)
}

fn select(fields: &Map<String, Value>, names: &[&str]) -> Map<String, Value> {
    names.iter().filter_map(|name| fields.get(*name).map(|value| ((*name).to_string(), value.clone()))).collect()
}

pub(super) fn reject_reserved(value: &Value) -> Result<(), Error> {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if credential_version::private_field(key) { return Err(Error::InvalidInput); }
                reject_reserved(value)?;
            }
        }
        Value::Array(items) => for item in items { reject_reserved(item)?; },
        _ => {}
    }
    Ok(())
}

fn public_scalar(key: &str, value: &Value) -> bool {
    match key {
        "hasAccessToken" | "hasRefreshToken" | "hasToken" | "hasCredentials" | "canRefresh" | "available" | "needsRelogin" => value.is_boolean(),
        "expiresAt" | "expiresAtMs" | "tokenExpireAt" | "tokenExpiresAt" | "accessTokenExpiresAt" | "refreshTokenExpiresAt" =>
            value.is_null() || value.as_i64().is_some_and(|value| value > 0),
        "tokenTail" => value.is_null() || value.as_str().is_some_and(|value|
            value.chars().count() <= 6 && !value.chars().any(char::is_control)),
        _ => value.is_null() || value.as_str().is_some_and(|value| value.len() <= 2048 && !value.chars().any(char::is_control)),
    }
}

fn sensitive_name(key: &str) -> bool {
    let name = key.to_ascii_lowercase().replace(['_', '-'], "");
    name.contains("token") || name.contains("password") || name.contains("secret")
        || name.contains("cookie") || name.contains("authorization") || name.contains("privatekey")
        || name == "oauthcontext" || name == "auth" || name == "key" || name == "accesskeyid"
        || name == "codeverifier" || name == "d"
}

fn validate_public(public: &Value, secret: &Value, native: &Value, changed: &Map<String, Value>) -> Result<(), Error> {
    fn add<'a>(text: &'a str, secrets: &mut Vec<&'a str>) -> bool {
        if secrets.len() >= 4096 { return false; }
        secrets.push(text);
        true
    }
    fn collect<'a>(value: &'a Value, protected: bool, secrets: &mut Vec<&'a str>) -> bool {
        match value {
            Value::Object(fields) => fields.iter().all(|(key, value)|
                collect(value, protected || sensitive_name(key), secrets)),
            Value::Array(items) => items.iter().all(|value| collect(value, protected, secrets)),
            Value::String(text) if protected && !text.is_empty() => add(text, secrets),
            _ => true,
        }
    }
    fn check(value: &Value, secrets: &[&str]) -> bool {
        match value {
            Value::Object(fields) => fields.iter().all(|(key, value)| {
                if credential_version::private_field(key) { return false; }
                if sensitive_name(key) {
                    return PUBLIC_KEYS.contains(&key.as_str()) && public_scalar(key, value)
                        && check(value, secrets);
                }
                check(value, secrets)
            }),
            Value::Array(items) => items.iter().all(|value| check(value, secrets)),
            Value::String(text) => !secrets.iter().any(|secret| text.contains(secret)),
            _ => true,
        }
    }
    // 未知secret字段也属于凭据；不能借任意展示字段回显长字符串。
    fn collect_all<'a>(value: &'a Value, out: &mut Vec<&'a str>) -> bool {
        match value {
            Value::Object(fields) => fields.values().all(|value| collect_all(value, out)),
            Value::Array(items) => items.iter().all(|value| collect_all(value, out)),
            Value::String(text) if text.len() >= 8 => add(text, out),
            _ => true,
        }
    }
    let mut secrets = Vec::new();
    if !collect(secret, false, &mut secrets) || !collect(native, false, &mut secrets) {
        return Err(Error::LimitExceeded);
    }
    if !check(public, &secrets) { return Err(Error::InvalidInput); }
    // 未知字符串只校验本次允许变更的公开字段，不把未修改的旧账号显示名误当新泄漏。
    secrets.clear();
    if !collect_all(secret, &mut secrets) || !collect_all(native, &mut secrets) {
        return Err(Error::LimitExceeded);
    }
    if !changed.values().all(|value| check(value, &secrets)) {
        return Err(Error::InvalidInput);
    }
    Ok(())
}
