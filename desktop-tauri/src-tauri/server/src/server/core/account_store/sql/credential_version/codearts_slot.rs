//! CodeArts单账号严格Ready/Pending存储。缺版本/槽/旧epoch从不建表、重建或自动纳管。
//! SQL返回的只是同事务候选；真实COMMIT+连接收尾由原来源门面确认后才交付对象。
//! 本家固定短期材料/原操作结清/下一Ready同事务，未知只能Blocked或保持Pending。

mod record;
mod final_check;
pub(in crate::server::core::account_store) use final_check::verify_pending_current;

use rusqlite::Connection;
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use super::{bounded, storage, CommitReceipt, Error, Stamp, StorageFailure, Version};
use crate::server::core::account_store::state::StoredAccount;
use record::{Slot, State};

pub(in crate::server::core::account_store) use record::RetainedRows;

#[derive(Debug)]
pub(crate) enum SlotFailure {
    Account(Error),
    Schema,
    Missing,
    Invalid,
    NotReady,
    ReceiptChanged,
    RequiresNewAuthorization,
    IncompleteReplacement,
    Interrupted,
}

impl From<Error> for SlotFailure {
    fn from(error: Error) -> Self {
        Self::Account(error)
    }
}

/// 不公开随机字段；只由严格读取真实Ready槽构造，不接JSON/账号/计时/默认代次。
pub(in crate::server::core::account_store) struct ReadyGeneration {
    generation: String,
}

pub(in crate::server::core::account_store) struct ReadyCandidate {
    pub(in crate::server::core::account_store) stamp: Stamp,
    pub(in crate::server::core::account_store) generation: ReadyGeneration,
    pub(in crate::server::core::account_store) proxy: Value,
}

/// 只是事务内Pending候选；提交前不能外发。字段私有，无Clone/Debug/serde。
pub(in crate::server::core::account_store) struct PendingWrite {
    generation: String,
    operation: String,
}

pub(crate) enum Settlement {
    Rotated(CommitReceipt),
    Blocked(SlotFailure),
}

/// 外层锁外纯校验得到的完整五项；不是上游响应回执。该对象不改变身份/OAuth。
pub(in crate::server::core::account_store) struct Replacement {
    fields: Map<String, Value>,
    expires_at: i64,
}

impl Drop for Replacement {
    fn drop(&mut self) {
        record::wipe_fields(&mut self.fields);
    }
}

/// 固定首次登记事务的无槽核对；任何存在的行均拒绝，不读取payload补造资格。
pub(in crate::server::core::account_store::sql::credential_version) fn require_initial_absent(
    conn: &Connection,
    id: &str,
) -> Result<(), SlotFailure> {
    record::require_absent(conn, id)
}

pub(in crate::server::core::account_store::sql::credential_version) fn remove_for_new_login(conn: &Connection, id: &str) -> Result<(), SlotFailure> {
    record::remove_for_new_login(conn, id)
}

/// 仅首次显式登记事务可调用；原行已经写入最终版本，此处从不自行纳管旧账号。
pub(in crate::server::core::account_store::sql::credential_version) fn establish_initial(
    conn: &Connection,
    key: &[u8; 32],
    id: &str,
    version: &Version,
) -> Result<(), SlotFailure> {
    let generation = storage::random_id()?;
    record::insert_initial(
        conn,
        key,
        &Slot {
            schema: 1,
            provider: "codearts".to_string(),
            account_id: id.to_string(),
            db_epoch: version.epoch.clone(),
            born: version.born.clone(),
            revision: version.revision,
            generation,
            state: State::Ready {
                settled_operation: None,
            },
        },
    )
}

