//! Free原行强身份/材料排除。旧插件报告仅增加拒绝条件，不补真实UID或首次登记资格。
//! 不读Pass；任何相关坏字段/互相冲突均拒绝，不跳过原行后另造新ID。

use serde_json::{Map, Value};

use super::{
    storage, FreeEnrollmentError, FreeEnrollmentInput, RetainedRows, StoredAccount, PROVIDER,
};

const UID_KEYS: &[&str] = &[
    "account",
    "accountId",
    "account_id",
    "userId",
    "user_id",
    "uid",
];
const ACCESS_KEYS: &[&str] = &["accessToken", "access_token", "token"];
const REFRESH_KEYS: &[&str] = &["refreshToken", "refresh_token"];
const CONTAINERS: &[&str] = &["auth"];

pub(super) fn verify_input(input: &FreeEnrollmentInput) -> Result<(), FreeEnrollmentError> {
    let secret = input
        .secret()
        .as_object()
        .ok_or(FreeEnrollmentError::InvalidInput)?;
    let public = input
        .public()
        .as_object()
        .ok_or(FreeEnrollmentError::InvalidInput)?;
    let expected_expiry = input
        .expires_at_ms()
        .map(Value::from)
        .unwrap_or(Value::Null);
    if !uid_valid(input.uid())
        || input.identity() != input.uid()
        || secret.get("account").and_then(Value::as_str) != Some(input.uid())
        || public.get("account").and_then(Value::as_str) != Some(input.uid())
        || public.get("uid").and_then(Value::as_str) != Some(input.uid())
        || secret
            .get("accessToken")
            .and_then(Value::as_str)
            .is_none_or(|token| {
                !token.starts_with("workos:") || normalize_token(token, true).is_err()
            })
        || secret
            .get("refreshToken")
            .and_then(Value::as_str)
            .is_none_or(|token| normalize_token(token, false).is_err())
        || public.get("canRefresh").and_then(Value::as_bool) != Some(true)
        || public.get("desktopAuth").and_then(Value::as_bool) != Some(false)
        || public.get("upstreamValidated").and_then(Value::as_bool) != Some(false)
        || secret.get("expiresAt") != Some(&expected_expiry)
        || public.get("expiresAt") != Some(&expected_expiry)
    {
        return Err(FreeEnrollmentError::InvalidInput);
    }
    Ok(())
}

