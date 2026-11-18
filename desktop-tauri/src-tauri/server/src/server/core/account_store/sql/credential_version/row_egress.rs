//! 只读原豆包账号的代理配置；不按账号字符串换来源，不在存储锁内解析代理或网络。

use rusqlite::{Connection, TransactionBehavior};
use serde_json::Value;

use super::{bounded, row_source::RowStamp, storage, Error, StorageFailure};
use crate::server::core::account_store::credential_version::RowProvider;

pub(in crate::server::core::account_store) fn doubao_proxy(
    conn: &mut Connection,
    key: &[u8; 32],
    stamp: &RowStamp,
) -> Result<Value, Error> {
    if stamp.provider() != RowProvider::DoubaoWork {
        return Err(Error::UnsupportedProvider);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage::sql_error(error, StorageFailure::Begin))?;
    let result = (|| {
        stamp.verify_current(&tx, key)?;
        let record = storage::load_for(&tx, key, stamp.provider().id(), stamp.account())?;
        let proxy = record.get("proxy")
            .or_else(|| record.get("pluginSecret").and_then(|secret| secret.get("proxy")))
            .cloned().unwrap_or(Value::Null);
        bounded::check(&proxy, 64 * 1024)?;
        Ok(proxy)
    })();
    tx.rollback().map_err(|_| Error::Storage(StorageFailure::RollbackUnconfirmed))?;
    result
}
