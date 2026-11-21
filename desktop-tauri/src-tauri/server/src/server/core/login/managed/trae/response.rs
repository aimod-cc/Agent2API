//! Trae 换码/GetUserInfo的严格读数：根业务码显式0、全层重复字段拒绝。
//! 只解析已证Result/字段，不递归找token/UID、不从回调或JWT制造身份。

use std::collections::BTreeSet;
use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use zeroize::{Zeroize, Zeroizing};

use super::{context::Variant, network::Kind};
use crate::server::errors::GatewayError;

pub(super) struct SecretJson(Value);
impl SecretJson {
    pub(super) fn new(value: Value) -> Self {
        Self(value)
    }
    pub(super) fn value(&self) -> &Value {
        &self.0
    }
    pub(super) fn value_mut(&mut self) -> &mut Value {
        &mut self.0
    }
    pub(super) fn into_value(mut self) -> Value {
        std::mem::take(&mut self.0)
    }
}
impl Drop for SecretJson {
    fn drop(&mut self) {
        erase(&mut self.0);
    }
}

pub(super) fn erase(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Object(fields) => {
            for child in fields.values_mut() {
                erase(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                erase(child);
            }
        }
        _ => {}
    }
}

pub(super) fn valid_token(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 16384
        && text.is_ascii()
        && !text
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
}

pub(super) struct Tokens {
    pub(super) access: Zeroizing<String>,
    pub(super) refresh: Option<Zeroizing<String>>,
    pub(super) expires_at: Option<i64>,
    pub(super) uid_hint: Option<String>,
}

pub(super) struct Profile {
    pub(super) uid: String,
    pub(super) nickname: Option<String>,
    pub(super) enterprise: Option<String>,
}

pub(super) fn decode(bytes: &[u8], kind: Kind) -> Result<SecretJson, GatewayError> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    Shape { depth: 0 }
        .deserialize(&mut decoder)
        .map_err(|_| kind.error())?;
    decoder.end().map_err(|_| kind.error())?;
    let value = SecretJson::new(serde_json::from_slice(bytes).map_err(|_| kind.error())?);
    business(value.value(), kind)?;
    Ok(value)
}

fn business(value: &Value, kind: Kind) -> Result<(), GatewayError> {
    let root = value.as_object().ok_or_else(|| kind.error())?;
    let mut code = None;
    for field in ["Code", "code"] {
        if let Some(value) = root.get(field).filter(|value| !value.is_null()) {
            let next = integer(value).ok_or_else(|| kind.error())?;
            if next != 0 || code.is_some_and(|old| old != next) {
                return Err(kind.error());
            }
            code = Some(next);
        }
    }
    if code.is_some_and(|code| code != 0)
        || (code.is_none()
            && !root.contains_key("Result")
            && !root.contains_key("result")
            && !root.contains_key("ResponseMetadata"))
    {
        return Err(kind.error());
    }
    for field in ["success", "Success"] {
        if root
            .get(field)
            .filter(|value| !value.is_null())
            .is_some_and(|value| value != &Value::Bool(true))
        {
            return Err(kind.error());
        }
    }
    for field in ["error", "Error", "__Message.error", "upstreamCode"] {
        if root.get(field).is_some_and(|value| {
            !value.is_null() && value != &Value::Bool(false) && value.as_str() != Some("")
        }) {
            return Err(kind.error());
        }
    }
    if let Some(metadata) = root.get("ResponseMetadata") {
        let metadata = metadata.as_object().ok_or_else(|| kind.error())?;
        if metadata.get("Error").is_some_and(|error| !error.is_null()) {
            return Err(kind.error());
        }
    }
    Ok(())
}

fn result<'a>(
    root: &'a Value,
    required: bool,
    kind: Kind,
) -> Result<&'a Map<String, Value>, GatewayError> {
    let root = root.as_object().ok_or_else(|| kind.error())?;
    let mut found: Option<&Value> = None;
    for field in ["Result", "result"] {
        if let Some(value) = root.get(field).filter(|value| !value.is_null()) {
            if !value.is_object() || found.is_some_and(|old| old != value) {
                return Err(kind.error());
            }
            found = Some(value);
        }
    }
    match found {
        Some(value) => {
            let fields = value.as_object().ok_or_else(|| kind.error())?;
            for field in ["Code", "code"] {
                if fields
                    .get(field)
                    .filter(|value| !value.is_null())
                    .is_some_and(|value| integer(value) != Some(0))
                {
                    return Err(kind.error());
                }
            }
            for field in ["success", "Success"] {
                if fields
                    .get(field)
                    .filter(|value| !value.is_null())
                    .is_some_and(|value| value != &Value::Bool(true))
                {
                    return Err(kind.error());
                }
            }
            for field in ["error", "Error", "__Message.error"] {
                if fields.get(field).is_some_and(|value| {
                    !value.is_null() && value != &Value::Bool(false) && value.as_str() != Some("")
                }) {
                    return Err(kind.error());
                }
            }
            Ok(fields)
        }
        None if !required => Ok(root),
        None => Err(kind.error()),
    }
}

