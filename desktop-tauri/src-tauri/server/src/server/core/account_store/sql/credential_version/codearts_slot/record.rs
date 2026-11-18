//! 固定格式的加密槽数据，不是授权对象。持久化内容只含行绑定、随机代次和状态。
//! 读取缺失、坏格式、未知状态都不补 Ready；不解密或枚举任何其它账号。

use rusqlite::{types::ValueRef, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zeroize::{Zeroize, Zeroizing};

use super::super::{bounded, check_id, projection::Projection, storage, Error, StorageFailure};
use super::SlotFailure;
use crate::server::core::{account_store::state::StoredAccount, crypto};
use crate::server::db::schema::CODEARTS_REFRESH_SLOT_SCHEMA;

const MAX_ENVELOPE: usize = 8192;
const MAX_PLAIN: usize = 4096;

/// 原行/新行/读取投影/被替换字段在外层事务拥有者内保留，所有账号与Db锁退出后才析构。
/// SQL只借用此对象；最多固定三条账号行和四个投影，不枚举或跨提供商读取。
pub(in crate::server::core::account_store) struct RetainedRows {
    accounts: Vec<StoredAccount>,
    projections: Vec<Projection>,
    values: Vec<Value>,
}

impl RetainedRows {
    pub(in crate::server::core::account_store) fn new() -> Self {
        Self {
            accounts: Vec::with_capacity(3),
            projections: Vec::with_capacity(4),
            values: Vec::new(),
        }
    }
    pub(super) fn account(&self, index: usize) -> &StoredAccount {
        &self.accounts[index]
    }
    pub(super) fn keep_projection(&mut self, index: usize) -> Result<usize, SlotFailure> {
        if self.projections.len() >= 4 {
            return Err(Error::LimitExceeded.into());
        }
        let projection = self.read_projection(index)?;
        let index = self.projections.len();
        self.projections.push(projection);
        Ok(index)
    }
    pub(super) fn read_projection(&mut self, index: usize) -> Result<Projection, SlotFailure> {
        // 先在外层保留的底稿上完成所有会失败的投影形状检查，避免Projection::read
        // 已克隆plugin/native之后才因坏public或超预算早退，在锁内释放那份克隆。
        let fields = self.account(index).fields();
        match fields.get("pluginSecret").filter(|value| !value.is_null()) {
            Some(value) if value.is_object() => {
                bounded::check(value, bounded::MAX_SECRET_BYTES)?;
                super::super::projection::reject_reserved(value)?;
            }
            Some(_) => return Err(SlotFailure::Invalid),
            None => {}
        }
        if fields
            .get("pluginPublic")
            .is_some_and(|value| !value.is_null() && !value.is_object())
        {
            return Err(SlotFailure::Invalid);
        }
        const NATIVE: &[&str] = &[
            "access_key_id",
            "secret_access_key",
            "security_token",
            "expires_at",
            "expiration",
            "refresh_token",
            "oauth_context",
            "accessToken",
            "refreshToken",
            "expiresAt",
            "domain_id",
            "account_id",
            "user_id",
            "principal_id",
            "user_name",
            "principal_urn",
            "login_type",
            "domainId",
            "userId",
            "userName",
            "loginType",
            "accessKeyId",
            "secretAccessKey",
            "securityToken",
            "credential",
            "codearts_provider_credential",
            "auth",
        ];
        let native = fields
            .iter()
            .filter(|(name, _)| NATIVE.contains(&name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let native = self.keep_value(Value::Object(native));
        bounded::check(&self.values[native], bounded::MAX_SECRET_BYTES)?;
        super::super::projection::reject_reserved(&self.values[native])?;
        if let Some(public) = self
            .account(index)
            .get("pluginPublic")
            .filter(|value| !value.is_null())
        {
            bounded::check(public, bounded::MAX_PUBLIC_BYTES)?;
        }
        Projection::read(self.account(index)).map_err(SlotFailure::from)
    }
    pub(super) fn projection(&self, index: usize) -> &Projection {
        &self.projections[index]
    }
    fn keep_value(&mut self, value: Value) -> usize {
        let index = self.values.len();
        self.values.push(value);
        index
    }
    fn parts(&mut self, index: usize) -> (&mut StoredAccount, &mut Vec<Value>) {
        (&mut self.accounts[index], &mut self.values)
    }
}

impl Drop for RetainedRows {
    fn drop(&mut self) {
        for account in &mut self.accounts {
            wipe_fields(account.fields_mut());
        }
        for value in &mut self.values {
            wipe_value(value);
        }
        // 私有Projection未提供就地清理API，只能在此锁外释放，不冒称全部内存已擦除。
    }
}

pub(super) fn wipe_fields(fields: &mut Map<String, Value>) {
    for (mut name, mut value) in std::mem::take(fields) {
        name.zeroize();
        wipe_value(&mut value);
    }
}
fn wipe_value(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Object(fields) => wipe_fields(fields),
        Value::Array(items) => items.iter_mut().for_each(wipe_value),
        _ => {}
    }
    *value = Value::Null;
}

/// 严格原加密账号行。普通storage::load的明文兼容不用于Ready读取/占用/结清。
/// 读取/校验失败的已解密业务内容也保留给外层，不在本同步账号/Db锁内析构。
pub(super) fn load_account(
    conn: &Connection,
    key: &[u8; 32],
    id: &str,
    retained: &mut RetainedRows,
) -> Result<usize, SlotFailure> {
    if conn.is_autocommit() {
        return Err(Error::Storage(StorageFailure::Begin).into());
    }
    check_id(id)?;
    storage::ensure_no_triggers(conn)?;
    if retained.accounts.len() >= 3 {
        return Err(Error::LimitExceeded.into());
    }
    let mut statement = conn
        .prepare(
            "SELECT CASE WHEN typeof(data)='text' AND octet_length(data)<=?2 THEN data END,
            CASE WHEN typeof(provider)='text' AND octet_length(provider)<=128 THEN provider END,
            enabled, priority, added_at
         FROM main.accounts WHERE id=?1 LIMIT 2",
        )
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    let mut rows = statement
        .query(rusqlite::params![id, bounded::MAX_ROW_BYTES as i64])
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    let row = rows
        .next()
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?
        .ok_or(Error::Conflict(super::super::Conflict::MissingAccount))?;
    let encrypted = text(row, 0, bounded::MAX_ROW_BYTES)?;
    if !crypto::is_envelope(encrypted) {
        return Err(SlotFailure::Invalid);
    }
    if text(row, 1, 128)? != "codearts" {
        return Err(Error::Conflict(super::super::Conflict::ProviderChanged).into());
    }
    let enabled = integer(row, 2)?;
    let priority = integer(row, 3)?;
    let added_at = integer(row, 4)?;
    if !matches!(enabled, 0 | 1) {
        return Err(SlotFailure::Invalid);
    }
    let plaintext =
        Zeroizing::new(crypto::open_with_key(encrypted, key).map_err(|_| SlotFailure::Invalid)?);
    let parsed = retained.keep_value(bounded::parse(&plaintext, bounded::MAX_PLAIN_BYTES)?);
    let fields = retained.values[parsed]
        .as_object()
        .ok_or(SlotFailure::Invalid)?;
    if fields.get("id").and_then(Value::as_str) != Some(id)
        || fields.get("provider").and_then(Value::as_str) != Some("codearts")
    {
        return Err(SlotFailure::Invalid);
    }
    // 兼容旧多层JSON只做真实完整解析；被替换的原字符串交给外层保留，不吞错误。
    for name in ["pluginSecret", "pluginPublic"] {
        let decoded = match retained.values[parsed].get(name) {
            Some(Value::String(raw)) => Some(bounded::parse(
                raw.as_bytes(),
                if name == "pluginSecret" {
                    bounded::MAX_SECRET_BYTES
                } else {
                    bounded::MAX_PUBLIC_BYTES
                },
            )?),
            _ => None,
        };
        if let Some(decoded) = decoded {
            let decoded = retained.keep_value(decoded);
            if !retained.values[decoded].is_object() {
                return Err(SlotFailure::Invalid);
            }
            let value = std::mem::replace(&mut retained.values[decoded], Value::Null);
            let Some(fields) = retained.values[parsed].as_object_mut() else {
                return Err(SlotFailure::Invalid);
            };
            let previous = fields.insert(name.to_string(), value);
            if let Some(previous) = previous {
                retained.keep_value(previous);
            }
        }
    }
    bounded::shape_check(&retained.values[parsed])?;
    let value = std::mem::replace(&mut retained.values[parsed], Value::Null);
    let Value::Object(fields) = value else {
        return Err(SlotFailure::Invalid);
    };
    let index = retained.accounts.len();
    retained.accounts.push(StoredAccount::from_map(fields));
    let account = retained.account(index);
    if account
        .get("enabled")
        .is_some_and(|value| !value.is_boolean())
        || i64::from(account.enabled()) != enabled
        || account
            .get("priority")
            .is_some_and(|value| value.as_i64().is_none())
        || account.priority() != priority
        || account
            .get("addedAt")
            .is_some_and(|value| value.as_i64().is_none())
        || account.added_at() != added_at
    {
        return Err(SlotFailure::Invalid);
    }
    if rows
        .next()
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?
        .is_some()
    {
        return Err(SlotFailure::Invalid);
    }
    drop(rows);
    statement
        .finalize()
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    Ok(index)
}

fn integer(row: &rusqlite::Row<'_>, column: usize) -> Result<i64, SlotFailure> {
    match row
        .get_ref(column)
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?
    {
        ValueRef::Integer(value) => Ok(value),
        _ => Err(SlotFailure::Invalid),
    }
}

const CONTAINERS: &[&str] = &["credential", "codearts_provider_credential", "auth"];

/// 保留原行，另建本次新行；只覆盖五项已有材料，原身份/OAuth/代理/运营属性均不变。
pub(super) fn apply_replacement(
    retained: &mut RetainedRows,
    original: usize,
    fields: &Map<String, Value>,
    expires: i64,
) -> Result<usize, SlotFailure> {
    if retained.accounts.len() >= 3 {
        return Err(Error::LimitExceeded.into());
    }
    let original = retained.account(original);
    let plugin = original
        .get("pluginSecret")
        .is_some_and(|value| !value.is_null());
    if plugin && !original.get("pluginSecret").is_some_and(Value::is_object) {
        return Err(SlotFailure::Invalid);
    }
    let next = original.clone();
    let index = retained.accounts.len();
    retained.accounts.push(next);
    let (record, discarded) = retained.parts(index);
    let public = match record.get("pluginPublic").filter(|value| !value.is_null()) {
        Some(Value::Object(fields)) => fields.clone(),
        Some(_) => return Err(SlotFailure::Invalid),
        None => Map::new(),
    };
    let public = retain_object(public, discarded);
    let secret = match record.get("pluginSecret").filter(|value| !value.is_null()) {
        Some(Value::Object(secret)) => Some(secret.clone()),
        _ => None,
    };
    if let Some(secret) = secret {
        let secret = retain_object(secret, discarded);
        update_materials(discarded, secret, fields)?;
        let secret = std::mem::replace(&mut discarded[secret], Value::Null);
        replace(record.fields_mut(), "pluginSecret", secret, discarded);
    }
    for (name, value) in material_updates(record.fields(), fields, !plugin) {
        replace(record.fields_mut(), &name, value, discarded);
    }
    for name in CONTAINERS {
        if let Some(value) = record.get(name) {
            let Some(container) = value.as_object() else {
                return Err(SlotFailure::Invalid);
            };
            let container = retain_object(container.clone(), discarded);
            update_container(discarded, container, fields, name == &"auth")?;
            let value = std::mem::replace(&mut discarded[container], Value::Null);
            replace(record.fields_mut(), name, value, discarded);
        }
    }
    for name in [
        "expiresAt",
        "expiresAtMs",
        "tokenExpireAt",
        "tokenExpiresAt",
        "accessTokenExpiresAt",
    ] {
        let Some(fields) = discarded[public].as_object_mut() else {
            return Err(SlotFailure::Invalid);
        };
        if fields.contains_key(name) || name == "expiresAt" || name == "accessTokenExpiresAt" {
            let previous = fields.insert(name.to_string(), Value::from(expires));
            if let Some(previous) = previous {
                discarded.push(previous);
            }
        }
    }
    if let Some(public) = discarded[public].as_object_mut() {
        public.insert("canRefresh".into(), Value::Bool(true));
        public.insert("refreshRequestSent".into(), Value::Bool(true));
        public.insert("refreshStatus".into(), Value::String("host_managed".into()));
        if public.contains_key("refreshTokenExpiresAt") {
            public.insert("refreshTokenExpiresAt".into(), Value::Null);
        }
    }
    let public = std::mem::replace(&mut discarded[public], Value::Null);
    replace(record.fields_mut(), "pluginPublic", public, discarded);
    Ok(index)
}

fn retain_object(fields: Map<String, Value>, retained: &mut Vec<Value>) -> usize {
    let index = retained.len();
    retained.push(Value::Object(fields));
    index
}
fn replace(fields: &mut Map<String, Value>, name: &str, value: Value, retained: &mut Vec<Value>) {
    if let Some(previous) = fields.insert(name.to_string(), value) {
        retained.push(previous);
    }
}
fn update_materials(
    retained: &mut Vec<Value>,
    index: usize,
    fields: &Map<String, Value>,
) -> Result<(), SlotFailure> {
    let target = retained[index].as_object().ok_or(SlotFailure::Invalid)?;
    let updates = material_updates(target, fields, true);
    for (name, value) in updates {
        let Some(target) = retained[index].as_object_mut() else {
            return Err(SlotFailure::Invalid);
        };
        let previous = target.insert(name, value);
        if let Some(previous) = previous {
            retained.push(previous);
        }
    }
    for name in CONTAINERS {
        let nested = match retained[index].get(name) {
            Some(Value::Object(nested)) => Some(nested.clone()),
            Some(_) => return Err(SlotFailure::Invalid),
            None => None,
        };
        if let Some(nested) = nested {
            let nested = retain_object(nested, retained);
            update_container(retained, nested, fields, name == &"auth")?;
            let value = std::mem::replace(&mut retained[nested], Value::Null);
            let Some(target) = retained[index].as_object_mut() else {
                return Err(SlotFailure::Invalid);
            };
            let previous = target.insert(name.to_string(), value);
            if let Some(previous) = previous {
                retained.push(previous);
            }
        }
    }
    Ok(())
}
fn update_container(
    retained: &mut Vec<Value>,
    index: usize,
    fields: &Map<String, Value>,
    existing_only: bool,
) -> Result<(), SlotFailure> {
    let target = retained[index].as_object().ok_or(SlotFailure::Invalid)?;
    let updates = material_updates(target, fields, !existing_only);
    for (name, value) in updates {
        let Some(target) = retained[index].as_object_mut() else {
            return Err(SlotFailure::Invalid);
        };
        let previous = target.insert(name, value);
        if let Some(previous) = previous {
            retained.push(previous);
        }
    }
    Ok(())
}

fn material_updates(target: &Map<String, Value>, fields: &Map<String, Value>, canonical: bool) -> Vec<(String, Value)> {
    let mut updates = Vec::new();
    for (name, value) in fields {
        if canonical || target.contains_key(name) { updates.push((name.clone(), value.clone())); }
        let aliases: &[&str] = match name.as_str() {
            "access_key_id" => &["accessKeyId", "accessToken"],
            "secret_access_key" => &["secretAccessKey"],
            "security_token" => &["securityToken"],
            "refresh_token" => &["refreshToken"],
            "expires_at" => &["expiration"],
            _ => &[],
        };
        for alias in aliases {
            if target.contains_key(*alias) { updates.push(((*alias).to_string(), value.clone())); }
        }
    }
    updates
}

pub(super) fn set_field(retained: &mut RetainedRows, index: usize, name: &str, value: Value) {
    let (record, discarded) = retained.parts(index);
    replace(record.fields_mut(), name, value, discarded);
}

/// 新行完整编码按有界字段借用，不为序列化临时克隆整份记录或调用普通upsert。
pub(super) fn write_account(
    conn: &Connection,
    key: &[u8; 32],
    retained: &RetainedRows,
    index: usize,
) -> Result<(), SlotFailure> {
    let record = retained.account(index);
    let bytes = Zeroizing::new(bounded::encode_fields(
        record.fields(),
        bounded::MAX_PLAIN_BYTES,
    )?);
    let encrypted = crypto::seal_with_key(&bytes, key)
        .map_err(|_| Error::Storage(StorageFailure::Encryption))?;
    if encrypted.len() > bounded::MAX_ROW_BYTES {
        return Err(Error::LimitExceeded.into());
    }
    let changed = conn
        .execute(
            "UPDATE main.accounts SET data=?3 WHERE id=?1 AND provider=?2",
            rusqlite::params![record.id(), "codearts", encrypted],
        )
        .map_err(|error| storage::sql_error(error, StorageFailure::Write))?;
    if changed != 1 {
        return Err(Error::Conflict(super::super::Conflict::MissingAccount).into());
    }
    Ok(())
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Slot {
    pub schema: u32,
    pub provider: String,
    pub account_id: String,
    pub db_epoch: String,
    pub born: String,
    pub revision: u64,
    pub generation: String,
    pub state: State,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum State {
    Ready { settled_operation: Option<String> },
    Pending { operation: String },
    Blocked { operation: String },
}

pub(super) fn initialize_empty(conn: &Connection) -> Result<(), SlotFailure> {
    if conn.is_autocommit() {
        return Err(Error::Storage(StorageFailure::Begin).into());
    }
    no_triggers(conn)?;
    conn.execute_batch(CODEARTS_REFRESH_SLOT_SCHEMA)
        .map_err(|error| storage::sql_error(error, StorageFailure::Write))?;
    validate_schema(conn)
}

fn no_triggers(conn: &Connection) -> Result<(), SlotFailure> {
    storage::ensure_no_triggers(conn)?;
    let found: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND tbl_name='codearts_refresh_slots')
             OR EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE type='trigger' AND tbl_name='codearts_refresh_slots')",
        [], |row| row.get(0),
    ).map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    if found != 0 {
        return Err(SlotFailure::Schema);
    }
    Ok(())
}

fn validate_schema(conn: &Connection) -> Result<(), SlotFailure> {
    no_triggers(conn)?;
    let table: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='codearts_refresh_slots' AND type='table')",
        [], |row| row.get(0),
    ).map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    if table != 1 {
        return Err(SlotFailure::Schema);
    }
    let mut statement = conn
        .prepare("PRAGMA main.table_xinfo(codearts_refresh_slots)")
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    let mut rows = statement
        .query([])
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    for (position, name, primary) in [(0i64, "account_id", 1i64), (1, "payload", 0)] {
        let row = rows
            .next()
            .map_err(|error| storage::sql_error(error, StorageFailure::Read))?
            .ok_or(SlotFailure::Schema)?;
        let integer = |index: usize| match row.get_ref(index) {
            Ok(ValueRef::Integer(value)) => Ok(value),
            _ => Err(SlotFailure::Schema),
        };
        if integer(0)? != position
            || text(row, 1, 32)? != name
            || text(row, 2, 16)? != "TEXT"
            || integer(3)? != 1
            || integer(5)? != primary
            || integer(6)? != 0
            || !matches!(row.get_ref(4), Ok(ValueRef::Null))
        {
            return Err(SlotFailure::Schema);
        }
    }
    if rows
        .next()
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?
        .is_some()
    {
        return Err(SlotFailure::Schema);
    }
    drop(rows);
    statement
        .finalize()
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    Ok(())
}

