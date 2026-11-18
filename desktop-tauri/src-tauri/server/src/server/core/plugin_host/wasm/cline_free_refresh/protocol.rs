//! Free固定换证只解析本次官网响应；JWT解码不是签名验证，未知期限不补默认值。

use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::unavailable;

pub(super) const URL: &str = "https://api.cline.bot/api/v1/auth/refresh";
pub(super) const MAX_RESPONSE: usize = 128 * 1024;
const MAX_TOKEN: usize = 8192;

pub(super) fn request(secret: &Value) -> Result<Zeroizing<String>, proto::WireError> {
    let refresh = tokens(secret, &["refreshToken", "refresh_token"], false)?;
    Ok(Zeroizing::new(json!({"refreshToken": refresh.as_str(), "grantType": "refresh_token"}).to_string()))
}

pub(in crate::server::core::plugin_host::wasm) fn replacement(bytes: &[u8], uid: &str, previous_public: &Value) -> Result<(SecretValue, Value), proto::WireError> {
    let value: Value = super::super::managed_task::declaration::parse_object(bytes, MAX_RESPONSE).map_err(|_| unavailable())?;
    let value = SecretValue(value);
    envelope(&value.0)?;
    let data = value.0.get("data").unwrap_or(&value.0);
    envelope(data)?;
    for key in ["tokenType", "token_type"] {
        if data.get(key).is_some_and(|value| value.as_str().is_none_or(|value| !value.eq_ignore_ascii_case("Bearer"))) {
            return Err(unavailable());
        }
    }
    let access = tokens(data, &["accessToken", "access_token", "token"], true)?;
    let refresh = tokens(data, &["refreshToken", "refresh_token"], false)?;
    let claims = claims(&access)?;
    let declared = identity(&claims.0, &["external_id"])?;
    let info = data.get("userInfo").map(|info| identity(info, &["clineUserId", "external_id"])).transpose()?.flatten();
    if declared.as_deref().is_some_and(|value| value != uid) || info.as_deref().is_some_and(|value| value != uid)
        || declared.is_none() && info.is_none() { return Err(unavailable()); }
    let jwt = claims.0.get("exp").map(|value| value.as_i64().filter(|seconds| *seconds > 0)
        .and_then(|seconds| seconds.checked_mul(1000)).ok_or_else(unavailable)).transpose()?;
    let expiry = expiration(data.get("expiresAt"))?;
    let expires = match (jwt, expiry) { (Some(jwt), Some(expiry)) => Some(jwt.min(expiry)), (jwt, expiry) => jwt.or(expiry) };
    let now = crate::server::logging::now_ms();
    if now <= 0 || expires.is_some_and(|expires| expires <= now) { return Err(unavailable()); }
    let secret = SecretValue(json!({"accessToken": access.as_str(), "refreshToken": refresh.as_str(), "account": uid, "expiresAt": expires}));
    let name = previous_public.get("displayName").or_else(|| previous_public.get("name")).and_then(Value::as_str)
        .filter(|value| value.len() <= 400 && !value.chars().any(char::is_control)).unwrap_or(uid);
    let tail = access.chars().rev().take(6).collect::<String>().chars().rev().collect::<String>();
    let tail = if protected(&tail, &access, &refresh) { "" } else { &tail };
    let name = if protected(name, &access, &refresh) { "" } else { name };
    let public = json!({"uid": uid, "account": uid, "name": name, "displayName": name, "expiresAt": expires,
        "hasAccessToken": true, "hasRefreshToken": true, "canRefresh": true, "desktopAuth": false,
        "tokenTail": tail, "upstreamValidated": true, "validation": "host_managed_refresh", "refreshStatus": "host_managed"});
    super::super::public_view::validate(&public, &secret.0)?;
    Ok((secret, public))
}

fn envelope(value: &Value) -> Result<(), proto::WireError> {
    if !value.is_object() || value["type"] == "error"
        || value.get("success").is_some_and(|value| value.as_bool() != Some(true))
        || value.get("error").is_some_and(|value| !value.is_null() && value.as_str() != Some("")) { return Err(unavailable()); }
    Ok(())
}

