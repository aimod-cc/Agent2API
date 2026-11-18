//! 本次原行底稿保存。既有id/运营属性/代理/未知字段保留，授权材料完整替换不从旧值补齐。
//! 只受外层FULL/IMMEDIATE提交；不调用普通upsert/put，不重排或解密其他提供商账号。

use rusqlite::{types::ValueRef, Connection};
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use super::{bounded, error, identity, EnrollmentError, EnrollmentInput, StoredAccount, PROVIDER};
use crate::server::core::account_store::priority;
use crate::server::core::crypto;

const MATERIAL: &[&str] = &[
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
    "oauth_context",
    "domain_id",
    "account_id",
    "domainId",
    "user_id",
    "principal_id",
    "userId",
    "user_name",
    "userName",
    "login_type",
    "loginType",
];
const CONTAINERS: &[&str] = &["codearts_provider_credential", "credential", "auth"];

pub(super) fn new_account(
    conn: &Connection,
    id: &str,
    input: &EnrollmentInput,
    now: i64,
) -> Result<StoredAccount, EnrollmentError> {
    let mut fields = Map::new();
    fields.insert("id".into(), Value::String(id.into()));
    fields.insert("provider".into(), Value::String(PROVIDER.into()));
    fields.insert("name".into(), Value::String(input.name().into()));
    fields.insert("source".into(), Value::String("plugin".into()));
    fields.insert("desktop".into(), Value::Bool(false));
    fields.insert("enabled".into(), Value::Bool(true));
    fields.insert("priority".into(), Value::from(next_priority(conn)?));
    fields.insert("addedAt".into(), Value::from(now));
    Ok(StoredAccount::from_map(fields))
}

pub(super) fn apply(
    record: &mut StoredAccount,
    input: &EnrollmentInput,
    discarded: &mut Vec<Value>,
    now: i64,
) -> Result<(), EnrollmentError> {
    let before_plugin = record
        .get("pluginSecret")
        .is_some_and(|value| !value.is_null());
    let source = input
        .secret()
        .as_object()
        .ok_or(EnrollmentError::InvalidRecord)?;
    // 原pluginSecret只是底稿：未知非材料字段与旧secret.proxy保留，固定材料只取本次Input。
    let secret = merged_secret(record, source, discarded)?;
    // 原行已有的兼容字段同步为本次真实材料；不让另一个原生读取路径继续用旧token/context。
    // 未知字段仍留在原行；本次没有提供的可选旧授权字段明确清掉，不沿用另一份上下文。
    for name in MATERIAL {
        if !before_plugin || record.get(name).is_some() {
            set_optional(
                record.fields_mut(),
                name,
                material_value(source, name).cloned(),
                discarded,
            );
        }
    }
    for name in CONTAINERS {
        if let Some(value) = record.fields_mut().get_mut(*name) {
            let fields = value
                .as_object_mut()
                .ok_or(EnrollmentError::InvalidRecord)?;
            for key in MATERIAL {
                if *name != "auth" || fields.contains_key(*key) {
                    set_optional(fields, key, material_value(source, key).cloned(), discarded);
                }
            }
        }
    }
    let mut public = match record.get("pluginPublic").filter(|value| !value.is_null()) {
        Some(Value::Object(fields)) => fields.clone(),
        Some(_) => return Err(EnrollmentError::InvalidRecord),
        None => Map::new(),
    };
    // 本家输出的公开字段覆盖原值，过期/重登录等旧凭据派生信息不能留在未知字段中冒充新事实。
    for name in [
        "needsRelogin",
        "needsReloginReason",
        "refreshTokenExpiresAt",
        "refreshExpiryStatus",
    ] {
        if let Some(value) = public.remove(name) {
            discarded.push(value);
        }
    }
    if let Some(fields) = input.public().as_object() {
        for (name, value) in fields {
            set_optional(&mut public, name, Some(value.clone()), discarded);
        }
    }
    for name in [
        "expiresAtMs",
        "tokenExpireAt",
        "tokenExpiresAt",
        "accessTokenExpiresAt",
    ] {
        if public.contains_key(name) {
            set_optional(
                &mut public,
                name,
                Some(Value::from(input.expires_at_ms())),
                discarded,
            );
        }
    }
    public.insert("canRefresh".into(), Value::Bool(true));
    public.insert("refreshStatus".into(), Value::String("host_managed".into()));
    set(record, "pluginSecret", secret, discarded);
    set(record, "pluginPublic", Value::Object(public), discarded);
    set(
        record,
        "pluginIdentity",
        Value::String(input.identity().into()),
        discarded,
    );
    set(record, "pluginIdentityContract", Value::from(1), discarded);
    set(record, "desktop", Value::Bool(false), discarded);
    set(record, "updatedAt", Value::from(now), discarded);
    // 保留未知运营字段；原备注只在新建时设置。原公开名称不能替代宿主账号顶层备注。
    verify_input(record, input)
}