pub(super) fn load(conn: &Connection, key: &[u8; 32], id: &str) -> Result<Slot, SlotFailure> {
    if conn.is_autocommit() {
        return Err(Error::Storage(StorageFailure::Begin).into());
    }
    check_id(id)?;
    validate_schema(conn)?;
    let mut statement = conn.prepare(
        "SELECT CASE WHEN typeof(payload)='text' AND octet_length(payload) <= ?2 THEN payload END
         FROM main.codearts_refresh_slots WHERE account_id=?1 LIMIT 2",
    ).map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    let mut rows = statement
        .query(rusqlite::params![id, MAX_ENVELOPE as i64])
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    let row = rows
        .next()
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?
        .ok_or(SlotFailure::Missing)?;
    let envelope = text(row, 0, MAX_ENVELOPE)?;
    if envelope.trim() != envelope || !crypto::is_envelope(envelope) {
        return Err(SlotFailure::Invalid);
    }
    let plain =
        Zeroizing::new(crypto::open_with_key(envelope, key).map_err(|_| SlotFailure::Invalid)?);
    let value = bounded::parse(&plain, MAX_PLAIN)?;
    let slot: Slot = serde_json::from_value(value).map_err(|_| SlotFailure::Invalid)?;
    valid(&slot)?;
    if slot.account_id != id {
        return Err(SlotFailure::Invalid);
    }
    if rows
        .next()
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?
        .is_some()
    {
        return Err(SlotFailure::Invalid);
    }
    drop(rows);
    statement
        .finalize()
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    Ok(slot)
}

