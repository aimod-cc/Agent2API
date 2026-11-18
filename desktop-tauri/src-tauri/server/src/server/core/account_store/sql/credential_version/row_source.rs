//! 两家只读行来源只由真实单行IMMEDIATE事务提交后产生；不带账号写入或执行许可。

use rusqlite::{Connection, TransactionBehavior};
use serde_json::Value;

use super::{row_projection::Projection, storage, Conflict, Error, StorageFailure, Version};
use crate::server::core::account_store::{credential_version::RowProvider, state::StoredAccount};

pub(in crate::server::core::account_store) struct RowStamp {
    provider: RowProvider,
    id: String,
    version: Version,
    projection: Projection,
    account_maintenance: bool,
}

impl RowStamp {
    pub(in crate::server::core::account_store) fn provider(&self) -> RowProvider {
        self.provider
    }
    pub(in crate::server::core::account_store) fn account(&self) -> &str {
        &self.id
    }
    pub(in crate::server::core::account_store) fn secret(&self) -> &Value {
        self.projection.secret()
    }
    pub(in crate::server::core::account_store) fn identity(&self) -> &str {
        self.projection.identity()
    }
    pub(in crate::server::core::account_store) fn user_agent(&self) -> Option<&str> {
        self.projection.user_agent()
    }

    /// 仅在原来源已验证的只读事务内重读原行；不接受替代账号、外来记录或新epoch。
    pub(in crate::server::core::account_store) fn verify_current(
        &self,
        conn: &Connection,
        key: &[u8; 32],
    ) -> Result<(), Error> {
        if conn.is_autocommit() {
            return Err(Error::Storage(StorageFailure::Begin));
        }
        let epoch = storage::epoch(conn)?.ok_or(Error::Conflict(Conflict::DatabaseEpochChanged))?;
        if epoch != self.version.epoch {
            return Err(Error::Conflict(Conflict::DatabaseEpochChanged));
        }
        let current = storage::load_for(conn, key, self.provider.id(), &self.id)?;
        self.compare(&current, &epoch)
    }

