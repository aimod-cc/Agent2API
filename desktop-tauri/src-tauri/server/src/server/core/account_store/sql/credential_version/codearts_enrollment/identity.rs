//! 本家原行身份的强核对。原pluginIdentity只参与旧ID匹配，不替代真实域/user材料。
//! 相关原行字段缺失/矛盾时拒绝，不能新建另一ID绕开旧Pending/Blocked或恢复隔离。

use serde_json::{Map, Value};

use super::{EnrollmentError, EnrollmentInput, StoredAccount, PROVIDER};
use crate::server::core::account_store::plugin_contract;

pub(super) fn matches(
    original: &StoredAccount,
    input: &EnrollmentInput,
) -> Result<bool, EnrollmentError> {
    let declared = original.get("pluginIdentity").and_then(Value::as_str) == Some(input.identity());
    let prior = identity(original)?;
    let expected = input_identity(input)?;
    if declared && prior != expected {
        return Err(EnrollmentError::IdentityConflict);
    }
    if prior == expected {
        verify_original(original, input)?;
        // 原合同ID命中与未迁移原生行身份命中两种规则；均已强核真实域/user，无AK或token兜底。
        if original
            .get("pluginIdentityContract")
            .and_then(Value::as_u64)
            == Some(1)
        {
            if !plugin_contract::contract_declared_match(original, input.identity(), false) {
                return Err(EnrollmentError::IdentityConflict);
            }
        }
        return Ok(true);
    }
    // 拒绝以相同一次材料另取身份/ID，尤其不能绕过旧槽。
    if shares_material(original, input) {
        return Err(EnrollmentError::IdentityConflict);
    }
    Ok(false)
}

pub(super) fn verify_original(
    original: &StoredAccount,
    input: &EnrollmentInput,
) -> Result<(), EnrollmentError> {
    if original.get("provider").and_then(Value::as_str) != Some(PROVIDER)
        || original
            .get("desktop")
            .is_some_and(|value| value.as_bool() != Some(false))
    {
        return Err(EnrollmentError::IdentityConflict);
    }
    if identity(original)? != input_identity(input)? {
        return Err(EnrollmentError::IdentityConflict);
    }
    if original
        .get("pluginIdentity")
        .is_some_and(|value| value.as_str() != Some(input.identity()))
    {
        return Err(EnrollmentError::IdentityConflict);
    }
    if original
        .get("pluginIdentityContract")
        .is_some_and(|value| value.as_u64() != Some(1))
    {
        return Err(EnrollmentError::InvalidRecord);
    }
    let secret = original.get("pluginSecret").and_then(Value::as_object);
    let public = original.get("pluginPublic").and_then(Value::as_object);
    for fields in [Some(original.fields()), secret, public]
        .into_iter()
        .flatten()
    {
        if fields
            .get("needsRelogin")
            .is_some_and(|value| value.as_bool() != Some(false))
        {
            return Err(EnrollmentError::RequiresNewAuthorization);
        }
        if fields
            .get("desktop")
            .is_some_and(|value| value.as_bool() != Some(false))
        {
            return Err(EnrollmentError::IdentityConflict);
        }
    }
    Ok(())
}

pub(super) fn input_identity(input: &EnrollmentInput) -> Result<(&str, &str), EnrollmentError> {
    let fields = input
        .secret()
        .as_object()
        .ok_or(EnrollmentError::InvalidRecord)?;
    let domain = fields
        .get("domain_id")
        .and_then(Value::as_str)
        .ok_or(EnrollmentError::InvalidRecord)?;
    let user = fields
        .get("user_id")
        .and_then(Value::as_str)
        .ok_or(EnrollmentError::InvalidRecord)?;
    if input.identity() != format!("codearts-{domain}-{user}") {
        return Err(EnrollmentError::IdentityConflict);
    }
    Ok((domain, user))
}

fn identity(original: &StoredAccount) -> Result<(&str, &str), EnrollmentError> {
    let secret = match original
        .get("pluginSecret")
        .filter(|value| !value.is_null())
    {
        Some(Value::Object(fields)) => Some(fields),
        Some(_) => return Err(EnrollmentError::InvalidRecord),
        None => None,
    };
    let public = match original
        .get("pluginPublic")
        .filter(|value| !value.is_null())
    {
        Some(Value::Object(fields)) => Some(fields),
        Some(_) => return Err(EnrollmentError::InvalidRecord),
        None => None,
    };
    let mut domain = None;
    let mut user = None;
    // 先真实secret/原生材料，不从public/pluginIdentity拼回缺少的授权身份。
    if let Some(secret) = secret {
        material_identity(secret, &mut domain, &mut user)?;
    } else {
        material_identity(original.fields(), &mut domain, &mut user)?;
    }
    let result = (
        domain.ok_or(EnrollmentError::IdentityConflict)?,
        user.ok_or(EnrollmentError::IdentityConflict)?,
    );
    // 旧顶层镜像和公开身份若实际存在必须同源；没有的镜像不补填旧凭据。
    material_identity(original.fields(), &mut domain, &mut user)?;
    if let Some(fields) = public {
        aliases(
            fields,
            &["domain_id", "account_id", "domainId"],
            &mut domain,
        )?;
        aliases(
            fields,
            &["user_id", "principal_id", "userId", "uid"],
            &mut user,
        )?;
    }
    Ok(result)
}

fn material_identity<'a>(
    fields: &'a Map<String, Value>,
    domain: &mut Option<&'a str>,
    user: &mut Option<&'a str>,
) -> Result<(), EnrollmentError> {
    aliases(fields, &["domain_id", "account_id", "domainId"], domain)?;
    aliases(fields, &["user_id", "principal_id", "userId"], user)?;
    for name in ["codearts_provider_credential", "credential", "auth"] {
        if let Some(value) = fields.get(name) {
            let fields = value.as_object().ok_or(EnrollmentError::InvalidRecord)?;
            aliases(fields, &["domain_id", "account_id", "domainId"], domain)?;
            aliases(fields, &["user_id", "principal_id", "userId"], user)?;
        }
    }
    Ok(())
}

fn aliases<'a>(
    fields: &'a Map<String, Value>,
    names: &[&str],
    selected: &mut Option<&'a str>,
) -> Result<(), EnrollmentError> {
    for name in names {
        let Some(value) = fields.get(*name) else {
            continue;
        };
        let value = value.as_str().ok_or(EnrollmentError::IdentityConflict)?;
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || selected.is_some_and(|previous| previous != value)
        {
            return Err(EnrollmentError::IdentityConflict);
        }
        *selected = Some(value);
    }
    Ok(())
}

fn shares_material(original: &StoredAccount, input: &EnrollmentInput) -> bool {
    let expected = input.secret();
    let check = |fields: &Map<String, Value>| {
        [
            ["access_key_id", "accessToken", "accessKeyId"].as_slice(),
            ["refresh_token", "refreshToken"].as_slice(),
        ]
        .iter()
        .any(|names| {
            names.iter().any(|name| {
                fields
                    .get(*name)
                    .and_then(Value::as_str)
                    .is_some_and(|value| {
                        !value.is_empty() && value == expected[names[0]].as_str().unwrap_or("")
                    })
            })
        })
    };
    for fields in [
        Some(original.fields()),
        original.get("pluginSecret").and_then(Value::as_object),
    ]
    .into_iter()
    .flatten()
    {
        if check(fields) {
            return true;
        }
        for name in ["codearts_provider_credential", "credential", "auth"] {
            if fields
                .get(name)
                .and_then(Value::as_object)
                .is_some_and(check)
            {
                return true;
            }
        }
    }
    false
}
