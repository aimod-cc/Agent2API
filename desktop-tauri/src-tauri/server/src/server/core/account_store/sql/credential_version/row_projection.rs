//! 固定只读账号来源的完整授权投影；不提供凭据写入、网络或发送许可。
//! 完整pluginSecret与顶层兼容授权字段分别比较，运营字段不参与出生版本维护。
//! CatPaw身份来自uid/loginName；启用状态由调用来源核对，拒绝缺失身份。

use serde_json::{Map, Value};

use super::{bounded, Error};
use crate::server::core::account_store::{
    credential_version::{self, RowProvider},
    state::StoredAccount,
};

// 与plugin_accounts旧账号摊平的骨架字段对应；身份/UA另纳入，顶层proxy仅属运营配置。
const OPERATION_KEYS: &[&str] = &[
    "id",
    "provider",
    "name",
    "nameCustom",
    "source",
    "priority",
    "enabled",
    "addedAt",
    "updatedAt",
    "proxy",
    "rateLimits",
    "upstreamCooldownUntil",
    "upstreamBlockedReason",
    "limiterGroups",
    "limiters",
    "lowBalance",
    "checkinAt",
    "streakDays",
    "claimAt",
    "claimPlanId",
    "claimPlans",
    "pluginState",
    "usageQuery",
    "pluginSecret",
    "pluginPublic",
];
const PUBLIC_IDENTITY_KEYS: &[&str] = &[
    "pluginIdentity",
    "pluginIdentityContract",
    "uid",
    "userId",
    "user_id",
    "accountId",
    "account_id",
    "deviceId",
    "device_id",
    "desktop",
    "userAgent",
    "endpoint",
    "domain",
    "prefixPath",
    "platform",
    "edition",
    "type",
    "enterpriseId",
];

#[derive(PartialEq)]
pub(super) struct Projection {
    provider: RowProvider,
    plugin: Option<Value>,
    native: Value,
    public_identity: Value,
    identity: String,
    user_agent: Option<String>,
}