pub(super) fn exclude_existing(
    index: usize,
    input: &FreeEnrollmentInput,
    retained: &mut RetainedRows,
) -> Result<(), FreeEnrollmentError> {
    let claims = retained.decode_claims(index)?;
    let original = retained.record(index);
    if original.get("provider").and_then(Value::as_str) != Some(PROVIDER) {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    let mut uid = None;
    // 实际secret/原生身份、metadata和原JWT external_id强核；public/pluginIdentity不补UID。
    visit_material(original, |fields| {
        collect_uid(fields, &mut uid)?;
        for name in ["userInfo", "metadata"] {
            if let Some(value) = fields.get(name).filter(|value| !value.is_null()) {
                let mut fields = value
                    .as_object()
                    .ok_or(FreeEnrollmentError::InvalidRecord)?;
                if name == "metadata" {
                    match fields.get("userInfo").filter(|value| !value.is_null()) {
                        Some(info) => {
                            fields = info.as_object().ok_or(FreeEnrollmentError::InvalidRecord)?
                        }
                        None => continue,
                    }
                }
                collect_aliases(fields, &["clineUserId", "external_id"], &mut uid)?;
            }
        }
        Ok(())
    })?;
    let mut declared_uid = None;
    if let Some(claims) = claims {
        let claims = retained
            .value(claims)
            .as_object()
            .ok_or(FreeEnrollmentError::InvalidRecord)?;
        collect_aliases(claims, &["external_id"], &mut declared_uid)?;
        collect_aliases(claims, &["external_id"], &mut uid)?;
        if let Some(value) = claims.get("exp").filter(|value| !value.is_null()) {
            if value
                .as_i64()
                .is_none_or(|value| value <= 0 || value.checked_mul(1000).is_none())
            {
                return Err(FreeEnrollmentError::InvalidRecord);
            }
        }
    }
    let uid = uid.ok_or(FreeEnrollmentError::IdentityConflict)?;
    if declared_uid.is_none() && (!uid.starts_with("usr-") || uid.len() <= 4) {
        return Err(FreeEnrollmentError::IdentityConflict);
    }
    let mut observed = Some(uid);
    if let Some(public) = original
        .get("pluginPublic")
        .filter(|value| !value.is_null())
    {
        collect_uid(
            public
                .as_object()
                .ok_or(FreeEnrollmentError::InvalidRecord)?,
            &mut observed,
        )?;
    }
    if let Some(declared) = original.get("pluginIdentity") {
        let declared = declared
            .as_str()
            .filter(|text| uid_valid(text))
            .ok_or(FreeEnrollmentError::InvalidRecord)?;
        // 不接受展示名/摘要旧身份与真实UID不符后，重新选择一个新合同ID。
        if declared != uid {
            return Err(FreeEnrollmentError::IdentityConflict);
        }
    }
    if original
        .get("pluginIdentityContract")
        .is_some_and(|value| value.as_u64() != Some(1))
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    validate_flags(original)?;
    let access = access_token(original)?;
    let refresh = refresh_token(original)?;
    if original
        .get("pluginSecret")
        .is_some_and(|value| !value.is_null())
        && access.is_none()
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    let same_material = access.is_some_and(|value| {
        input
            .secret()
            .get("accessToken")
            .and_then(Value::as_str)
            .and_then(|token| token.strip_prefix("workos:"))
            .is_some_and(|token| value == token)
    }) || refresh.is_some_and(|value| {
        input.secret().get("refreshToken").and_then(Value::as_str) == Some(value)
    });
    if uid == input.uid() || same_material {
        return Err(FreeEnrollmentError::ExistingAccount);
    }
    Ok(())
}

pub(super) fn verify_refresh(index: usize, retained: &mut RetainedRows) -> Result<(), FreeEnrollmentError> {
    let claims = retained.decode_claims(index)?;
    let record = retained.record(index);
    validate_flags(record)?;
    let mut uid = None;
    visit_material(record, |fields| {
        collect_uid(fields, &mut uid)?;
        if fields.get("desktop").is_some_and(|value| value.as_bool() != Some(false))
            || fields.get("source").is_some_and(|value| value.as_str() == Some("desktop")) {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
        Ok(())
    })?;
    let mut declared = None;
    if let Some(claims) = claims {
        collect_aliases(retained.value(claims).as_object().ok_or(FreeEnrollmentError::InvalidRecord)?,
            &["external_id"], &mut declared)?;
        collect_aliases(retained.value(claims).as_object().ok_or(FreeEnrollmentError::InvalidRecord)?,
            &["external_id"], &mut uid)?;
    }
    let uid = uid.ok_or(FreeEnrollmentError::IdentityConflict)?;
    if declared.is_none() && (!uid.starts_with("usr-") || uid.len() <= 4)
        || record.get("pluginIdentity").and_then(Value::as_str) != Some(uid)
        || record.get("pluginIdentityContract").and_then(Value::as_u64) != Some(1)
        || access_token(record)?.is_none() || refresh_token(record)?.is_none() {
        return Err(FreeEnrollmentError::IdentityConflict);
    }
    Ok(())
}

pub(super) fn access_token(record: &StoredAccount) -> Result<Option<&str>, FreeEnrollmentError> {
    token_aliases(record, ACCESS_KEYS, true)
}
pub(super) fn refresh_token(record: &StoredAccount) -> Result<Option<&str>, FreeEnrollmentError> {
    token_aliases(record, REFRESH_KEYS, false)
}
fn token_aliases<'a>(
    record: &'a StoredAccount,
    aliases: &[&str],
    access: bool,
) -> Result<Option<&'a str>, FreeEnrollmentError> {
    let mut selected = None;
    visit_material(record, |fields| {
        for key in aliases {
            let Some(value) = fields.get(*key).filter(|value| !value.is_null()) else {
                continue;
            };
            let token = normalize_token(
                value.as_str().ok_or(FreeEnrollmentError::InvalidRecord)?,
                access,
            )?;
            if selected.is_some_and(|previous| previous != token) {
                return Err(FreeEnrollmentError::IdentityConflict);
            }
            selected = Some(token);
        }
        Ok(())
    })?;
    Ok(selected)
}
fn normalize_token(token: &str, access: bool) -> Result<&str, FreeEnrollmentError> {
    if token.is_empty()
        || token.len() > storage::MAX_SECRET_BYTES
        || !token.is_ascii()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    let token = if access {
        token.strip_prefix("workos:").unwrap_or(token)
    } else {
        token
    };
    if token.is_empty() || access && token.starts_with("workos:") {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    Ok(token)
}
fn visit_material<'a>(
    record: &'a StoredAccount,
    mut action: impl FnMut(&'a Map<String, Value>) -> Result<(), FreeEnrollmentError>,
) -> Result<(), FreeEnrollmentError> {
    for fields in [
        Some(record.fields()),
        record
            .get("pluginSecret")
            .filter(|value| !value.is_null())
            .map(|value| value.as_object().ok_or(FreeEnrollmentError::InvalidRecord))
            .transpose()?,
    ]
    .into_iter()
    .flatten()
    {
        action(fields)?;
        for name in CONTAINERS {
            if let Some(value) = fields.get(*name).filter(|value| !value.is_null()) {
                action(
                    value
                        .as_object()
                        .ok_or(FreeEnrollmentError::InvalidRecord)?,
                )?;
            }
        }
    }
    Ok(())
}
fn collect_uid<'a>(
    fields: &'a Map<String, Value>,
    selected: &mut Option<&'a str>,
) -> Result<(), FreeEnrollmentError> {
    collect_aliases(fields, UID_KEYS, selected)
}
fn collect_aliases<'a>(
    fields: &'a Map<String, Value>,
    names: &[&str],
    selected: &mut Option<&'a str>,
) -> Result<(), FreeEnrollmentError> {
    for name in names {
        if let Some(value) = fields.get(*name).filter(|value| !value.is_null()) {
            let uid = value
                .as_str()
                .filter(|text| uid_valid(text))
                .ok_or(FreeEnrollmentError::InvalidRecord)?;
            if selected.is_some_and(|previous| previous != uid) {
                return Err(FreeEnrollmentError::IdentityConflict);
            }
            *selected = Some(uid);
        }
    }
    Ok(())
}
fn uid_valid(uid: &str) -> bool {
    !uid.is_empty()
        && uid.len() <= 512
        && uid.trim() == uid
        && !uid.chars().any(char::is_control)
        && !uid.contains(['/', '\\'])
        && !uid.contains("://")
}
fn validate_flags(record: &StoredAccount) -> Result<(), FreeEnrollmentError> {
    visit_material(record, |fields| flags(fields))?;
    if let Some(public) = record.get("pluginPublic").filter(|value| !value.is_null()) {
        flags(
            public
                .as_object()
                .ok_or(FreeEnrollmentError::InvalidRecord)?,
        )?;
    }
    Ok(())
}
fn flags(fields: &Map<String, Value>) -> Result<(), FreeEnrollmentError> {
    if fields
        .get("provider")
        .is_some_and(|value| value.as_str() != Some(PROVIDER))
        || fields.get("pool").is_some_and(|value| {
            value.as_str().is_none_or(|pool| {
                !pool.eq_ignore_ascii_case("free") && !pool.eq_ignore_ascii_case(PROVIDER)
            })
        })
    {
        return Err(FreeEnrollmentError::InvalidRecord);
    }
    for name in [
        "desktop",
        "desktopAuth",
        "needsRelogin",
        "hasAccessToken",
        "hasRefreshToken",
    ] {
        if fields
            .get(name)
            .is_some_and(|value| !value.is_null() && value.as_bool().is_none())
        {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
    }
    if let Some(value) = fields.get("expiresAt").filter(|value| !value.is_null()) {
        let valid = value.as_i64().is_some_and(|value| value > 0)
            || value.as_str().is_some_and(|text| {
                text.parse::<i64>().is_ok_and(|value| value > 0)
                    || chrono::DateTime::parse_from_rfc3339(text)
                        .is_ok_and(|value| value.timestamp_millis() > 0)
            });
        if !valid {
            return Err(FreeEnrollmentError::InvalidRecord);
        }
    }
    Ok(())
}