/// 缺表是真正尚未建立容器；坏结构不能当作缺槽。存在任何本账号行均拒绝，不解密判Ready。
pub(super) fn require_absent(conn: &Connection, id: &str) -> Result<(), SlotFailure> {
    if conn.is_autocommit() {
        return Err(Error::Storage(StorageFailure::Begin).into());
    }
    check_id(id)?;
    no_triggers(conn)?;
    let object: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='codearts_refresh_slots')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    if object == 0 {
        return Ok(());
    }
    validate_schema(conn)?;
    let exists: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.codearts_refresh_slots WHERE account_id=?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    if exists != 0 {
        return Err(SlotFailure::ReceiptChanged);
    }
    Ok(())
}

pub(super) fn remove_for_new_login(conn: &Connection, id: &str) -> Result<(), SlotFailure> {
    if conn.is_autocommit() { return Err(SlotFailure::Invalid); }
    check_id(id)?;
    let exists: i64 = conn.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='codearts_refresh_slots')",[],|row| row.get(0))
        .map_err(|_| SlotFailure::Invalid)?;
    if exists == 0 { return Ok(()); }
    validate_schema(conn)?;
    no_triggers(conn)?;
    conn.execute("DELETE FROM main.codearts_refresh_slots WHERE account_id=?1",[id]).map_err(|_| SlotFailure::Invalid)?;
    Ok(())
}