/// 只读同原FULL/IMMEDIATE事务：读取真实加密行、已有版本/epoch、完整投影与同代Ready。
/// 不调用prepare/establish_epoch/initialize_empty；缺任何前置都拒绝，无写入动作。
pub(in crate::server::core::account_store) fn read_ready(
    conn: &Connection,
    key: &[u8; 32],
    id: &str,
    retained: &mut RetainedRows,
) -> Result<ReadyCandidate, SlotFailure> {
    if conn.is_autocommit() {
        return Err(Error::Storage(StorageFailure::Begin).into());
    }
    super::check_target("codearts", id)?;
    let index = record::load_account(conn, key, id, retained)?;
    let account = retained.account(index);
    eligible(account)?;
    let version =
        storage::version(account)?.ok_or(Error::Conflict(super::Conflict::UnestablishedVersion))?;
    let epoch =
        storage::epoch(conn)?.ok_or(Error::Conflict(super::Conflict::DatabaseEpochChanged))?;
    if version.epoch != epoch {
        return Err(Error::Conflict(super::Conflict::DatabaseEpochChanged).into());
    }
    let slot = record::load(conn, key, id)?;
    if slot.db_epoch != version.epoch
        || slot.born != version.born
        || slot.revision != version.revision
    {
        return Err(SlotFailure::ReceiptChanged);
    }
    if !matches!(&slot.state, State::Ready { .. }) {
        return Err(SlotFailure::NotReady);
    }
    let proxy = retained.account(index).get("proxy")
        .or_else(|| retained.account(index).get("pluginSecret").and_then(|secret| secret.get("proxy")))
        .cloned().unwrap_or(Value::Null);
    bounded::check(&proxy, 64 * 1024)?;
    let projection = retained.read_projection(index)?;
    Ok(ReadyCandidate {
        stamp: Stamp {
            id: id.to_string(),
            version,
            projection,
        },
        generation: ReadyGeneration { generation: slot.generation },
        proxy,
    })
}

/// 原ReadyRead按值消费；与原ReadStamp完整投影/版本及真实Ready generation逐一核对。
pub(in crate::server::core::account_store) fn begin(
    conn: &Connection,
    key: &[u8; 32],
    stamp: &Stamp,
    ready: &ReadyGeneration,
    retained: &mut RetainedRows,
) -> Result<PendingWrite, SlotFailure> {
    let index = current(conn, key, stamp, retained)?;
    eligible(retained.account(index))?;
    let mut slot = record::load(conn, key, &stamp.id)?;
    require_binding(&slot, stamp)?;
    if slot.generation != ready.generation {
        return Err(SlotFailure::ReceiptChanged);
    }
    let State::Ready { settled_operation } = &slot.state else {
        return Err(SlotFailure::NotReady);
    };
    let operation = storage::random_id()?;
    if operation == slot.generation || settled_operation.as_ref() == Some(&operation) {
        return Err(Error::Storage(StorageFailure::RandomUnavailable).into());
    }
    slot.state = State::Pending {
        operation: operation.clone(),
    };
    record::update(conn, key, &slot)?;
    Ok(PendingWrite {
        generation: slot.generation,
        operation,
    })
}

/// 原Pending结清：本次五项完整值/私有版本/下一Ready只占原行原槽，同事务回读确认。
/// replacement从锁外预检按借用传入；任何错误都不沿用旧刷新令牌或另取来源。
pub(in crate::server::core::account_store) fn complete(
    conn: &Connection,
    key: &[u8; 32],
    stamp: &Stamp,
    pending: &PendingWrite,
    replacement: &Replacement,
    retained: &mut RetainedRows,
) -> Result<Settlement, SlotFailure> {
    let mut slot = pending_slot(conn, key, stamp, pending)?;
    let index = match current(conn, key, stamp, retained) {
        Ok(index) => index,
        Err(error) => return block_with(conn, key, &mut slot, error),
    };
    if let Err(error) = eligible(retained.account(index)) {
        return block_with(conn, key, &mut slot, error);
    }
    let now = crate::server::logging::now_ms();
    if now <= 0 || replacement.expires_at <= now {
        return block_with(conn, key, &mut slot, SlotFailure::IncompleteReplacement);
    }
    let original_index = index;
    let index = match record::apply_replacement(
        retained,
        index,
        &replacement.fields,
        replacement.expires_at,
    ) {
        Ok(index) => index,
        Err(error) => return block_with(conn, key, &mut slot, error),
    };
    if let Err(error) =
        validate_replacement_view(retained.account(original_index), retained.account(index))
    {
        return block_with(conn, key, &mut slot, error);
    }
    let Some(revision) = stamp.version.revision.checked_add(1) else {
        return block_with(conn, key, &mut slot, Error::VersionExhausted.into());
    };
    // 完整新投影也只在外层持有，SQL返回/设置恢复失败不会在外层锁内丢弃其内容。
    if let Err(error) = retained.keep_projection(index) {
        return block_with(conn, key, &mut slot, error);
    }
    let next_generation = storage::random_id()?;
    if next_generation == slot.generation || next_generation == pending.operation {
        return Err(Error::Storage(StorageFailure::RandomUnavailable).into());
    }
    record::set_field(
        retained,
        index,
        crate::server::core::account_store::credential_version::PRIVATE_FIELD,
        serde_json::json!({ "schema":1, "dbEpoch":stamp.version.epoch, "born":stamp.version.born, "revision":revision }),
    );
    record::set_field(retained, index, "updatedAt", Value::from(now));
    record::write_account(conn, key, retained, index)?;
    let readback = record::load_account(conn, key, &stamp.id, retained)?;
    if retained.account(readback).fields() != retained.account(index).fields() {
        return Err(Error::Storage(StorageFailure::Write).into());
    }
    slot.revision = revision;
    slot.generation = next_generation;
    slot.state = State::Ready {
        settled_operation: Some(pending.operation.clone()),
    };
    record::update(conn, key, &slot)?;
    Ok(Settlement::Rotated(CommitReceipt {
        revision_advanced: true,
    }))
}