impl Projection {
    pub(super) fn read(record: &StoredAccount, provider: RowProvider) -> Result<Self, Error> {
        if record.get("provider").and_then(Value::as_str) != Some(provider.id()) {
            return Err(Error::InvalidRecord);
        }
        let catpaw = provider == RowProvider::CatPaw;
        if catpaw && record.get("enabled").is_some_and(|value| !value.is_boolean()) {
            return Err(Error::InvalidRecord);
        }
        let fields = record.fields();
        if catpaw
            && fields
                .get("pluginSecret")
                .is_some_and(|value| !value.is_object())
        {
            return Err(Error::InvalidRecord);
        }
        let plugin = match fields.get("pluginSecret").filter(|value| !value.is_null()) {
            Some(value) if value.is_object() => {
                bounded::check(value, bounded::MAX_SECRET_BYTES)?;
                reject_reserved(value)?;
                Some(value.clone())
            }
            Some(_) => return Err(Error::InvalidRecord),
            None => None,
        };
        let native = Value::Object(
            fields
                .iter()
                .filter(|(key, _)| {
                    (!OPERATION_KEYS.contains(&key.as_str()) || catpaw && key.as_str() == "source")
                        && !key.starts_with("pluginLedger.")
                        && !credential_version::private_field(key)
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        bounded::check(&native, bounded::MAX_SECRET_BYTES)?;
        reject_reserved(&native)?;
        let public_identity = match fields.get("pluginPublic").filter(|value| !value.is_null()) {
            Some(Value::Object(public)) => {
                let mut selected = select(public, PUBLIC_IDENTITY_KEYS);
                if catpaw {
                    selected.extend(select(public, &["loginName", "login_name", "source"]));
                }
                let value = Value::Object(selected);
                bounded::check(&value, bounded::MAX_PUBLIC_BYTES)?;
                reject_reserved(&value)?;
                value
            }
            Some(_) => return Err(Error::InvalidRecord),
            None => Value::Null,
        };
        let identity = if catpaw {
            catpaw_identity(record, plugin.as_ref().unwrap_or(&native))?
        } else {
            match record.get("pluginIdentity") {
                Some(Value::String(value)) if text(value, 1024) => value.clone(),
                Some(_) => return Err(Error::InvalidRecord),
                None => record.id().to_string(),
            }
        };
        let user_agent = match record.get("userAgent") {
            None | Some(Value::Null) => None,
            Some(Value::String(value))
                if value.len() <= 1024 && !value.chars().any(char::is_control) =>
            {
                Some(value.clone())
            }
            Some(_) => return Err(Error::InvalidRecord),
        };
        if catpaw {
            if user_agent.as_deref().is_some_and(|ua| !ua.bytes().all(|byte| matches!(byte, 0x20..=0x7e))) {
                return Err(Error::InvalidRecord);
            }
        }
        Ok(Self {
            provider,
            plugin,
            native,
            public_identity,
            identity,
            user_agent,
        })
    }

    pub(super) fn matches_doubao_fields(
        &self,
        fields: &Map<String, Value>,
        plugin: Option<&Value>,
        public: Option<&Value>,
    ) -> Result<bool, Error> {
        if self.provider != RowProvider::DoubaoWork
            || fields.get("provider").and_then(Value::as_str) != Some(self.provider.id())
        {
            return Err(Error::InvalidRecord);
        }
        let plugin = plugin.filter(|value| !value.is_null());
        if plugin.is_some_and(|value| !value.is_object()) {
            return Err(Error::InvalidRecord);
        }
        if plugin != self.plugin.as_ref() {
            return Ok(false);
        }
        let native = self.native.as_object().ok_or(Error::InvalidRecord)?;
        let included = |key: &str| {
            !OPERATION_KEYS.contains(&key)
                && !key.starts_with("pluginLedger.")
                && !credential_version::private_field(key)
        };
        if fields.iter().filter(|(key, _)| included(key)).count() != native.len()
            || native.iter().any(|(key, value)| fields.get(key) != Some(value))
        {
            return Ok(false);
        }
        match public.filter(|value| !value.is_null()) {
            None if self.public_identity.is_null() => {}
            Some(Value::Object(public)) => {
                let expected = self.public_identity.as_object().ok_or(Error::InvalidRecord)?;
                if PUBLIC_IDENTITY_KEYS.iter().filter(|name| public.contains_key(**name)).count()
                    != expected.len()
                    || expected.iter().any(|(key, value)| public.get(key) != Some(value))
                {
                    return Ok(false);
                }
            }
            _ => return Err(Error::InvalidRecord),
        }
        let identity = match fields.get("pluginIdentity") {
            Some(Value::String(value)) if text(value, 1024) => value.as_str(),
            None => fields.get("id").and_then(Value::as_str).ok_or(Error::InvalidRecord)?,
            _ => return Err(Error::InvalidRecord),
        };
        let user_agent = match fields.get("userAgent") {
            None | Some(Value::Null) => None,
            Some(Value::String(value))
                if value.len() <= 1024 && !value.chars().any(char::is_control) => Some(value.as_str()),
            _ => return Err(Error::InvalidRecord),
        };
        Ok(identity == self.identity && user_agent == self.user_agent.as_deref())
    }

    pub(super) fn secret(&self) -> &Value {
        self.plugin.as_ref().unwrap_or(&self.native)
    }

    pub(super) fn identity(&self) -> &str {
        &self.identity
    }
    pub(super) fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }
}

fn catpaw_identity(record: &StoredAccount, secret: &Value) -> Result<String, Error> {
    let desktop = record.id() == "desktop-auth" || secret["desktop"] == true || secret["source"] == "desktop";
    if desktop {
        if record.get("pluginIdentity").is_some_and(|identity| identity.as_str() != Some("desktop-auth"))
            || ["accessToken", "token", "access_token"].iter().any(|key| secret.get(*key).is_some_and(|value| !value.is_null())) {
            return Err(Error::InvalidRecord);
        }
        return Ok("desktop-auth".to_string());
    }
    for fields in [
        Some(record.fields()),
        secret.as_object(),
        record.get("pluginPublic").and_then(Value::as_object),
    ]
    .into_iter()
    .flatten()
    {
        if fields
            .get("desktop")
            .is_some_and(|value| !value.is_null() && value.as_bool() != Some(false))
            || ["id", "identity", "pluginIdentity"]
                .iter()
                .any(|key| fields.get(*key).and_then(Value::as_str) == Some("desktop-auth"))
            || fields.get("source").is_some_and(|source| {
                matches!(
                    source.as_str(),
                    Some("desktop" | "desktop-auth" | "desktop-live")
                ) || source.get("kind").and_then(Value::as_str) == Some("desktop")
            })
        {
            return Err(Error::InvalidRecord);
        }
    }
    let account = match secret.get("account") {
        None | Some(Value::Null) => None,
        Some(Value::Object(fields)) => Some(fields),
        Some(_) => return Err(Error::InvalidRecord),
    };
    let root = secret.as_object().ok_or(Error::InvalidRecord)?;
    let uid = catpaw_alias([
        root.get("uid"),
        root.get("userId"),
        account.and_then(|fields| fields.get("uid")),
        account.and_then(|fields| fields.get("userId")),
    ])?;
    let login = catpaw_alias([
        root.get("loginName"),
        account.and_then(|fields| fields.get("loginName")),
    ])?;
    let identity = uid.or(login).ok_or(Error::InvalidRecord)?;
    if identity == "desktop-auth" {
        return Err(Error::InvalidRecord);
    }
    if let Some(value) = record.get("pluginIdentity") {
        if value.as_str() != Some(identity) {
            return Err(Error::InvalidRecord);
        }
    }
    Ok(identity.to_string())
}

fn catpaw_alias<'a, const N: usize>(
    values: [Option<&'a Value>; N],
) -> Result<Option<&'a str>, Error> {
    let mut found = None;
    for value in values
        .into_iter()
        .flatten()
        .filter(|value| !value.is_null())
    {
        let raw = value.as_str().ok_or(Error::InvalidRecord)?;
        if raw.len() > 1024 || raw.chars().any(char::is_control) {
            return Err(Error::InvalidRecord);
        }
        let current = raw.trim();
        if current.is_empty() {
            continue;
        }
        if current.chars().count() > 256
            || current.contains(';')
            || found.is_some_and(|previous| previous != current)
        {
            return Err(Error::InvalidRecord);
        }
        found = Some(current);
    }
    Ok(found)
}

fn select(fields: &Map<String, Value>, names: &[&str]) -> Map<String, Value> {
    names
        .iter()
        .filter_map(|name| {
            fields
                .get(*name)
                .map(|value| ((*name).to_string(), value.clone()))
        })
        .collect()
}

fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn reject_reserved(value: &Value) -> Result<(), Error> {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if credential_version::private_field(key) {
                    return Err(Error::InvalidInput);
                }
                reject_reserved(value)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_reserved(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}
