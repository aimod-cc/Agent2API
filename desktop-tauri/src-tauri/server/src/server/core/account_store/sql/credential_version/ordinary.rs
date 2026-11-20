//! 普通账号写者的固定三家版本收口。只认本事务原行，传入的私有字段永不回灌。
//! 没有来源签发或网络能力；保存点释放不代表外层COMMIT，恢复由原Db来源单向失效。

use std::borrow::Cow;

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use zeroize::Zeroizing;

use super::{
    bounded, maintained_provider, projection, row_projection, storage, Conflict, Error,
    StorageFailure, Version,
};
use crate::server::core::account_store::{
    credential_version::{self, RowProvider},
    state::{AccountState, StoredAccount},
};
use crate::server::core::crypto::{self, SecretScope};

/// 由SQL入口选定，不接受记录里的来源字段。Imported不掌握逐行的可信改动意图。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::server::core::account_store::sql) enum WriteKind {
    Update,
    Replace,
    Imported,
}

pub(in crate::server::core::account_store::sql) struct PreparedRow<'a> {
    record: Cow<'a, StoredAccount>,
    existed: bool,
    changed: bool,
}

impl PreparedRow<'_> {
    pub(in crate::server::core::account_store::sql) fn record(&self) -> &StoredAccount {
        &self.record
    }
    pub(in crate::server::core::account_store::sql) fn existed(&self) -> bool {
        self.existed
    }
    fn changed(&self) -> bool {
        self.changed
    }
}

/// 独立调用拥有IMMEDIATE事务；已在外层事务内时只拥有自己的局部保存点。
/// 仅SQL内部同步调用，既不取账号/配置锁，也不把连接或版本对象带出闭包。
pub(in crate::server::core::account_store::sql) fn atomic<T>(
    conn: &Connection,
    action: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    if conn.is_autocommit() {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        return finish(tx, action);
    }
    let mut savepoint = WriteSavepoint::begin(conn)?;
    match action(conn) {
        Ok(value) => {
            savepoint.release()?;
            Ok(value)
        }
        Err(error) => {
            savepoint.rollback()?;
            Err(error)
        }
    }
}

fn finish<T>(
    tx: Transaction<'_>,
    action: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    match action(&tx) {
        Ok(value) => {
            tx.commit()?;
            Ok(value)
        }
        Err(error) => {
            if !tx.is_autocommit() {
                tx.rollback()?;
            }
            Err(error)
        }
    }
}

/// rusqlite的Savepoint要求&mut Connection；普通行级SQL持有的是已串行化的&Connection。
/// 固定名称从不接输入；嵌套调用总是先回滚/释放最近的一层，不能提前提交调用者事务。
struct WriteSavepoint<'a> {
    conn: &'a Connection,
    active: bool,
}

impl<'a> WriteSavepoint<'a> {
    fn begin(conn: &'a Connection) -> rusqlite::Result<Self> {
        if conn.is_autocommit() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        conn.execute_batch("SAVEPOINT _host_account_credential_write")?;
        Ok(Self { conn, active: true })
    }

    fn release(&mut self) -> rusqlite::Result<()> {
        self.conn
            .execute_batch("RELEASE SAVEPOINT _host_account_credential_write")?;
        self.active = false;
        Ok(())
    }

    fn rollback(&mut self) -> rusqlite::Result<()> {
        // SQLite的严重错误可能已经回滚整个外层事务；不能因此再开事务或补写。
        if self.conn.is_autocommit() {
            self.active = false;
            return Ok(());
        }
        self.conn
            .execute_batch("ROLLBACK TO SAVEPOINT _host_account_credential_write")?;
        self.release()
    }
}

impl Drop for WriteSavepoint<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.rollback();
        }
    }
}