fn valid_cn_client_id(client: &str, expected: &str) -> bool {
    client == expected || matches!(client, "ono9krqynydwx5" | "en1oxy7wnw8j9n")
}

pub(super) fn tokens(root: &Value, variant: Variant) -> Result<Tokens, GatewayError> {
    let kind = Kind::Exchange;
    let fields = result(root, true, kind)?;
    let access = text(
        fields,
        &["Token", "token", "AccessToken", "accessToken"],
        16384,
        true,
        kind,
    )?
    .filter(|token| valid_token(token))
    .ok_or_else(|| kind.error())?;
    let refresh = text_optional_token(
        fields,
        &["RefreshToken", "refreshToken", "refresh_token"],
        kind,
    )?;
    if refresh.as_ref().is_some_and(|token| !valid_token(token)) {
        return Err(kind.error());
    }
    let expected = variant.client_id();
    for source in [root.as_object().ok_or_else(|| kind.error())?, fields] {
        if text(
            source,
            &["ClientID", "clientId", "client_id"],
            256,
            true,
            kind,
        )?
        .is_some_and(|client| !valid_cn_client_id(&client, expected))
        {
            return Err(kind.error());
        }
    }
    let mut uid_hint = uid(fields, kind)?;
    if let Some(root_uid) = uid(root.as_object().ok_or_else(|| kind.error())?, kind)? {
        if uid_hint.as_ref().is_some_and(|old| old != &root_uid) {
            return Err(kind.error());
        }
        uid_hint = Some(root_uid);
    }
    let mut expires_at = None;
    for field in ["TokenExpireAt", "tokenExpireAt", "ExpiresAt", "expiresAt"] {
        if let Some(value) = fields.get(field).filter(|value| !value.is_null()) {
            let Some(mut next) = integer(value).filter(|value| *value > 0) else {
                continue;
            };
            if next < 100_000_000_000 {
                next = next.checked_mul(1000).ok_or_else(|| kind.error())?;
            }
            if next >= (1_i64 << 53)
                || chrono::DateTime::from_timestamp_millis(next).is_none()
                || expires_at.is_some_and(|old| old != next)
            {
                return Err(kind.error());
            }
            expires_at = Some(next);
        }
    }
    if expires_at.is_none() {
        for field in [
            "TokenExpireDuration",
            "tokenExpireDuration",
            "ExpiresIn",
            "expiresIn",
        ] {
            if let Some(value) = fields.get(field).filter(|value| !value.is_null()) {
                let Some(duration) = integer(value).filter(|value| *value > 0) else {
                    continue;
                };
                let delta_ms = if duration > 1_000_000_000 {
                    duration
                } else {
                    duration.saturating_mul(1000)
                };
                let next = chrono::Utc::now().timestamp_millis().saturating_add(delta_ms);
                if next < (1_i64 << 53) && chrono::DateTime::from_timestamp_millis(next).is_some() {
                    expires_at = Some(next);
                    break;
                }
            }
        }
    }
    // 根层同名材料不能绕过Result提供另一个token/期限；非空冲突一律拒绝。
    for names in [
        &["Token", "token", "AccessToken", "accessToken"][..],
        &["RefreshToken", "refreshToken", "refresh_token"][..],
        &["TokenExpireAt", "tokenExpireAt", "ExpiresAt", "expiresAt"][..],
    ] {
        for name in names {
            if root.get(*name).is_some_and(|value| !value.is_null()) {
                return Err(kind.error());
            }
        }
    }
    Ok(Tokens {
        access: Zeroizing::new(access),
        refresh: refresh.map(Zeroizing::new),
        expires_at,
        uid_hint,
    })
}

pub(super) fn profile(
    root: &Value,
    variant: Variant,
    tokens: &Tokens,
) -> Result<Profile, GatewayError> {
    let kind = Kind::Profile;
    let fields = result(root, false, kind)?;
    let uid = uid(fields, kind)?.ok_or_else(|| kind.error())?;
    let root_fields = root.as_object().ok_or_else(|| kind.error())?;
    if uid_field(root_fields, kind)?.is_some_and(|root_uid| root_uid != uid)
        || tokens.uid_hint.as_ref().is_some_and(|hint| hint != &uid)
    {
        return Err(kind.error());
    }
    for source in [root_fields, fields] {
        if text(
            source,
            &["ClientID", "clientId", "client_id"],
            256,
            true,
            kind,
        )?
        .is_some_and(|client| !valid_cn_client_id(&client, variant.client_id()))
        {
            return Err(kind.error());
        }
        if text(
            source,
            &["Token", "token", "AccessToken", "accessToken"],
            16384,
            true,
            kind,
        )?
        .is_some_and(|token| token != tokens.access.as_str())
        {
            return Err(kind.error());
        }
        if text_optional_token(
            source,
            &["RefreshToken", "refreshToken", "refresh_token"],
            kind,
        )?
        .is_some_and(|token| tokens.refresh.as_deref().map(String::as_str) != Some(token.as_str()))
        {
            return Err(kind.error());
        }
    }
    let nickname = display_text(
        fields,
        &[
            "ScreenName",
            "screenName",
            "Nickname",
            "nickname",
            "Name",
            "name",
            "displayName",
        ],
        480,
        kind,
    )?;
    let enterprise = display_text(
        fields,
        &["EnterpriseID", "enterpriseId", "TenantID", "tenantId"],
        480,
        kind,
    )?;
    Ok(Profile {
        uid,
        nickname,
        enterprise,
    })
}

