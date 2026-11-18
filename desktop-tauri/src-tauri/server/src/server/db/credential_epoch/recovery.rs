//! 已知应用内恢复的持久隔离。只修改固定宿主epoch，不读取/改写账号、槽或任何凭据。
//! 原连接先提交撤销，再创建安全备份；暂存恢复库另提交新epoch后才允许覆盖原库。
//! 不覆盖绕过应用的整库拷回、磁盘/设备快照回滚或外部旧进程；FULL也不是exactly-once。

use std::sync::atomic::AtomicBool;

use rusqlite::{types::ValueRef, Connection};

use crate::server::db::codearts_slot::{
    full_transaction, CodeArtsStorageError, CodeArtsTransaction,
};

// 与account_store的宿主出生版本同一落盘键；已登记RESERVED_KV_KEYS且不参与云同步。
// 私有代次由本模块真实随机生成，绝不从备份JSON/快照输入构造或把建键当首次纳管。
const EPOCH_KEY: &str = "accountCredentialEpoch";

#[derive(Clone, Copy)]
enum EpochFailure {
    Schema,
    Read,
    Random,
    Write,
}

#[derive(Clone, Copy)]
pub(crate) enum IsolationFailure {
    NotEntered {
        reason: CodeArtsStorageError,
        storage_closed: bool,
    },
    Rejected {
        storage_closed: bool,
    },
    CommitUnconfirmed {
        storage_closed: bool,
    },
    CommittedButUnusable,
}

impl IsolationFailure {
    fn message(self) -> &'static str {
        match self {
            Self::CommitUnconfirmed { storage_closed: true } =>
                "替换前的发送资格隔离提交未确认，未继续执行业务替换；不能据此认定隔离没有写入",
            Self::CommitUnconfirmed { storage_closed: false } =>
                "替换前的发送资格隔离提交和连接收尾均未确认，未继续执行业务替换；不能重放或恢复旧资格",
            Self::CommittedButUnusable =>
                "旧发送资格隔离已提交，但连接设置恢复未确认，未继续执行业务替换；不得恢复旧资格",
            Self::NotEntered { reason: CodeArtsStorageError::Busy, storage_closed: true } =>
                "数据库忙，未完成替换前的发送资格持久隔离，未继续执行业务替换",
            Self::NotEntered { storage_closed: false, .. } | Self::Rejected { storage_closed: false } =>
                "替换前的发送资格隔离未完成且连接收尾未确认，未继续执行业务替换",
            Self::NotEntered { .. } | Self::Rejected { storage_closed: true } =>
                "无法确认替换前的发送资格持久隔离，未继续执行业务替换",
        }
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for String {}
    impl Sealed for rusqlite::Error {}
}

/// 固定现有三条替换链的错误承载，不能另实现一个吞掉隔离错误的无Result旁路。
pub(crate) trait AccountChangeError: sealed::Sealed {
    fn isolation(error: IsolationFailure) -> Self;
}

impl AccountChangeError for String {
    fn isolation(error: IsolationFailure) -> Self {
        error.message().to_string()
    }
}

impl AccountChangeError for rusqlite::Error {
    fn isolation(error: IsolationFailure) -> Self {
        Self::ToSqlConversionFailure(error.message().to_string().into())
    }
}

/// 只在原Db连接锁内调用；无CodeArts槽/无旧epoch均可正常建立隔离，不创建Ready。
pub(super) fn advance(conn: &Connection, unusable: &AtomicBool) -> Result<(), IsolationFailure> {
    confirmed(full_transaction(conn, unusable, |conn| rotate(conn, None)))
}

/// 原库已经耐久撤销，暂存库再用不同epoch隔离。失败只丢弃暂存结果，不回退原库epoch。
pub(super) fn isolate_staged(staged: &Connection, main: &Connection) -> Result<(), String> {
    let original = epoch(main)
        .map_err(|_| "无法核对原库已提交的隔离状态，未覆盖主库".to_string())?
        .ok_or_else(|| "原库隔离状态缺失，未覆盖主库".to_string())?;
    // 此连接属于现有恢复临时库，不增加文件访问能力；失败后它直接由原恢复守卫丢弃。
    let unusable = AtomicBool::new(false);
    confirmed(full_transaction(staged, &unusable, |conn| {
        rotate(conn, Some(&original))
    }))
    .map_err(|failure| {
        format!(
            "暂存恢复库隔离失败：{}；原库旧发送资格已先完成隔离提交",
            failure.message()
        )
    })
}