fn tokens(value: &Value, keys: &[&str], access: bool) -> Result<Zeroizing<String>, proto::WireError> {
    let mut found: Option<Zeroizing<String>> = None;
    for key in keys {
        if let Some(value) = value.get(*key) {
            let text = value.as_str().filter(|text| valid_token(text)).ok_or_else(unavailable)?;
            let normalized = if access {
                let raw = text.strip_prefix("workos:").unwrap_or(text);
                if raw.is_empty() || raw.starts_with("workos:") { return Err(unavailable()); }
                Zeroizing::new(format!("workos:{raw}"))
            } else { Zeroizing::new(text.to_string()) };
            if normalized.len() > MAX_TOKEN || found.as_ref().is_some_and(|previous| previous.as_str() != normalized.as_str()) {
                return Err(unavailable());
            }
            found = Some(normalized);
        }
    }
    found.ok_or_else(unavailable)
}

fn claims(token: &str) -> Result<SecretValue, proto::WireError> {
    let raw = token.strip_prefix("workos:").unwrap_or(token);
    if !raw.contains('.') { return Ok(SecretValue(Value::Null)); }
    let parts = raw.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) { return Err(unavailable()); }
    let encoded = parts[1].trim_end_matches('=');
    if parts[1].len() - encoded.len() > 2 { return Err(unavailable()); }
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded).map_err(|_| unavailable())?);
    let value = super::super::managed_task::declaration::parse_object::<Value>(&bytes, MAX_RESPONSE).map_err(|_| unavailable())?;
    if !value.is_object() { return Err(unavailable()); }
    Ok(SecretValue(value))
}

fn identity(value: &Value, keys: &[&str]) -> Result<Option<String>, proto::WireError> {
    if value.is_null() { return Ok(None); }
    if !value.is_object() { return Err(unavailable()); }
    let mut found = None;
    for key in keys {
        if let Some(value) = value.get(*key) {
            let uid = value.as_str().filter(|text| !text.is_empty() && text.len() <= 512 && text.trim() == *text
                && !text.chars().any(char::is_control) && !text.contains(['/', '\\'])).ok_or_else(unavailable)?;
            if found.as_deref().is_some_and(|previous| previous != uid) { return Err(unavailable()); }
            found = Some(uid.to_string());
        }
    }
    Ok(found)
}

fn expiration(value: Option<&Value>) -> Result<Option<i64>, proto::WireError> {
    let Some(value) = value.filter(|value| !value.is_null()) else { return Ok(None); };
    let expiry = match value {
        Value::Number(value) => value.as_i64(),
        Value::String(text) if text.len() <= 64 && text.trim() == text && !text.chars().any(char::is_control) =>
            text.parse().ok().or_else(|| chrono::DateTime::parse_from_rfc3339(text).ok().map(|value| value.timestamp_millis())),
        _ => None,
    };
    expiry.filter(|value| *value > 0).map(Some).ok_or_else(unavailable)
}

fn valid_token(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_TOKEN && text.is_ascii()
        && !text.bytes().any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
}
fn protected(text: &str, access: &str, refresh: &str) -> bool {
    !text.is_empty() && [access, access.strip_prefix("workos:").unwrap_or(access), refresh].iter()
        .any(|token| !token.is_empty() && text.contains(token))
}

pub(in crate::server::core::plugin_host::wasm) struct SecretValue(pub(in crate::server::core::plugin_host::wasm) Value);
impl Drop for SecretValue {
    fn drop(&mut self) { wipe(&mut self.0); }
}
fn wipe(value: &mut Value) {
    use zeroize::Zeroize;
    match value {
        Value::String(text) => text.zeroize(),
        Value::Object(fields) => {
            for (mut key, mut value) in std::mem::take(fields) { key.zeroize(); wipe(&mut value); }
        },
        Value::Array(items) => items.iter_mut().for_each(wipe),
        _ => {},
    }
}
