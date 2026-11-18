//! 只查询有界内存快照，不把真实路径交给 SQLite，不创建临时文件或恢复 WAL/journal。
//! 锁定 libsqlite3-sys 0.38.2 bundled 源码/绑定含 sqlite3_deserialize，build.rs 未定义
//! SQLITE_OMIT_DESERIALIZE；使用已存在的 ffi，不新增 Cargo 特性或猜测动态符号。
//! https://sqlite.org/c3ref/deserialize.html
//! WAL 标头不作修补：读写版本非 1 的原始文件直接拒绝。

use std::time::{Duration, Instant};

use rusqlite::{config::DbConfig, ffi, hooks::{AuthAction, AuthContext, Authorization}, types::ValueRef, Connection};
use zeroize::Zeroizing;

use super::{validate_token, ReadError, COOKIE_ACCESS, COOKIE_HOST, COOKIE_REFRESH, MAX_TOKEN};

const MAX_ROW: usize = 128 * 1024;
const MAX_ENCRYPTED: usize = MAX_TOKEN + 3 + 12 + 16 + 32;
const MAX_ROWS: usize = 2;
const MAX_VM_STEPS: usize = 200_000;

pub(super) struct Cookie {
    pub name: &'static str,
    pub value: Zeroizing<Vec<u8>>,
    pub encrypted: Zeroizing<Vec<u8>>,
}

pub(super) struct Cookies {
    pub version: u32,
    pub rows: Vec<Cookie>,
}

/// Cookie 数据缓冲的地址及长度在整个同步查询期间不变；连接必在借用结束前关闭。
pub(super) fn read(bytes: &mut [u8]) -> Result<Cookies, ReadError> {
    header(bytes)?;
    let connection = Connection::open_in_memory().map_err(database_error)?;
    secure_connection(&connection)?;
    let length = i64::try_from(bytes.len()).map_err(|_| ReadError::TooLarge)?;
    // SAFETY: bytes 来自有界 Zeroizing<Vec<u8>>，被唯一可变借用，生命周期长于 connection；
    // 不设置 FREEONCLOSE/RESIZEABLE，SQLite 不释放、不扩容、不写它。没有活动语句。
    let result = unsafe {
        ffi::sqlite3_deserialize(connection.handle(), c"main".as_ptr(), bytes.as_mut_ptr(), length, length,
            ffi::SQLITE_DESERIALIZE_READONLY)
    };
    if result != ffi::SQLITE_OK { return Err(code_error(result)); }
    // sqlite3_deserialize 内部会编译固定 ATTACH 来替换 main 的内存 VFS（锁定源码已核对）；
    // 必须在它完成后才安装拒绝 ATTACH 的 SQL 门禁，且早于任何不可信 schema 查询。
    // SAFETY: 固定限制作用于当前独占连接，不修改全局或文件。
    unsafe { ffi::sqlite3_limit(connection.handle(), ffi::SQLITE_LIMIT_ATTACHED, 0) };
    connection.authorizer(Some(authorize)).map_err(database_error)?;
    let result = query(&connection);
    // 明确关闭；查询错误也不得绕过连接收尾。没有向外返回 stmt/row/connection。
    let closed = connection.close();
    if closed.is_err() { return Err(ReadError::ReadFailed); }
    result
}

fn header(bytes: &[u8]) -> Result<(), ReadError> {
    if bytes.len() < 100 || bytes.get(..16) != Some(b"SQLite format 3\0".as_slice()) {
        return Err(ReadError::Malformed);
    }
    if bytes[18] != 1 || bytes[19] != 1 { return Err(ReadError::UnsupportedLayout); }
    let page = u16::from_be_bytes([bytes[16], bytes[17]]);
    let page = if page == 1 { 65536usize } else { usize::from(page) };
    if !(512..=65536).contains(&page) || !page.is_power_of_two() || bytes.len() % page != 0 {
        return Err(ReadError::Malformed);
    }
    // 不接受旧计数、截断或额外尾部，不能根据无 sidecar 就假设完整。
    let pages = u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]) as usize;
    if pages.checked_mul(page) != Some(bytes.len()) || bytes[24..28] != bytes[92..96]
        || bytes[20] != 0 || bytes[21..24] != [64, 32, 32]
        || bytes[56..60] != [0, 0, 0, 1]
    { return Err(ReadError::UnsupportedLayout); }
    Ok(())
}