fn merged_secret(
    record: &StoredAccount,
    input: &Map<String, Value>,
    discarded: &mut Vec<Value>,
) -> Result<Value, EnrollmentError> {
    let mut fields = match record.get("pluginSecret").filter(|value| !value.is_null()) {
        Some(Value::Object(fields)) => fields.clone(),
        Some(_) => return Err(EnrollmentError::InvalidRecord),
        None => Map::new(), // 旧原生未知字段留在原行，不摊平运营内容成为凭据。
    };
    for key in MATERIAL {
        set_optional(
            &mut fields,
            key,
            material_value(input, key).cloned(),
            discarded,
        );
    }
    for name in CONTAINERS {
        if let Some(value) = fields.get_mut(*name) {
            let nested = value
                .as_object_mut()
                .ok_or(EnrollmentError::InvalidRecord)?;
            for key in MATERIAL {
                if *name != "auth" || nested.contains_key(*key) {
                    set_optional(nested, key, material_value(input, key).cloned(), discarded);
                }
            }
        }
    }
    // Input只含已核验的材料与可选显示名；不创建input没有提供的长期材料。
    for (key, value) in input {
        set_optional(&mut fields, key, Some(value.clone()), discarded);
    }
    Ok(Value::Object(fields))
}

fn material_value<'a>(source: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    let canonical = match name {
        "accessKeyId" => "access_key_id",
        "secretAccessKey" => "secret_access_key",
        "securityToken" => "security_token",
        "account_id" | "domainId" => "domain_id",
        "principal_id" => "user_id",
        "userName" => "user_name",
        "loginType" => "login_type",
        _ => name,
    };
    source.get(canonical)
}

pub(super) fn set(
    record: &mut StoredAccount,
    name: &str,
    value: Value,
    discarded: &mut Vec<Value>,
) {
    set_optional(record.fields_mut(), name, Some(value), discarded);
}
fn set_optional(
    fields: &mut Map<String, Value>,
    name: &str,
    value: Option<Value>,
    discarded: &mut Vec<Value>,
) {
    let previous = match value {
        Some(value) => fields.insert(name.to_string(), value),
        None => fields.remove(name),
    };
    if let Some(previous) = previous {
        discarded.push(previous);
    }
}

pub(super) fn verify_input(
    record: &StoredAccount,
    input: &EnrollmentInput,
) -> Result<(), EnrollmentError> {
    if record.get("pluginIdentity").and_then(Value::as_str) != Some(input.identity())
        || record.get("provider").and_then(Value::as_str) != Some(PROVIDER)
    {
        return Err(EnrollmentError::IdentityConflict);
    }
    let secret = record
        .get("pluginSecret")
        .and_then(Value::as_object)
        .ok_or(EnrollmentError::InvalidRecord)?;
    let expected = input
        .secret()
        .as_object()
        .ok_or(EnrollmentError::InvalidRecord)?;
    for key in MATERIAL {
        if secret.get(*key) != material_value(expected, key) {
            return Err(EnrollmentError::IdentityConflict);
        }
    }
    identity::verify_original(record, input)?;
    super::super::projection::Projection::read(record).map_err(error)?;
    let checked = Zeroizing::new(
        bounded::encode_fields(record.fields(), bounded::MAX_PLAIN_BYTES).map_err(error)?,
    );
    drop(checked);
    Ok(())
}

pub(super) fn write(
    conn: &Connection,
    key: &[u8; 32],
    record: &StoredAccount,
    existed: bool,
) -> Result<(), EnrollmentError> {
    let plain = Zeroizing::new(
        bounded::encode_fields(record.fields(), bounded::MAX_PLAIN_BYTES).map_err(error)?,
    );
    let data = crypto::seal_with_key(&plain, key).map_err(|_| EnrollmentError::Storage)?;
    if data.len() > bounded::MAX_ROW_BYTES {
        return Err(EnrollmentError::LimitExceeded);
    }
    let count = if existed {
        conn.execute("UPDATE main.accounts SET data=?3 WHERE id=?1 AND provider=?2",
            rusqlite::params![record.id(), PROVIDER, data])
    } else {
        conn.execute("INSERT INTO main.accounts(id,provider,priority,enabled,added_at,data) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![record.id(), PROVIDER, record.priority(), i64::from(record.enabled()), record.added_at(), data])
    }.map_err(|_| EnrollmentError::Storage)?;
    if count != 1 {
        return Err(EnrollmentError::Storage);
    }
    Ok(())
}

fn next_priority(conn: &Connection) -> Result<i64, EnrollmentError> {
    let max = conn
        .query_row(
            "SELECT MAX(priority) FROM main.accounts",
            [],
            |row| match row.get_ref(0)? {
                ValueRef::Null => Ok(None),
                ValueRef::Integer(value) => Ok(Some(value)),
                _ => Err(rusqlite::Error::InvalidQuery),
            },
        )
        .map_err(|_| EnrollmentError::InvalidRecord)?;
    let next = max
        .map(priority::normalize_priority_value)
        .unwrap_or(priority::DEFAULT_PRIORITY - 1)
        + 1;
    if next <= priority::MAX_PRIORITY && !priority_occupied(conn, next)? {
        return Ok(next);
    }
    for candidate in priority::MIN_PRIORITY..=priority::MAX_PRIORITY {
        if !priority_occupied(conn, candidate)? {
            return Ok(candidate);
        }
    }
    Err(EnrollmentError::PriorityExhausted)
}
fn priority_occupied(conn: &Connection, priority: i64) -> Result<bool, EnrollmentError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.accounts WHERE priority=?1)",
        [priority],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|_| EnrollmentError::Storage)
}