pub(in crate::server::core::account_store::sql) fn prepare<'a>(
    conn: &Connection,
    incoming: &'a StoredAccount,
    kind: WriteKind,
) -> rusqlite::Result<PreparedRow<'a>> {
    if conn.is_autocommit() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let current = load_current(conn, incoming.id())?;
    let provider = incoming.provider();
    let maintained = maintained_provider(&provider);
    let supplied_version = incoming.get(credential_version::PRIVATE_FIELD).is_some();
    if maintained
        || current
            .as_ref()
            .is_some_and(|row| maintained_provider(&row.record.provider()))
    {
        storage::ensure_no_triggers(conn).map_err(write_error)?;
    }
    if maintained && current.is_none() && supplied_version && kind == WriteKind::Update {
        return Err(write_error(Error::Conflict(Conflict::MissingAccount)));
    }
    let mut next = Cow::Borrowed(incoming);
    // provider改变也不能携带私有版本，稍后改回同provider不能恢复旧来源。
    if supplied_version {
        next.to_mut().remove(credential_version::PRIVATE_FIELD);
    }
    if let Some(current) = current.as_ref().filter(|row| row.version_source) {
        if maintained && current.record.provider() == provider {
            maintain(conn, &current.record, &mut next, kind).map_err(write_error)?;
        }
    }
    let changed = current
        .as_ref()
        .is_none_or(|row| row.record.fields() != next.fields());
    Ok(PreparedRow {
        record: next,
        existed: current.is_some(),
        changed,
    })
}