/// 原Pending唯一操作收据才能封锁；缺槽、旧Db/epoch/generation或其它操作不能代替。
pub(in crate::server::core::account_store) fn block(
    conn: &Connection,
    key: &[u8; 32],
    stamp: &Stamp,
    pending: &PendingWrite,
) -> Result<(), SlotFailure> {
    let mut slot = pending_slot(conn, key, stamp, pending)?;
    slot.state = State::Blocked {
        operation: pending.operation.clone(),
    };
    record::update(conn, key, &slot)
}

fn current(
    conn: &Connection,
    key: &[u8; 32],
    stamp: &Stamp,
    retained: &mut RetainedRows,
) -> Result<usize, SlotFailure> {
    if conn.is_autocommit() {
        return Err(Error::Storage(StorageFailure::Begin).into());
    }
    let epoch =
        storage::epoch(conn)?.ok_or(Error::Conflict(super::Conflict::DatabaseEpochChanged))?;
    if epoch != stamp.version.epoch {
        return Err(Error::Conflict(super::Conflict::DatabaseEpochChanged).into());
    }
    let index = record::load_account(conn, key, &stamp.id, retained)?;
    let version = storage::version(retained.account(index))?
        .ok_or(Error::Conflict(super::Conflict::UnestablishedVersion))?;
    if version != stamp.version {
        return Err(Error::Conflict(super::Conflict::VersionChanged).into());
    }
    let projection = retained.keep_projection(index)?;
    if retained.projection(projection) != &stamp.projection {
        return Err(Error::Conflict(super::Conflict::AuthorizationChanged).into());
    }
    Ok(index)
}

fn eligible(account: &StoredAccount) -> Result<(), SlotFailure> {
    for fields in [
        Some(account.fields()),
        account.get("pluginSecret").and_then(Value::as_object),
        account.get("pluginPublic").and_then(Value::as_object),
    ]
    .into_iter()
    .flatten()
    {
        if fields
            .get("needsRelogin")
            .is_some_and(|value| value.as_bool() != Some(false))
        {
            return Err(SlotFailure::RequiresNewAuthorization);
        }
        if fields
            .get("desktop")
            .is_some_and(|value| value.as_bool() != Some(false))
        {
            return Err(SlotFailure::Invalid);
        }
    }
    Ok(())
}

fn require_binding(slot: &Slot, stamp: &Stamp) -> Result<(), SlotFailure> {
    if slot.account_id != stamp.id
        || slot.db_epoch != stamp.version.epoch
        || slot.born != stamp.version.born
        || slot.revision != stamp.version.revision
    {
        return Err(SlotFailure::ReceiptChanged);
    }
    Ok(())
}

fn pending_slot(
    conn: &Connection,
    key: &[u8; 32],
    stamp: &Stamp,
    pending: &PendingWrite,
) -> Result<Slot, SlotFailure> {
    let epoch =
        storage::epoch(conn)?.ok_or(Error::Conflict(super::Conflict::DatabaseEpochChanged))?;
    if epoch != stamp.version.epoch {
        return Err(Error::Conflict(super::Conflict::DatabaseEpochChanged).into());
    }
    let slot = record::load(conn, key, &stamp.id)?;
    require_binding(&slot, stamp)?;
    if slot.generation != pending.generation
        || !matches!(&slot.state, State::Pending { operation } if operation == &pending.operation)
    {
        return Err(SlotFailure::ReceiptChanged);
    }
    Ok(slot)
}

fn block_with(
    conn: &Connection,
    key: &[u8; 32],
    slot: &mut Slot,
    error: SlotFailure,
) -> Result<Settlement, SlotFailure> {
    let State::Pending { operation } = &slot.state else {
        return Err(SlotFailure::ReceiptChanged);
    };
    slot.state = State::Blocked {
        operation: operation.clone(),
    };
    record::update(conn, key, slot)?;
    // Ok仅要求真实提交Blocked，业务结果绝不是成功。
    Ok(Settlement::Blocked(error))
}

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
    "expiration",
    "expires_at",
    "expiresAt",
];
const PUBLIC_SCALARS: &[&str] = &[
    "tokenTail",
    "hasAccessToken",
    "hasRefreshToken",
    "hasToken",
    "hasCredentials",
    "canRefresh",
    "available",
    "expiresAt",
    "expiresAtMs",
    "tokenExpireAt",
    "tokenExpiresAt",
    "accessTokenExpiresAt",
    "refreshTokenExpiresAt",
    "needsRelogin",
    "needsReloginReason",
];