fn confirmed(outcome: CodeArtsTransaction<(), EpochFailure>) -> Result<(), IsolationFailure> {
    match outcome {
        CodeArtsTransaction::Committed {
            storage_closed: true,
            ..
        } => Ok(()),
        CodeArtsTransaction::Committed {
            storage_closed: false,
            ..
        } => Err(IsolationFailure::CommittedButUnusable),
        CodeArtsTransaction::NotEntered {
            error,
            storage_closed,
        } => Err(IsolationFailure::NotEntered {
            reason: error,
            storage_closed,
        }),
        CodeArtsTransaction::Rejected { storage_closed, .. } => {
            Err(IsolationFailure::Rejected { storage_closed })
        }
        CodeArtsTransaction::CommitUnconfirmed { storage_closed } => {
            Err(IsolationFailure::CommitUnconfirmed { storage_closed })
        }
    }
}

fn rotate(conn: &Connection, other: Option<&str>) -> Result<(), EpochFailure> {
    if conn.is_autocommit() {
        return Err(EpochFailure::Write);
    }
    schema(conn)?;
    let before = epoch(conn)?;
    let fresh = random_epoch()?;
    // 不用计数器/时钟/账号ID，碰撞或熵源异常不重试、不沿用前值。
    if before.as_deref() == Some(fresh.as_str()) || other == Some(fresh.as_str()) {
        return Err(EpochFailure::Random);
    }
    let changed = conn
        .execute(
            "INSERT INTO main.kv(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![EPOCH_KEY, fresh],
        )
        .map_err(|_| EpochFailure::Write)?;
    if changed != 1 || epoch(conn)?.as_deref() != Some(fresh.as_str()) {
        return Err(EpochFailure::Write);
    }
    Ok(())
}

/// 只借出本机固定kv项；坏外来epoch不是来源，覆盖它不会恢复任何现有槽的资格。
fn epoch(conn: &Connection) -> Result<Option<String>, EpochFailure> {
    let mut statement = conn
        .prepare(
            "SELECT CASE WHEN typeof(value)='text' AND octet_length(value)=64 THEN value END
         FROM main.kv WHERE key=?1 LIMIT 2",
        )
        .map_err(|_| EpochFailure::Read)?;
    let mut rows = statement
        .query([EPOCH_KEY])
        .map_err(|_| EpochFailure::Read)?;
    let value = match rows.next().map_err(|_| EpochFailure::Read)? {
        Some(row) => match row.get_ref(0).map_err(|_| EpochFailure::Read)? {
            ValueRef::Text(bytes) => std::str::from_utf8(bytes)
                .ok()
                .filter(|value| valid_epoch(value))
                .map(str::to_string),
            ValueRef::Null => None,
            _ => return Err(EpochFailure::Read),
        },
        None => None,
    };
    if rows.next().map_err(|_| EpochFailure::Read)?.is_some() {
        return Err(EpochFailure::Read);
    }
    drop(rows);
    statement.finalize().map_err(|_| EpochFailure::Read)?;
    Ok(value)
}

fn random_epoch() -> Result<String, EpochFailure> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| EpochFailure::Random)?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(EpochFailure::Random);
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn valid_epoch(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && value.bytes().any(|byte| byte != b'0')
}

fn schema(conn: &Connection) -> Result<(), EpochFailure> {
    // 不执行备份中能把新epoch改回旧值的触发器，也不删除用户触发器或业务数据。
    let safe: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='kv' AND type='table')
          AND NOT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND tbl_name='kv')
          AND NOT EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE type='trigger' AND tbl_name='kv')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| EpochFailure::Schema)?;
    if safe != 1 {
        return Err(EpochFailure::Schema);
    }
    Ok(())
}