fn maintain(
    conn: &Connection,
    current: &StoredAccount,
    next: &mut Cow<'_, StoredAccount>,
    kind: WriteKind,
) -> Result<(), Error> {
    let version = match storage::version(current) {
        Ok(Some(version)) => version,
        Ok(None) | Err(Error::InvalidRecord) => return Ok(()),
        Err(error) => return Err(error),
    };
    // 缺失/不同epoch不当作第0代，也不自动把旧版本迁入当前库；只丢弃旧合同。
    if storage::epoch(conn)?.as_deref() != Some(version.epoch.as_str()) {
        return Ok(());
    }
    let before = authorization(current)?;
    let after = authorization(next)?;
    let (Some(before), Some(after)) = (before, after) else {
        // 普通凭据仍按原输入保存；无法证明同一授权时不留下可被复用的私有版本。
        return Ok(());
    };
    let version = if matches!(kind, WriteKind::Replace | WriteKind::Imported) {
        let born = storage::random_id()?;
        if born == version.born {
            return Err(Error::Storage(StorageFailure::RandomUnavailable));
        }
        Version {
            born,
            revision: 1,
            ..version
        }
    } else if before != after || reauthorization_changed(current, next) {
        Version {
            revision: version
                .revision
                .checked_add(1)
                .ok_or(Error::VersionExhausted)?,
            ..version
        }
    } else {
        version
    };
    // 唯一来源是刚才在同事务读出的真实行，不比较或采用传入的born/revision。
    storage::set_version(next.to_mut(), &version);
    // 私有标记也计入合同预算；加回标记后越界只撤去合同，不拒绝正常业务保存。
    match bounded::encode_fields(next.fields(), bounded::MAX_PLAIN_BYTES) {
        Ok(bytes) => drop(Zeroizing::new(bytes)),
        Err(Error::InvalidInput | Error::InvalidRecord | Error::LimitExceeded) => {
            next.to_mut().remove(credential_version::PRIVATE_FIELD);
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

#[derive(PartialEq)]
enum Authorization {
    CodeArts(projection::Projection),
    ReadOnly(row_projection::Projection),
}

fn authorization(record: &StoredAccount) -> Result<Option<Authorization>, Error> {
    let checked = (|| {
        // 预算只决定能否沿用受控合同；普通台账/未知字段超过预算仍应完整写回。
        // 先有界检查字段借用，再构造授权投影，避免为判断版本而克隆超大正文。
        drop(Zeroizing::new(bounded::encode_fields(
            record.fields(),
            bounded::MAX_PLAIN_BYTES,
        )?));
        let provider = record.provider();
        if provider == projection::PROVIDER {
            projection::Projection::read(record).map(Authorization::CodeArts)
        } else {
            let provider = RowProvider::parse(&provider).ok_or(Error::UnsupportedProvider)?;
            row_projection::Projection::read(record, provider).map(Authorization::ReadOnly)
        }
    })();
    match checked {
        Ok(projection) => Ok(Some(projection)),
        Err(Error::InvalidInput | Error::InvalidRecord | Error::LimitExceeded) => Ok(None),
        Err(error) => Err(error),
    }
}

fn reauthorization_changed(current: &StoredAccount, next: &StoredAccount) -> bool {
    // plugin_upsert在真实IMMEDIATE事务内由宿主推进；这里只用作失效信号，绝非授权来源。
    // 导入同名JSON也最多导致旧合同失效，不能签发或恢复版本。旧计数饱和后每次保守失效。
    let before = current.get("usageCredentialRevision");
    let after = next.get("usageCredentialRevision");
    before != after
        || [before, after]
            .into_iter()
            .flatten()
            .any(|value| value.as_u64().is_none_or(|revision| revision == u64::MAX))
}

struct CurrentRow {
    record: StoredAccount,
    // 仅用于判断是否可沿用版本，不构成普通读写的准入条件。
    version_source: bool,
}

/// 同事务读取真实原行；普通编解码沿用既有兼容契约，不把受控预算推广到所有账号。
fn load_current(conn: &Connection, id: &str) -> rusqlite::Result<Option<CurrentRow>> {
    let mut stmt =
        conn.prepare("SELECT data, provider FROM main.accounts WHERE id = ?1 LIMIT 1")?;
    let mut rows = stmt.query([id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let raw: String = row.get(0)?;
    let record = super::super::decode_data(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            error.describe().into(),
        )
    })?;
    let provider = record.get("provider").and_then(serde_json::Value::as_str);
    let version_source = crypto::is_envelope(&raw)
        && raw.len() <= bounded::MAX_ROW_BYTES
        && record.id() == id
        && provider.is_some_and(maintained_provider)
        && matches!(row.get_ref(1)?, rusqlite::types::ValueRef::Text(column) if provider.is_some_and(|provider| column == provider.as_bytes()));
    // JSON是业务权威；历史投影不一致只撤去本合同资格，普通UPDATE仍可修复投影列。
    drop(rows);
    stmt.finalize()?;
    Ok(Some(CurrentRow {
        record,
        version_source,
    }))
}

/// 排序读失败不能把受控版本维护降级成“没有这条”；其他家保留原来的坏行跳过行为。
pub(in crate::server::core::account_store::sql) fn priority_record(
    conn: &Connection,
    id: &str,
) -> rusqlite::Result<Option<StoredAccount>> {
    let provider: Option<String> = conn
        .query_row(
            "SELECT provider FROM main.accounts WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(provider) = provider else {
        return Ok(None);
    };
    if maintained_provider(&provider) {
        return load_current(conn, id).map(|row| row.map(|row| row.record));
    }
    let raw: String = conn.query_row(
        "SELECT data FROM main.accounts WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?;
    Ok(super::super::decode_data(&raw).ok())
}

/// 删除不单独留下版本墓碑；同事务删除整条记录，任何重建都必须经过不信任输入的写入口。
pub(in crate::server::core::account_store::sql) fn delete(
    conn: &Connection,
    id: &str,
) -> rusqlite::Result<bool> {
    atomic(conn, |conn| {
        let provider: Option<String> = conn
            .query_row(
                "SELECT provider FROM main.accounts WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if provider.as_deref().is_some_and(maintained_provider) {
            storage::ensure_no_triggers(conn).map_err(write_error)?;
        }
        Ok(conn.execute("DELETE FROM main.accounts WHERE id = ?1", [id])? > 0)
    })
}

fn removable_from_state(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let provider: Option<String> = conn
        .query_row(
            "SELECT provider FROM main.accounts WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(provider) = provider else {
        return Ok(false);
    };
    if maintained_provider(&provider) {
        // 受控原行的真实解密/解析/读取错误仍使事务失败；超出受控预算不属于坏行。
        return load_current(conn, id).map(|record| record.is_some());
    }
    let raw: String = conn.query_row(
        "SELECT data FROM main.accounts WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?;
    // 其他家的旧全量读会过滤坏行；继续保留这些行，不把一次批量修改变成清库。
    Ok(super::super::decode_value(&raw).is_ok())
}

/// 整份输入仍差异写，正常save保留纯运营改动；公开save_locked按不可信整份替换失效。
/// 未取得逐行输入见证前，Imported会保守失效输入集合内固定三家的已有版本。
pub(in crate::server::core::account_store::sql) fn save_state(
    conn: &mut Connection,
    state: &AccountState,
    kind: WriteKind,
) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    finish(tx, |conn| {
        let ids = {
            let mut stmt = conn.prepare("SELECT id FROM main.accounts")?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            stmt.finalize()?;
            ids
        };
        // 整份状态应是id集合；不把同批重复输入悄悄折叠成最后一条覆盖。
        let mut seen = std::collections::HashSet::new();
        for record in &state.accounts {
            if !seen.insert(record.id()) {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        for id in ids {
            if state.accounts.iter().any(|record| record.id() == id) {
                continue;
            }
            if removable_from_state(conn, &id)? {
                super::super::delete(conn, &id)?;
            }
        }
        for incoming in &state.accounts {
            let prepared = prepare(conn, incoming, kind)?;
            if !prepared.existed() {
                super::super::insert_row(conn, prepared.record())?;
            } else if prepared.changed() {
                // 直接写本次已处理的版本，不再次走普通失效，避免重复推进。
                super::super::update_row(conn, prepared.record())?;
            }
        }
        if let Some(scope) = &state.priority_scope {
            conn.execute(
                "INSERT INTO main.kv (key, value) VALUES ('priorityScope', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![scope],
            )?;
        }
        Ok(())
    })
}

/// 旧明文迁移也不能把外来私有字段加密后包装成可信版本；读旧值与写新密文同一事务。
pub(in crate::server::core::account_store::sql) fn encrypt_legacy_rows(
    conn: &mut Connection,
) -> rusqlite::Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    finish(tx, |conn| {
        let pending = {
            let mut stmt = conn.prepare("SELECT id, provider, data FROM main.accounts")?;
            let mut rows = stmt.query([])?;
            let mut pending = Vec::new();
            while let Some(row) = rows.next()? {
                let raw: String = row.get(2)?;
                // 已加密行不参与迁移，不解密、不解析，也不受续期合同的大小预算限制。
                if crypto::is_envelope(&raw) {
                    continue;
                }
                pending.push((row.get::<_, String>(0)?, row.get::<_, String>(1)?, raw));
            }
            drop(rows);
            stmt.finalize()?;
            pending
        };
        for (id, provider, raw) in &pending {
            // 旧明文的外来私有标记只能剥离，不能加密成受控续期资格。
            let envelope = if maintained_provider(provider) {
                storage::ensure_no_triggers(conn).map_err(write_error)?;
                let mut value = super::super::decode_value(raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        error.describe().into(),
                    )
                })?;
                let removed = value.as_object_mut().is_some_and(|fields| {
                    fields.remove(credential_version::PRIVATE_FIELD).is_some()
                });
                if removed {
                    let bytes =
                        Zeroizing::new(serde_json::to_vec(&value).map_err(|error| {
                            rusqlite::Error::ToSqlConversionFailure(error.into())
                        })?);
                    crypto::seal(&bytes, SecretScope::Accounts)
                } else {
                    // 无私有标记的旧行保持原始明文字节，不顺手归一化业务内容。
                    crypto::seal(raw.as_bytes(), SecretScope::Accounts)
                }
            } else {
                crypto::seal(raw.as_bytes(), SecretScope::Accounts)
            }
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(error.into()))?;
            let changed = conn.execute(
                "UPDATE main.accounts SET data = ?2 WHERE id = ?1",
                params![id, envelope],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
        }
        Ok(pending.len())
    })
}

fn write_error(error: Error) -> rusqlite::Error {
    // 固定枚举不包含账号正文、私有版本、令牌或底层解密载荷。
    rusqlite::Error::ToSqlConversionFailure(
        format!("账号凭据版本维护失败，本次未确认保存（{error:?}）").into(),
    )
}