/// 固定替换仍核对原生/secret/容器的非材料字段，防止新增实现洗掉身份/原OAuth或未知字段。
/// 公开视图不允许旧/新敏感材料回显；只借用两份外层保留原行，无JSON克隆和锁内销毁。
fn validate_replacement_view(
    original: &StoredAccount,
    next: &StoredAccount,
) -> Result<(), SlotFailure> {
    fn same(left: &Map<String, Value>, right: &Map<String, Value>, containers: bool) -> bool {
        for (name, value) in left {
            if MATERIAL.contains(&name.as_str()) {
                continue;
            }
            if containers
                && matches!(
                    name.as_str(),
                    "credential" | "codearts_provider_credential" | "auth"
                )
            {
                let (Some(left), Some(right)) = (
                    value.as_object(),
                    right.get(name).and_then(Value::as_object),
                ) else {
                    return false;
                };
                if !same(left, right, false) {
                    return false;
                }
            } else if right.get(name) != Some(value) {
                return false;
            }
        }
        right
            .keys()
            .all(|name| MATERIAL.contains(&name.as_str()) || left.contains_key(name))
    }
    let old_secret = original.get("pluginSecret").and_then(Value::as_object);
    let new_secret = next.get("pluginSecret").and_then(Value::as_object);
    match (old_secret, new_secret) {
        (Some(left), Some(right)) if same(left, right, true) => {}
        (None, None) => {}
        _ => return Err(SlotFailure::Invalid),
    }
    for name in ["credential", "codearts_provider_credential", "auth"] {
        match (original.get(name), next.get(name)) {
            (Some(Value::Object(left)), Some(Value::Object(right))) if same(left, right, false) => {
            }
            (None, None) => {}
            _ => return Err(SlotFailure::Invalid),
        }
    }
    let public = next
        .get("pluginPublic")
        .and_then(Value::as_object)
        .ok_or(SlotFailure::Invalid)?;
    fn sensitive(name: &str) -> bool {
        let name = Zeroizing::new(name.to_ascii_lowercase().replace(['_', '-'], ""));
        let name = name.as_str();
        name.contains("token")
            || name.contains("password")
            || name.contains("secret")
            || name.contains("cookie")
            || name.contains("authorization")
            || name.contains("privatekey")
            || name == "oauthcontext"
            || name == "auth"
            || name == "key"
            || name == "accesskeyid"
            || name == "codeverifier"
            || name == "d"
    }
    fn collect<'a>(
        fields: &'a Map<String, Value>,
        secrets: &mut Vec<&'a str>,
        protected: bool,
    ) -> Result<(), SlotFailure> {
        for (name, value) in fields {
            gather(value, secrets, protected || sensitive(name))?;
        }
        Ok(())
    }
    fn gather<'a>(
        value: &'a Value,
        secrets: &mut Vec<&'a str>,
        protected: bool,
    ) -> Result<(), SlotFailure> {
        match value {
            Value::Object(fields) => collect(fields, secrets, protected),
            Value::Array(items) => {
                for value in items {
                    gather(value, secrets, protected)?;
                }
                Ok(())
            }
            Value::String(text) if protected && !text.is_empty() => {
                if secrets.len() >= 4096 {
                    return Err(Error::LimitExceeded.into());
                }
                secrets.push(text);
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn scalar(name: &str, value: &Value) -> bool {
        match name {
            "hasAccessToken" | "hasRefreshToken" | "hasToken" | "hasCredentials" | "canRefresh"
            | "available" | "needsRelogin" => value.is_boolean(),
            "expiresAt"
            | "expiresAtMs"
            | "tokenExpireAt"
            | "tokenExpiresAt"
            | "accessTokenExpiresAt"
            | "refreshTokenExpiresAt" => {
                value.is_null() || value.as_i64().is_some_and(|value| value > 0)
            }
            "tokenTail" => {
                value.is_null()
                    || value.as_str().is_some_and(|text| {
                        text.chars().count() <= 6 && !text.chars().any(char::is_control)
                    })
            }
            "needsReloginReason" => {
                value.is_null()
                    || value.as_str().is_some_and(|text| {
                        text.len() <= 2048 && !text.chars().any(char::is_control)
                    })
            }
            _ => false,
        }
    }
    fn check(value: &Value, secrets: &[&str]) -> bool {
        match value {
            Value::Object(fields) => fields.iter().all(|(name, value)| {
                let private =
                    crate::server::core::account_store::credential_version::private_field(name);
                !private
                    && (!sensitive(name)
                        || PUBLIC_SCALARS.contains(&name.as_str()) && scalar(name, value))
                    && check(value, secrets)
            }),
            Value::Array(items) => items.iter().all(|value| check(value, secrets)),
            Value::String(text) => !secrets.iter().any(|secret| text.contains(secret)),
            _ => true,
        }
    }
    let mut secrets = Vec::new();
    for row in [original, next] {
        for name in MATERIAL {
            if let Some(value) = row.get(name) {
                gather(value, &mut secrets, true)?;
            }
        }
        for name in [
            "oauth_context",
            "pluginSecret",
            "credential",
            "codearts_provider_credential",
            "auth",
        ] {
            if let Some(value) = row.get(name) {
                gather(
                    value,
                    &mut secrets,
                    matches!(name, "oauth_context" | "auth"),
                )?;
            }
        }
    }
    if !public.iter().all(|(name, value)| {
        !crate::server::core::account_store::credential_version::private_field(name)
            && (!sensitive(name) || PUBLIC_SCALARS.contains(&name.as_str()) && scalar(name, value))
            && check(value, &secrets)
    }) {
        return Err(SlotFailure::Invalid);
    }
    Ok(())
}

const ALIASES: &[&[&str]] = &[
    &["access_key_id", "accessKeyId", "accessToken"],
    &["secret_access_key", "secretAccessKey"],
    &["security_token", "securityToken"],
    &["refresh_token", "refreshToken"],
    &["expiration", "expires_at"],
];

pub(in crate::server::core::account_store) fn prepare_replacement(
    credentials: &Value,
) -> Result<Replacement, SlotFailure> {
    // 纯校验由PendingSlot门面在取任何存储锁之前调用，候选从不构造来源或发送回执。
    bounded::check(credentials, bounded::MAX_SECRET_BYTES)?;
    let fields = credentials
        .as_object()
        .ok_or(SlotFailure::IncompleteReplacement)?;
    if fields
        .keys()
        .any(|key| key != "expiresAt" && !ALIASES.iter().any(|group| group.contains(&key.as_str())))
    {
        return Err(SlotFailure::IncompleteReplacement);
    }
    let mut replacement = Replacement {
        fields: Map::new(),
        expires_at: 0,
    };
    for (index, aliases) in ALIASES.iter().enumerate() {
        let text = replacement_text(fields, aliases, if index == 4 { 1024 } else { 32 * 1024 })?;
        if matches!(index, 0 | 2) && !text.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
            return Err(SlotFailure::IncompleteReplacement);
        }
        for name in *aliases {
            replacement
                .fields
                .insert((*name).to_string(), Value::String(text.to_string()));
        }
        if index == 4 {
            let expires = chrono::DateTime::parse_from_rfc3339(text)
                .map_err(|_| SlotFailure::IncompleteReplacement)?
                .timestamp_millis();
            let now = crate::server::logging::now_ms();
            if now <= 0
                || expires <= now
                || fields
                    .get("expiresAt")
                    .is_some_and(|value| value.as_i64() != Some(expires))
            {
                return Err(SlotFailure::IncompleteReplacement);
            }
            replacement.expires_at = expires;
            replacement
                .fields
                .insert("expiresAt".to_string(), Value::from(expires));
        }
    }
    drop(Zeroizing::new(bounded::encode_fields(
        &replacement.fields,
        bounded::MAX_SECRET_BYTES,
    )?));
    Ok(replacement)
}

fn replacement_text<'a>(
    fields: &'a Map<String, Value>,
    aliases: &[&str],
    maximum: usize,
) -> Result<&'a str, SlotFailure> {
    let mut found = None;
    for name in aliases {
        if let Some(value) = fields.get(*name) {
            let text = value.as_str().ok_or(SlotFailure::IncompleteReplacement)?;
            if text.is_empty()
                || text.len() > maximum
                || text.trim() != text
                || text.chars().any(char::is_control)
                || found.is_some_and(|previous| previous != text)
            {
                return Err(SlotFailure::IncompleteReplacement);
            }
            found = Some(text);
        }
    }
    found.ok_or(SlotFailure::IncompleteReplacement)
}