fn secure_connection(connection: &Connection) -> Result<(), ReadError> {
    connection.busy_timeout(Duration::ZERO).map_err(database_error)?;
    for (setting, value) in [
        (DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true),
        (DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY, false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_FTS3_TOKENIZER, false),
        (DbConfig::SQLITE_DBCONFIG_WRITABLE_SCHEMA, false),
        (DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true),
    ] {
        if connection.set_db_config(setting, value).map_err(database_error)? != value {
            return Err(ReadError::UnsupportedLayout);
        }
    }
    let mut enabled = -1i32;
    // SAFETY: 独占同步连接；DBCONFIG_ENABLE_LOAD_EXTENSION 的文档签名为 (int, int*)。
    let code = unsafe { ffi::sqlite3_db_config(connection.handle(), ffi::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION,
        0i32, &mut enabled as *mut i32) };
    if code != ffi::SQLITE_OK || enabled != 0 { return Err(ReadError::UnsupportedLayout); }
    for (kind, maximum) in [
        (ffi::SQLITE_LIMIT_LENGTH, MAX_ROW as i32),
        (ffi::SQLITE_LIMIT_SQL_LENGTH, 16 * 1024),
        (ffi::SQLITE_LIMIT_COLUMN, 64),
        (ffi::SQLITE_LIMIT_EXPR_DEPTH, 32),
        (ffi::SQLITE_LIMIT_COMPOUND_SELECT, 2),
        (ffi::SQLITE_LIMIT_VDBE_OP, 20_000),
        (ffi::SQLITE_LIMIT_VARIABLE_NUMBER, 8),
        (ffi::SQLITE_LIMIT_TRIGGER_DEPTH, 0),
        (ffi::SQLITE_LIMIT_WORKER_THREADS, 0),
    ] {
        // SAFETY: 固定 limit ID 和非负上限，handle 在当前线程独占；不触碰全局 SQLite 配置。
        unsafe { ffi::sqlite3_limit(connection.handle(), kind, maximum) };
    }
    // PRAGMA 只改变这份空内存连接的行为，先于不可信 schema；不设置 journal_mode。
    connection.execute_batch("PRAGMA temp_store=MEMORY; PRAGMA query_only=ON; PRAGMA cache_size=-1024;")
        .map_err(database_error)?;
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut instructions = 0usize;
    connection.progress_handler(1000, Some(move || {
        instructions = instructions.saturating_add(1000);
        instructions >= MAX_VM_STEPS || Instant::now() >= deadline
    })).map_err(database_error)?;
    Ok(())
}

fn authorize(context: AuthContext<'_>) -> Authorization {
    if context.accessor.is_some() || context.database_name.is_some_and(|database| database != "main") {
        return Authorization::Deny;
    }
    let allowed = match context.action {
        AuthAction::Select => true,
        AuthAction::Read { table_name: "sqlite_master" | "sqlite_schema", column_name } =>
            matches!(column_name, "name" | "type" | "sql" | "rootpage" | "tbl_name" | ""),
        AuthAction::Read { table_name: "meta", column_name } => matches!(column_name, "key" | "value"),
        AuthAction::Read { table_name: "cookies", column_name } =>
            matches!(column_name, "host_key" | "name" | "value" | "encrypted_value"),
        AuthAction::Pragma { pragma_name: "table_xinfo", pragma_value: Some("cookies" | "meta") } => true,
        _ => false, // 仅允许两张固定表的只读结构查询；其余 PRAGMA、扩展、DDL/DML 等全拒绝。
    };
    if allowed { Authorization::Allow } else { Authorization::Deny }
}

fn query(connection: &Connection) -> Result<Cookies, ReadError> {
    verify_tables(connection)?;
    // 禁止 generated/hidden 列把表达式计算结果伪装成官方持久 Cookie。
    verify_columns(connection, "PRAGMA main.table_xinfo('cookies')", &["host_key", "name", "value", "encrypted_value"])?;
    verify_columns(connection, "PRAGMA main.table_xinfo('meta')", &["key", "value"])?;
    let version = version(connection)?;
    let mut statement = connection.prepare(
        "SELECT name, value, encrypted_value FROM main.cookies NOT INDEXED \
         WHERE host_key COLLATE BINARY = ?1 AND name COLLATE BINARY IN (?2, ?3) LIMIT 3"
    ).map_err(database_error)?;
    let mut records = statement.query([COOKIE_HOST, COOKIE_ACCESS, COOKIE_REFRESH]).map_err(database_error)?;
    let mut rows: Vec<Cookie> = Vec::with_capacity(MAX_ROWS);
    while let Some(row) = records.next().map_err(database_error)? {
        if rows.len() == MAX_ROWS { return Err(ReadError::Conflict); }
        let name = match row.get_ref(0).map_err(database_error)? {
            ValueRef::Text(value) if value == COOKIE_ACCESS.as_bytes() => COOKIE_ACCESS,
            ValueRef::Text(value) if value == COOKIE_REFRESH.as_bytes() => COOKIE_REFRESH,
            _ => return Err(ReadError::Malformed),
        };
        if rows.iter().any(|previous| previous.name == name) { return Err(ReadError::Conflict); }
        let plain = match row.get_ref(1).map_err(database_error)? {
            ValueRef::Text(value) => value,
            _ => return Err(ReadError::Malformed),
        };
        let encrypted = match row.get_ref(2).map_err(database_error)? {
            ValueRef::Blob(value) => value,
            _ => return Err(ReadError::Malformed),
        };
        if plain.len() > MAX_TOKEN || encrypted.len() > MAX_ENCRYPTED { return Err(ReadError::TooLarge); }
        if plain.is_empty() && encrypted.is_empty() { return Err(ReadError::Malformed); }
        if !plain.is_empty() && !encrypted.is_empty() { return Err(ReadError::Conflict); }
        if !plain.is_empty() {
            validate_token(std::str::from_utf8(plain).map_err(|_| ReadError::Malformed)?)?;
        } else if encrypted.len() <= 3 + 12 + 16 || encrypted.get(..3) != Some(b"v10".as_slice()) {
            return Err(ReadError::UnsupportedLayout);
        }
        rows.push(Cookie { name, value: Zeroizing::new(plain.to_vec()), encrypted: Zeroizing::new(encrypted.to_vec()) });
    }
    if rows.is_empty() { return Err(ReadError::NoCredentials); }
    Ok(Cookies { version, rows })
}