    /// 只供固定豆包SQL借用同事务已读取并保留的原始行；不读取第二份大正文或签发来源。
    /// plugin/public仅是该原行内兼容JSON文本的严格解码借用，调用者必须持到锁外收尾。
    pub(in crate::server::core::account_store::sql::credential_version) fn verify_doubao_loaded(
        &self,
        conn: &Connection,
        raw: &Value,
        plugin: Option<&Value>,
        public: Option<&Value>,
    ) -> Result<(), Error> {
        if self.provider != RowProvider::DoubaoWork || conn.is_autocommit() {
            return Err(Error::UnsupportedProvider);
        }
        let epoch = storage::epoch(conn)?.ok_or(Error::Conflict(Conflict::DatabaseEpochChanged))?;
        if epoch != self.version.epoch {
            return Err(Error::Conflict(Conflict::DatabaseEpochChanged));
        }
        let fields = raw.as_object().ok_or(Error::InvalidRecord)?;
        if fields.get("id").and_then(Value::as_str) != Some(self.id.as_str())
            || fields.get("provider").and_then(Value::as_str) != Some(self.provider.id())
        {
            return Err(Error::Conflict(Conflict::ProviderChanged));
        }
        let version = fields.get(crate::server::core::account_store::credential_version::PRIVATE_FIELD)
            .and_then(Value::as_object).ok_or(Error::InvalidRecord)?;
        if version.len() != 4
            || version.get("schema").and_then(Value::as_u64) != Some(1)
            || version.get("dbEpoch").and_then(Value::as_str) != Some(self.version.epoch.as_str())
            || version.get("born").and_then(Value::as_str) != Some(self.version.born.as_str())
            || version.get("revision").and_then(Value::as_u64) != Some(self.version.revision)
        {
            return Err(Error::Conflict(Conflict::VersionChanged));
        }
        let (enabled, priority, added_at): (i64, i64, i64) = conn.query_row(
            "SELECT enabled, priority, added_at FROM main.accounts WHERE id=?1 AND provider=?2",
            rusqlite::params![self.id, self.provider.id()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
        let expected_enabled = match fields.get("enabled") {
            None | Some(Value::Bool(true)) => 1,
            Some(Value::Bool(false)) => 0,
            _ => return Err(Error::InvalidRecord),
        };
        let expected_priority = match fields.get("priority") {
            None => crate::server::core::account_store::priority::DEFAULT_PRIORITY,
            Some(value) => value.as_i64().ok_or(Error::InvalidRecord)?,
        };
        let expected_added = match fields.get("addedAt") {
            None => 0,
            Some(value) => value.as_i64().ok_or(Error::InvalidRecord)?,
        };
        if enabled != expected_enabled || !self.account_maintenance && expected_enabled != 1
            || priority != expected_priority || added_at != expected_added {
            return Err(Error::InvalidRecord);
        }
        if !self.projection.matches_doubao_fields(fields, plugin, public)? {
            return Err(Error::Conflict(Conflict::AuthorizationChanged));
        }
        Ok(())
    }

    /// 纯比较只留在本模块；最终检查不得从调用者提供的JSON或epoch签发来源。
    fn compare(&self, current: &StoredAccount, epoch: &str) -> Result<(), Error> {
        if current.id() != self.id || current.provider() != self.provider.id() {
            return Err(Error::Conflict(Conflict::ProviderChanged));
        }
        if !self.account_maintenance && !current.enabled() {
            return Err(Error::Conflict(Conflict::AuthorizationChanged));
        }
        if epoch != self.version.epoch {
            return Err(Error::Conflict(Conflict::DatabaseEpochChanged));
        }
        let version =
            storage::version(current)?.ok_or(Error::Conflict(Conflict::UnestablishedVersion))?;
        if version != self.version {
            return Err(Error::Conflict(Conflict::VersionChanged));
        }
        if Projection::read(current, self.provider)? != self.projection {
            return Err(Error::Conflict(Conflict::AuthorizationChanged));
        }
        Ok(())
    }
}

pub(in crate::server::core::account_store) fn prepare(
    conn: &mut Connection,
    key: &[u8; 32],
    provider: RowProvider,
    id: &str,
    account_maintenance: bool,
) -> Result<RowStamp, Error> {
    super::check_id(id)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage::sql_error(error, StorageFailure::Begin))?;
    let result = (|| {
        let mut record = storage::load_for(&tx, key, provider.id(), id)?;
        if !account_maintenance && !record.enabled() {
            return Err(Error::Conflict(Conflict::AuthorizationChanged));
        }
        let projection = Projection::read(&record, provider)?;
        let existing = storage::version(&record)?;
        let epoch = storage::epoch(&tx)?;
        let (version, establish) = match (existing, epoch) {
            (Some(version), Some(epoch)) if version.epoch == epoch => (version, false),
            (previous, epoch) => {
                let epoch = match epoch {
                    Some(epoch) => epoch,
                    None => storage::establish_epoch(&tx)?,
                };
                let born = storage::random_id()?;
                if previous
                    .as_ref()
                    .is_some_and(|version| version.born == born)
                {
                    return Err(Error::Storage(StorageFailure::RandomUnavailable));
                }
                (
                    Version {
                        epoch,
                        born,
                        revision: 1,
                    },
                    true,
                )
            }
        };
        if establish {
            storage::set_version(&mut record, &version);
            storage::update_existing_for(&tx, key, provider.id(), &record)?;
            let written = storage::load_for(&tx, key, provider.id(), id)?;
            if written.fields() != record.fields() {
                return Err(Error::Storage(StorageFailure::Write));
            }
        }
        let stamp = RowStamp {
            provider,
            id: id.to_string(),
            version,
            projection,
            account_maintenance,
        };
        stamp.compare(&record, &stamp.version.epoch)?;
        Ok(stamp)
    })();
    super::finish(tx, result)
}