/// 仅固定首次登记内核调用：先核无槽，再INSERT一行；绝不UPSERT/清除旧槽或按ID解封。
pub(super) fn insert_initial(
    conn: &Connection,
    key: &[u8; 32],
    slot: &Slot,
) -> Result<(), SlotFailure> {
    require_absent(conn, &slot.account_id)?;
    initialize_empty(conn)?;
    valid(slot)?;
    if !matches!(
        &slot.state,
        State::Ready {
            settled_operation: None
        }
    ) {
        return Err(SlotFailure::Invalid);
    }
    let value = serde_json::to_value(slot).map_err(|_| SlotFailure::Invalid)?;
    let plain = Zeroizing::new(bounded::encode(&value, MAX_PLAIN)?);
    let envelope = crypto::seal_with_key(&plain, key)
        .map_err(|_| Error::Storage(StorageFailure::Encryption))?;
    if envelope.len() > MAX_ENVELOPE {
        return Err(Error::LimitExceeded.into());
    }
    let changed = conn
        .execute(
            "INSERT INTO main.codearts_refresh_slots(account_id,payload) VALUES(?1,?2)",
            rusqlite::params![slot.account_id, envelope],
        )
        .map_err(|error| storage::sql_error(error, StorageFailure::Write))?;
    if changed != 1 || load(conn, key, &slot.account_id)? != *slot {
        return Err(Error::Storage(StorageFailure::Write).into());
    }
    Ok(())
}

