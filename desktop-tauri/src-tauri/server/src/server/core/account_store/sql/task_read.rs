//! 最终事务内唯一单行读取；不走decode_data、日志、密钥加载或公开账号/代理投影。

mod json;

use rusqlite::{types::ValueRef, Connection, ErrorCode, Row};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::crypto::{self, SecretScope};

const MAX_DATA_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum TaskReadError {
    Busy,
    Storage,
    Missing,
    Invalid,
    TooLarge,
    KeyUnavailable,
}

impl TaskReadError {
    fn sql(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => Self::Busy,
            _ => Self::Storage,
        }
    }
}

pub(crate) fn load_task_final(conn: &Connection, id: &str) -> Result<StoredAccount, TaskReadError> {
    if id.trim().is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
        return Err(TaskReadError::Invalid);
    }
    // octet_length读取列的字节数；CASE在超限时不取正文。Rust仍在借用字节上二次限长，
    // 不先构造String或解密；不枚举账号、不从cache/revision猜内容。
    let mut stmt = conn.prepare(
        "SELECT CASE WHEN typeof(data) = 'text' AND octet_length(data) <= ?2 THEN data END,
                octet_length(data), typeof(data), id,
                CASE WHEN typeof(provider) = 'text' AND octet_length(provider) <= 128 THEN provider END,
                enabled, priority, added_at
         FROM main.accounts WHERE id = ?1 LIMIT 1",
    ).map_err(TaskReadError::sql)?;
    let mut rows = stmt.query(rusqlite::params![id, MAX_DATA_BYTES as i64]).map_err(TaskReadError::sql)?;
    let row = rows.next().map_err(TaskReadError::sql)?.ok_or(TaskReadError::Missing)?;
    if text(row, 2, 16)? != "text" {
        return Err(TaskReadError::Invalid);
    }
    let bytes: i64 = row.get(1).map_err(TaskReadError::sql)?;
    if bytes <= 0 { return Err(TaskReadError::Invalid); }
    if bytes > MAX_DATA_BYTES as i64 { return Err(TaskReadError::TooLarge); }
    let raw = text(row, 0, MAX_DATA_BYTES)?;
    let column_id = text(row, 3, 512)?;
    let column_provider = text(row, 4, 128)?;
    let column_enabled: i64 = row.get(5).map_err(TaskReadError::sql)?;
    if column_id != id || column_provider != "monkeycode" || column_enabled != 1 {
        return Err(TaskReadError::Invalid);
    }
    let record = decode(raw)?;
    // 先读权威字段，再核对SQL派生列；禁用、删除、错误类型/归属都不能成为消费材料。
    if record.id() != column_id
        || record.get("provider").and_then(Value::as_str).map(str::trim) != Some(column_provider)
        || record.provider() != column_provider
        || !record.enabled()
        || record.get("enabled").is_some_and(|value| !value.is_boolean())
        || record.priority() != row.get::<_, i64>(6).map_err(TaskReadError::sql)?
        || record.added_at() != row.get::<_, i64>(7).map_err(TaskReadError::sql)?
    {
        return Err(TaskReadError::Invalid);
    }
    // 将语句推进到完成并显式收尾，读取/收尾错误仍在消费回调之前拒绝。
    if rows.next().map_err(TaskReadError::sql)?.is_some() { return Err(TaskReadError::Invalid); }
    drop(rows);
    stmt.finalize().map_err(TaskReadError::sql)?;
    // row/statement已释放，但调用方仍持同一账号锁、Db锁和IMMEDIATE事务。
    Ok(record)
}

fn text<'a>(row: &'a Row<'_>, column: usize, limit: usize) -> Result<&'a str, TaskReadError> {
    let ValueRef::Text(bytes) = row.get_ref(column).map_err(TaskReadError::sql)? else {
        return Err(TaskReadError::Invalid);
    };
    if bytes.len() > limit { return Err(TaskReadError::TooLarge); }
    std::str::from_utf8(bytes).map_err(|_| TaskReadError::Invalid)
}

fn decode(raw: &str) -> Result<StoredAccount, TaskReadError> {
    let plaintext;
    let bytes = if crypto::is_envelope(raw) {
        // 缓存空/忙/poison全部拒绝；不使用existing_key/open/secret_key的回落路径。
        let key = Zeroizing::new(crypto::try_cached_key(SecretScope::Accounts).ok_or(TaskReadError::KeyUnavailable)?);
        plaintext = Zeroizing::new(crypto::open_with_key(raw, &key).map_err(|_| TaskReadError::Invalid)?);
        plaintext.as_slice()
    } else {
        raw.as_bytes()
    };
    let Value::Object(mut fields) = json::parse(bytes)? else {
        return Err(TaskReadError::Invalid);
    };
    // StoredAccount兼容字符串化插件对象，但其普通回落会吞重复键。这里先严格解析，
    // 再交给原形状访问器，既保留旧对象规则，也不让宽松反序列化进入最终复核。
    for name in ["pluginSecret", "pluginPublic"] {
        if let Some(Value::String(text)) = fields.get(name) {
            let nested = json::parse(text.as_bytes())?;
            if !nested.is_object() { return Err(TaskReadError::Invalid); }
            fields.insert(name.to_string(), nested);
        }
    }
    StoredAccount::from_value(Value::Object(fields)).ok_or(TaskReadError::Invalid)
}