fn verify_tables(connection: &Connection) -> Result<(), ReadError> {
    let mut statement = connection.prepare(
        "SELECT name, type, sql FROM main.sqlite_schema WHERE name IN ('cookies', 'meta') LIMIT 3"
    ).map_err(database_error)?;
    let mut rows = statement.query([]).map_err(database_error)?;
    let (mut cookies, mut meta) = (false, false);
    while let Some(row) = rows.next().map_err(database_error)? {
        let name = row.get_ref(0).map_err(database_error)?.as_str().map_err(|_| ReadError::Malformed)?;
        let kind = row.get_ref(1).map_err(database_error)?.as_str().map_err(|_| ReadError::Malformed)?;
        let sql = row.get_ref(2).map_err(database_error)?.as_str().map_err(|_| ReadError::Malformed)?;
        if kind != "table" || sql.len() > 16 * 1024 { return Err(ReadError::UnsupportedLayout); }
        let mut parts = sql.split_ascii_whitespace();
        if !parts.next().is_some_and(|part| part.eq_ignore_ascii_case("create"))
            || !parts.next().is_some_and(|part| part.eq_ignore_ascii_case("table"))
        { return Err(ReadError::UnsupportedLayout); }
        match name {
            "cookies" if !cookies => cookies = true,
            "meta" if !meta => meta = true,
            _ => return Err(ReadError::Conflict),
        }
    }
    if !cookies || !meta { return Err(ReadError::UnsupportedLayout); }
    Ok(())
}

fn verify_columns(connection: &Connection, sql: &'static str, required: &[&str]) -> Result<(), ReadError> {
    let mut statement = connection.prepare(sql).map_err(database_error)?;
    let mut rows = statement.query([]).map_err(database_error)?;
    let mut present = vec![false; required.len()];
    let mut count = 0usize;
    while let Some(row) = rows.next().map_err(database_error)? {
        count += 1;
        if count > 64 { return Err(ReadError::TooLarge); }
        let name = row.get_ref(1).map_err(database_error)?.as_str().map_err(|_| ReadError::Malformed)?;
        let hidden = row.get_ref(6).map_err(database_error)?.as_i64().map_err(|_| ReadError::Malformed)?;
        if hidden != 0 { return Err(ReadError::UnsupportedLayout); }
        if let Some(index) = required.iter().position(|expected| *expected == name) {
            if present[index] { return Err(ReadError::Conflict); }
            present[index] = true;
        }
    }
    if present.iter().any(|present| !present) { return Err(ReadError::UnsupportedLayout); }
    Ok(())
}

fn version(connection: &Connection) -> Result<u32, ReadError> {
    let mut statement = connection.prepare(
        "SELECT value FROM main.meta NOT INDEXED WHERE key COLLATE BINARY='version' LIMIT 2"
    ).map_err(database_error)?;
    let mut rows = statement.query([]).map_err(database_error)?;
    let row = rows.next().map_err(database_error)?.ok_or(ReadError::UnsupportedLayout)?;
    let version = match row.get_ref(0).map_err(database_error)? {
        ValueRef::Integer(value) => u32::try_from(value).map_err(|_| ReadError::UnsupportedLayout)?,
        ValueRef::Text(value) if value.len() <= 3 => std::str::from_utf8(value).ok()
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|value| value.parse::<u32>().ok()).ok_or(ReadError::UnsupportedLayout)?,
        _ => return Err(ReadError::UnsupportedLayout),
    };
    if rows.next().map_err(database_error)?.is_some() { return Err(ReadError::Conflict); }
    // 只承接已公开的 v23/v24 格式，不把未来 schema 当成同一算法猜读。
    if !matches!(version, 23 | 24) { return Err(ReadError::UnsupportedLayout); }
    Ok(version)
}

fn database_error(error: rusqlite::Error) -> ReadError {
    match error {
        rusqlite::Error::SqliteFailure(error, _) => code_error(error.extended_code),
        _ => ReadError::Malformed,
    }
}
fn code_error(code: i32) -> ReadError {
    match code & 0xff {
        ffi::SQLITE_BUSY | ffi::SQLITE_LOCKED => ReadError::Busy,
        ffi::SQLITE_TOOBIG | ffi::SQLITE_NOMEM | ffi::SQLITE_INTERRUPT => ReadError::TooLarge,
        ffi::SQLITE_AUTH | ffi::SQLITE_CANTOPEN | ffi::SQLITE_READONLY => ReadError::UnsupportedLayout,
        _ => ReadError::Malformed,
    }
}