pub(super) fn update(conn: &Connection, key: &[u8; 32], slot: &Slot) -> Result<(), SlotFailure> {
    if conn.is_autocommit() {
        return Err(Error::Storage(StorageFailure::Begin).into());
    }
    validate_schema(conn)?;
    valid(slot)?;
    let value = serde_json::to_value(slot).map_err(|_| SlotFailure::Invalid)?;
    let plain = Zeroizing::new(bounded::encode(&value, MAX_PLAIN)?);
    let envelope = crypto::seal_with_key(&plain, key)
        .map_err(|_| Error::Storage(StorageFailure::Encryption))?;
    if envelope.len() > MAX_ENVELOPE {
        return Err(Error::LimitExceeded.into());
    }
    // 没有 INSERT/UPSERT；缺失的槽、被删除后同 ID 重建的账号都不能凭此被纳管。
    let changed = conn
        .execute(
            "UPDATE main.codearts_refresh_slots SET payload=?2 WHERE account_id=?1",
            rusqlite::params![slot.account_id, envelope],
        )
        .map_err(|error| storage::sql_error(error, StorageFailure::Write))?;
    if changed != 1 {
        return Err(SlotFailure::Missing);
    }
    if load(conn, key, &slot.account_id)? != *slot {
        return Err(Error::Storage(StorageFailure::Write).into());
    }
    Ok(())
}

fn valid(slot: &Slot) -> Result<(), SlotFailure> {
    if slot.schema != 1
        || slot.provider != "codearts"
        || check_id(&slot.account_id).is_err()
        || !random_id(&slot.db_epoch)
        || !random_id(&slot.born)
        || slot.revision == 0
        || !random_id(&slot.generation)
    {
        return Err(SlotFailure::Invalid);
    }
    let operation = match &slot.state {
        State::Ready { settled_operation } => settled_operation.as_deref(),
        State::Pending { operation } | State::Blocked { operation } => Some(operation.as_str()),
    };
    if operation.is_some_and(|value| !random_id(value) || value == slot.generation) {
        return Err(SlotFailure::Invalid);
    }
    Ok(())
}

fn random_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && value.bytes().any(|byte| byte != b'0')
}

fn text<'a>(
    row: &'a rusqlite::Row<'_>,
    column: usize,
    maximum: usize,
) -> Result<&'a str, SlotFailure> {
    let Ok(ValueRef::Text(bytes)) = row.get_ref(column) else {
        return Err(SlotFailure::Invalid);
    };
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(SlotFailure::Invalid);
    }
    std::str::from_utf8(bytes).map_err(|_| SlotFailure::Invalid)
}