const UID_FIELDS: &[&str] = &["UserID", "userId", "uid", "UID", "user_id"];
fn uid(fields: &Map<String, Value>, kind: Kind) -> Result<Option<String>, GatewayError> {
    uid_field(fields, kind)
}
fn uid_field(fields: &Map<String, Value>, kind: Kind) -> Result<Option<String>, GatewayError> {
    let mut found = None;
    for field in UID_FIELDS {
        if let Some(value) = fields.get(*field) {
            if value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty()) {
                continue;
            }
            let next = match value {
                Value::String(text) => text.trim().to_string(),
                Value::Number(number) => {
                    let Some(value) = number.as_u64() else {
                        return Err(kind.error());
                    };
                    if value == 0 {
                        continue;
                    }
                    value.to_string()
                }
                _ => return Err(kind.error()),
            };
            if next.len() > 480
                || next.chars().any(|ch| ch.is_control() || ch.is_whitespace())
                || matches!(
                    next.to_ascii_lowercase().as_str(),
                    "unknown" | "unknown-cn" | "solo-unknown" | "unknown-intl" | "0"
                )
                || found.as_ref().is_some_and(|old| old != &next)
            {
                return Err(kind.error());
            }
            found = Some(next);
        }
    }
    Ok(found)
}

fn text(
    fields: &Map<String, Value>,
    names: &[&str],
    limit: usize,
    header: bool,
    kind: Kind,
) -> Result<Option<String>, GatewayError> {
    let mut found = None;
    for name in names {
        if let Some(value) = fields.get(*name) {
            if value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty()) {
                continue;
            }
            let next = value
                .as_str()
                .filter(|text| {
                    !text.is_empty()
                        && text.len() <= limit
                        && text.trim() == *text
                        && !text.chars().any(char::is_control)
                        && (!header
                            || text.is_ascii()
                                && !text.bytes().any(|byte| byte.is_ascii_whitespace()))
                })
                .ok_or_else(|| kind.error())?;
            if found.as_ref().is_some_and(|old: &String| old != next) {
                return Err(kind.error());
            }
            found = Some(next.to_string());
        }
    }
    Ok(found)
}

fn text_optional_token(
    fields: &Map<String, Value>,
    names: &[&str],
    kind: Kind,
) -> Result<Option<String>, GatewayError> {
    let mut found = None;
    for name in names {
        if let Some(value) = fields.get(*name) {
            if value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty()) {
                continue;
            }
            let token = value
                .as_str()
                .filter(|text| valid_token(text))
                .ok_or_else(|| kind.error())?;
            if found.as_ref().is_some_and(|old: &String| old != token) {
                return Err(kind.error());
            }
            found = Some(token.to_string());
        }
    }
    Ok(found)
}

// 昵称/租户只是可选显示读数，空或null保留未知；不影响UID的严格必需门。
fn display_text(
    fields: &Map<String, Value>,
    names: &[&str],
    limit: usize,
    _kind: Kind,
) -> Result<Option<String>, GatewayError> {
    for name in names {
        if let Some(value) = fields.get(*name) {
            let next = match value {
                Value::String(text) => {
                    let trimmed = text.trim();
                    if trimmed.is_empty()
                        || trimmed == "0"
                        || trimmed.len() > limit
                        || trimmed.chars().any(char::is_control)
                    {
                        continue;
                    }
                    trimmed.to_string()
                }
                Value::Number(number) => {
                    let Some(value) = number.as_u64().filter(|value| *value > 0) else {
                        continue;
                    };
                    value.to_string()
                }
                _ => continue,
            };
            return Ok(Some(next));
        }
    }
    Ok(None)
}

fn integer(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|n| i64::try_from(n).ok()))
            .or_else(|| number.as_f64().map(|n| n as i64)),
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.parse().ok()).flatten()
        }
        _ => None,
    }
}

struct Shape {
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for Shape {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        decoder.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Shape {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded Trae JSON with unique keys")
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_string<E: de::Error>(self, mut text: String) -> Result<(), E> {
        text.zeroize();
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        if self.depth >= 16 {
            return Err(de::Error::custom("Trae JSON depth exceeded"));
        }
        while sequence
            .next_element_seed(Shape {
                depth: self.depth + 1,
            })?
            .is_some()
        {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<(), A::Error> {
        if self.depth >= 16 {
            return Err(de::Error::custom("Trae JSON depth exceeded"));
        }
        let mut seen = BTreeSet::new();
        while let Some(key) = fields.next_key::<String>()? {
            if key.len() > 256 || seen.len() >= 1024 || !seen.insert(key) {
                return Err(de::Error::custom(
                    "Trae JSON field repeated or exceeded limit",
                ));
            }
            fields.next_value_seed(Shape {
                depth: self.depth + 1,
            })?;
        }
        Ok(())
    }
}
