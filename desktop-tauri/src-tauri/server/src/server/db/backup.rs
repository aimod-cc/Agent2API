//! 整库备份 / 恢复：数据库与必要密钥装进密码加密包。
//!
//! `archive` 负责版本化容器、Argon2id 与分段认证加密；`keys` 校验并将
//! 备份凭证重新加密到本机密钥；`restore` 负责暂存校验、覆盖与失败回滚。
//! 旧 SQL 仍可导入，但它不包含密钥，指纹不匹配时保留明确警告。
//!
//! 导出先用 SQLite Backup 取得一致性内存快照，再按批写入 SQL，避免
//! 长时间占用主连接锁。导入 SQL 的 authorizer 拒绝 ATTACH、文件后端
//! 虚拟表和未知动作，所有校验都在私有暂存库完成，不能写到主库之外。
//! 替换前用 Backup API 保留原数据库；目标机器密钥不会被覆盖。
//!
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use chrono::Local;
use rusqlite::backup::{Backup, StepResult};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::Connection;
use serde_json::{json, Value};

use super::Db;
use crate::server::core::crypto;
use crate::server::logging;

mod archive;
mod keys;
pub(crate) mod retention;
mod restore;
mod safety;
mod sql_import;
mod stream;
mod temp;

pub use archive::{BACKUP_EXTENSION, MAX_BACKUP_BYTES};
pub use temp::transfer_file;
use temp::TempRestoreDb;

/// 导出文件的第一行。导入端只认带这个前缀的文件 —— 用户的其它 SQL
/// （手写、别处导出）即便语法正确也拒绝，避免「恢复成了半份陌生数据」。
pub const BACKUP_SQL_HEADER: &str = "-- Agent2API SQLite 导出";

/// 文件头里记录「导出时账号加密密钥指纹」的注释行前缀（值在冒号之后）。
/// 放在 `BACKUP_SQL_HEADER` 之后，是**注释行**，不影响「以头部前缀开头」的判定。
const KEY_FINGERPRINT_LINE: &str = "-- account-key-fingerprint:";

/// 导出时账号密钥尚不存在时写入的明确标记（对应 `crypto::key_fingerprint` 返回
/// `None` 的情形）。写一个可读的词而不是留空：留空会让「没这一行」与「有但为空」
/// 两种含义混在一起，导入侧无法区分「旧备份没记」和「记了但没有密钥」。
const KEY_FINGERPRINT_ABSENT: &str = "(未生成)";

/// 解析指纹时只扫文件开头这几行 —— 头部是固定几行注释，为一行标记去遍历整份
/// 上百 MB 的正文毫无必要。留足余量以容忍将来在前面再加一两行头部注释。
const HEADER_SCAN_LINES: usize = 16;

/// 账号域密钥文件名，只用于给用户的警告文案（「把这个文件一起带过来」）。
/// 事实来源与 `crypto::SecretScope::key_file` 保持一致。
const ACCOUNT_KEY_FILE: &str = "account.key";

/// 精简导出（不带日志）跳过的表：事件日志、请求明细/聚合、调试报文与请求
/// 原始报文。业务数据（kv、accounts、account_usage_records）无论哪种口径
/// 都导出 —— 账号与配置是「换机器」真正要带的东西。
const LOG_TABLES: &[&str] = &[
    "logs",
    "requests",
    "request_daily",
    "debug_traffic",
    "request_raw",
];

/// 暂存库执行完外部 SQL 后必须存在的表。只卡业务底线两张：精简备份合法地
/// 没有日志类表（导入时由 `schema::migrate` 补建），而这两张若缺说明文件
/// 被截断或根本不是本程序的库。
const REQUIRED_TABLES: &[&str] = &["kv", "accounts"];

/// 恢复执行中的互斥（含暂存库校验到覆盖主库全程）。两个恢复并发没有意义
/// （后到的把先到的覆盖掉，用户看到的统计却各说各话），也不必细化到读写锁。
static RESTORE_MUTEX: Mutex<()> = Mutex::new(());

// ─── 进度（恢复 / 导出，供前端轮询）──────────────────────────
//
// ── 为什么要有这个 ──────────────────────────────────────────
// 整库恢复是**一次同步的长调用**：读几百 MB 的 SQL、在暂存库上执行、校验
// schema、拍安全备份、用 Backup API 覆盖主库。整个过程在 30 秒到几分钟之间，
// 而接口在结束时才返回 —— 界面上只有一个「恢复中…」的灰按钮，用户既看不到
// 进展，也不知道还要等多久。
//
// ── 为什么是「全局快照 + 轮询」而不是事件推送 ────────────────
// 与 `/api/update/progress`、`/api/storage/progress` 同一套：进度写入进程内的
// 一个静态快照，前端每 500ms 拉一次。理由是不必为一条低频进度引入事件通道
// （桌面壳与网页端两种形态都要接），而且刷新页面后仍能接上同一次恢复 ——
// 弹窗按快照重建即可，事件流做不到（断了就丢）。
//
// ── 为什么没有「取消」────────────────────────────────────────
// 覆盖主库与重载状态不能从界面中断；错误还原由恢复流程统一处理。

/// 恢复进度的一个阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestorePhase {
    /// 空闲（没有恢复在跑）
    Idle,
    /// 读备份文件 + 头部校验
    Reading,
    Decrypting,
    Reencrypting,
    RollingBack,
    /// 在暂存库上执行备份 SQL
    Applying,
    /// 校验 schema 版本 / 必需表 / 对齐
    Validating,
    /// 给当前主库拍安全备份
    SafetyBackup,
    /// 用 Backup API 覆盖主库
    Replacing,
    /// 重装内存态（config / 令牌）
    Reloading,
    /// 完成
    Done,
    /// 失败
    Failed,
}

impl RestorePhase {
    /// 阶段 → 界面文案。
    ///
    /// 文案写在后端：它是「这一步在做什么」的唯一权威（步骤顺序由这里决定），
    /// 前端只负责显示。两处各写一份必然漂 —— 早先的「恢复中…」就是因为
    /// 只有一句话，才说不出任何进展。
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Reading => "正在读取备份文件…",
            Self::Decrypting => "正在解密并校验备份包…",
            Self::Reencrypting => "正在迁移加密凭证到本机密钥…",
            Self::RollingBack => "正在还原替换前的数据…",
            Self::Applying => "正在把数据写入暂存库…",
            Self::Validating => "正在校验备份完整性…",
            Self::SafetyBackup => "正在备份当前数据（可回滚）…",
            Self::Replacing => "正在替换主数据库…",
            Self::Reloading => "正在重新加载配置…",
            Self::Done => "恢复完成",
            Self::Failed => "恢复失败",
        }
    }

    /// 这个阶段占整条进度的百分比（起点）。
    ///
    /// 各阶段的耗时比例差得很远（读文件几秒、执行 SQL 可能几十秒、覆盖主库
    /// 与库大小成正比），但**没有可靠的细粒度计量**（`execute_batch` 不回调）。
    /// 所以给的是「大致位置」而不是精确百分比 —— 一个稳定的、只增不减的
    /// 进度条比一个乱跳的假进度好。数值在 `DONE` 处收口到 100。
    fn percent(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Reading => 5,
            Self::Decrypting => 8,
            Self::Reencrypting => 48,
            Self::RollingBack => 90,
            Self::Applying => 15,
            Self::Validating => 45,
            Self::SafetyBackup => 55,
            Self::Replacing => 65,
            Self::Reloading => 92,
            Self::Done => 100,
            Self::Failed => 100,
        }
    }
}

/// 恢复进度快照（前端轮询读它）。
#[derive(Clone, Debug)]
struct RestoreProgress {
    phase: RestorePhase,
    /// 失败时的原因（`Failed` 阶段才有）
    error: Option<String>,
    /// 完成时的结果（`Done` 阶段才有；形状与 `import_from_file` 的返回值一致）
    result: Option<Value>,
    /// 开始时刻（毫秒）：界面据此显示「已用时」
    started_at: i64,
    /// 结束时刻（毫秒；进行中为 0）
    finished_at: i64,
}

impl RestoreProgress {
    fn idle() -> Self {
        Self {
            phase: RestorePhase::Idle,
            error: None,
            result: None,
            started_at: 0,
            finished_at: 0,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "active": matches!(
                self.phase,
                RestorePhase::Reading
                    | RestorePhase::Decrypting
                    | RestorePhase::Reencrypting
                    | RestorePhase::RollingBack
                    | RestorePhase::Applying
                    | RestorePhase::Validating
                    | RestorePhase::SafetyBackup
                    | RestorePhase::Replacing
                    | RestorePhase::Reloading
            ),
            "phase": self.phase.label(),
            "percent": self.phase.percent(),
            "done": self.phase == RestorePhase::Done,
            "failed": self.phase == RestorePhase::Failed,
            "error": self.error,
            "result": self.result,
            "startedAt": self.started_at,
            "finishedAt": self.finished_at,
        })
    }
}

/// 全局进度槽。`None` = 从没跑过恢复（界面据此不显示任何东西）。
static RESTORE_PROGRESS: Mutex<Option<RestoreProgress>> = Mutex::new(None);

/// 写一个阶段（内部用；锁失败时静默 —— 进度是锦上添花，不该让恢复本身失败）。
fn set_phase(phase: RestorePhase) {
    if let Ok(mut guard) = RESTORE_PROGRESS.lock() {
        if let Some(progress) = guard.as_mut() {
            progress.phase = phase;
        }
    }
}

/// 读当前进度（前端轮询）。从没跑过 → `{"active": false, "phase": ""}`。
pub fn restore_progress() -> Value {
    match RESTORE_PROGRESS.lock() {
        Ok(guard) => guard
            .as_ref()
            .map(RestoreProgress::to_json)
            .unwrap_or_else(|| RestoreProgress::idle().to_json()),
        Err(_) => RestoreProgress::idle().to_json(),
    }
}

/// 开始一次恢复：重置进度槽。`finish` 负责收口（成功 / 失败）。
///
/// ── 为什么没有 Drop 兜底 ────────────────────────────────────
/// `import_from_file` 里 `finish` 是**唯一出口**（`let result = …` 先接住结果，
/// 无论 Ok / Err 都会走到它），所以不存在「没收口就返回」的路径。而 release 是
/// `panic=abort` —— 真到进程异常那一步 `Drop` 本身也不会执行，兜底代码是死的。
/// 留着它只会让「失败时进度槽会怎样」看起来比实际复杂。
struct RestoreRun;

impl RestoreRun {
    fn begin() -> Self {
        if let Ok(mut guard) = RESTORE_PROGRESS.lock() {
            *guard = Some(RestoreProgress {
                phase: RestorePhase::Reading,
                error: None,
                result: None,
                started_at: logging::now_ms(),
                finished_at: 0,
            });
        }
        Self
    }

    fn finish(self, result: Result<Value, String>) {
        if let Ok(mut guard) = RESTORE_PROGRESS.lock() {
            if let Some(progress) = guard.as_mut() {
                progress.finished_at = logging::now_ms();
                match result {
                    Ok(value) => {
                        progress.phase = RestorePhase::Done;
                        progress.result = Some(value);
                    }
                    Err(message) => {
                        progress.phase = RestorePhase::Failed;
                        progress.error = Some(message);
                    }
                }
            }
        }
    }
}

/// 多行 VALUES 批的上限：行数封顶 + 字节封顶（大行单独成批，因为它拆不开）。
/// 批量把「每行一次语句解析」摊薄成每批一次，cc-switch 实测 2 万行
/// 21 秒 → <100ms。
const INSERT_BATCH_MAX_ROWS: usize = 200;
const INSERT_BATCH_MAX_BYTES: usize = 1024 * 1024;

/// authorizer 放行的 PRAGMA：dump 的产物只含这两条（头部与结尾各一次）。
/// 其余一律拒绝 —— `temp_store_directory` 能把临时文件重定向到任意目录，
/// `writable_schema` 能绕过 schema 完整性检查。
const IMPORT_ALLOWED_PRAGMAS: &[&str] = &["foreign_keys", "user_version"];

// ── 导出 ────────────────────────────────────────────────────────

/// 导出密码加密完整包到用户指定路径。
///
/// `include_logs = false` 为精简导出（跳过日志与调试报文五张表）。
/// 返回 `{ bytes, tables: {表名: 行数} }` 给前端展示。
pub fn export_to_file(
    db: &Db,
    target: &Path,
    include_logs: bool,
    password: &str,
) -> Result<Value, String> {
    archive::validate_password(password, true)?;
    let parent = target.parent().ok_or("备份路径缺少父目录")?;
    std::fs::create_dir_all(parent).map_err(|error| format!("创建备份目录失败: {error}"))?;
    let mut output = tempfile::Builder::new()
        .prefix(".agent2api-backup-")
        .tempfile_in(parent)
        .map_err(|error| format!("创建备份文件失败: {error}"))?;
    let result = archive::export(
        db,
        output.as_file_mut(),
        include_logs,
        password,
        &crate::paths::config_dir(),
    )?;
    output
        .as_file()
        .sync_all()
        .map_err(|error| format!("同步备份文件失败: {error}"))?;
    output
        .persist(target)
        .map_err(|error| format!("保存备份文件失败: {}", error.error))?;
    logging::log(
        "[Storage]",
        &format!("📤 加密整库备份完成: {}", target.display()),
    );
    Ok(result)
}

pub fn export_to_writer(
    db: &Db,
    writer: &mut impl Write,
    include_logs: bool,
    password: &str,
) -> Result<Value, String> {
    archive::export(
        db,
        writer,
        include_logs,
        password,
        &crate::paths::config_dir(),
    )
}

/// 主库 → 内存库的一致性快照（主连接锁只持拷贝那一下，见模块头）。
fn snapshot_to_memory(db: &Db) -> Result<Connection, String> {
    db.with(|conn| {
        let mut snapshot =
            Connection::open_in_memory().map_err(|e| format!("创建内存快照库失败: {e}"))?;
        {
            let backup =
                Backup::new(conn, &mut snapshot).map_err(|e| format!("初始化快照失败: {e}"))?;
            complete_backup(&backup, "创建内存快照")?;
        }
        Ok(snapshot)
    })
    .unwrap_or_else(|| Err("数据库连接不可用，导出未执行".to_string()))
}

/// SQLite Backup 的收尾：`step(-1)` 必须以 Done 结束，Busy/Locked/More 都是
/// 「没拷完」—— 当成功返回会留下半截库，比直接报错严重得多。
fn complete_backup(backup: &Backup<'_, '_>, context: &str) -> Result<(), String> {
    let result = backup.step(-1).map_err(|e| format!("{context}失败: {e}"))?;
    match result {
        StepResult::Done => Ok(()),
        other => Err(format!("{context}未完成: SQLite Backup 返回 {other:?}")),
    }
}

/// 把整库按多行 VALUES 批写入 SQL。
fn dump_sql_into(
    conn: &Connection,
    skip_tables: &[&str],
    writer: &mut impl Write,
) -> Result<(), String> {
    let mut output = String::new();
    let user_version: i64 = conn
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .map_err(|e| format!("读取库版本失败: {e}"))?;

    output.push_str(&format!(
        "{BACKUP_SQL_HEADER}\n-- 生成时间: {}\n-- user_version: {user_version}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
    ));
    // SQL 保留指纹兼容旧导入；密钥本体只存放在外层加密清单中。
    output.push_str(&account_key_fingerprint_line());
    output.push_str("PRAGMA foreign_keys=OFF;\n");
    output.push_str(&format!("PRAGMA user_version={user_version};\n"));
    output.push_str("BEGIN TRANSACTION;\n");

    // schema：表/索引/视图的 DDL 先出（`type='table' DESC` 保证建表在索引前），
    // trigger 挪到数据装载之后 —— 提前建的 trigger 会在 INSERT 时触发，
    // 篡改 dump 出来的行，甚至打断多行 INSERT 的剩余部分。
    let mut stmt = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_master
             WHERE sql NOT NULL AND type IN ('table','index','trigger','view')
             ORDER BY type='table' DESC, name",
        )
        .map_err(|e| format!("读取 schema 失败: {e}"))?;
    let mut tables = Vec::new();
    let mut triggers = Vec::new();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("读取 schema 失败: {e}"))?;
    for row in rows {
        let (obj_type, name, sql) = row.map_err(|e| format!("读取 schema 失败: {e}"))?;
        if name.starts_with("sqlite_") {
            continue;
        }
        if obj_type == "trigger" {
            triggers.push(sql);
            continue;
        }
        output.push_str(&sql);
        output.push_str(";\n");
        if obj_type == "table" {
            tables.push(name);
        }
    }

    writer
        .write_all(output.as_bytes())
        .map_err(|error| format!("写入备份 SQL 失败: {error}"))?;
    output.clear();
    for table in &tables {
        if skip_tables.contains(&table.as_str()) {
            continue;
        }
        dump_table_rows(conn, table, writer)?;
    }

    dump_sqlite_sequences(conn, skip_tables, &mut output)?;

    for sql in triggers {
        output.push_str(&sql);
        output.push_str(";\n");
    }

    output.push_str("COMMIT;\nPRAGMA foreign_keys=ON;\n");
    writer
        .write_all(output.as_bytes())
        .map_err(|error| format!("写入备份 SQL 失败: {error}"))
}

/// 文件头那行账号密钥指纹注释（含结尾换行）。
///
/// SQL 头部只记录指纹；完整包的密钥本体位于外层加密清单。
/// 读指纹用 `crypto::key_fingerprint`（只读），**不会**因为有这个调用就生成密钥 ——
/// 导出是纯读动作，不能在用户机器上凭空造出一把 `account.key`。
fn account_key_fingerprint_line() -> String {
    let fingerprint = crypto::key_fingerprint(crypto::SecretScope::Accounts)
        .unwrap_or_else(|| KEY_FINGERPRINT_ABSENT.to_string());
    format!("{KEY_FINGERPRINT_LINE} {fingerprint}\n")
}

/// 一张表的数据 → 多行 VALUES 的 INSERT 批。
fn dump_table_rows(conn: &Connection, table: &str, writer: &mut impl Write) -> Result<(), String> {
    let quoted_table = quote_identifier(table);
    let columns = table_columns(conn, table)?;
    if columns.is_empty() {
        return Ok(());
    }
    let quoted_columns = columns
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_prefix = format!("INSERT INTO {quoted_table} ({quoted_columns}) VALUES ");

    let mut stmt = conn
        .prepare(&format!("SELECT {quoted_columns} FROM {quoted_table}"))
        .map_err(|e| format!("读取表 {table} 失败: {e}"))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| format!("读取表 {table} 失败: {e}"))?;

    let mut pending_rows = 0usize;
    let mut batch = String::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| format!("读取表 {table} 失败: {e}"))?
    {
        let mut values = Vec::with_capacity(columns.len());
        for idx in 0..columns.len() {
            let value = row
                .get_ref(idx)
                .map_err(|e| format!("读取表 {table} 失败: {e}"))?;
            values.push(format_sql_value(value)?);
        }

        let row_sql = format!("({})", values.join(", "));
        let separator = usize::from(pending_rows > 0);
        if pending_rows > 0 && batch.len() + separator + row_sql.len() + 2 > INSERT_BATCH_MAX_BYTES
        {
            batch.push_str(";\n");
            writer
                .write_all(batch.as_bytes())
                .map_err(|error| format!("写入表 {table} 备份失败: {error}"))?;
            pending_rows = 0;
        }
        if pending_rows == 0 {
            batch.clear();
            batch.push_str(&insert_prefix);
        } else {
            batch.push(',');
        }
        batch.push_str(&row_sql);
        pending_rows += 1;

        if pending_rows >= INSERT_BATCH_MAX_ROWS {
            batch.push_str(";\n");
            writer
                .write_all(batch.as_bytes())
                .map_err(|error| format!("写入表 {table} 备份失败: {error}"))?;
            pending_rows = 0;
        }
    }
    if pending_rows > 0 {
        batch.push_str(";\n");
        writer
            .write_all(batch.as_bytes())
            .map_err(|error| format!("写入表 {table} 备份失败: {error}"))?;
    }
    Ok(())
}

/// 自增水位（sqlite_sequence）单独重放：数据 INSERT 会把它推到 MAX(rowid)，
/// 丢掉「删过的高水位」。导出期间 sequence 表不存在（没用 AUTOINCREMENT）就跳过。
fn dump_sqlite_sequences(
    conn: &Connection,
    skip_tables: &[&str],
    output: &mut String,
) -> Result<(), String> {
    if !table_exists(conn, "sqlite_sequence")? {
        return Ok(());
    }
    let mut stmt = conn
        .prepare("SELECT name, seq FROM sqlite_sequence ORDER BY name")
        .map_err(|e| format!("读取自增序列失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| format!("读取自增序列失败: {e}"))?;
    let mut values = Vec::new();
    for row in rows {
        let (table, sequence) = row.map_err(|e| format!("读取自增序列失败: {e}"))?;
        if skip_tables.contains(&table.as_str()) {
            continue;
        }
        values.push(format!(
            "({}, {})",
            format_sql_value(rusqlite::types::ValueRef::Text(table.as_bytes()))?,
            sequence
        ));
    }
    output.push_str("DELETE FROM sqlite_sequence;\n");
    if !values.is_empty() {
        output.push_str("INSERT INTO sqlite_sequence (name, seq) VALUES ");
        output.push_str(&values.join(","));
        output.push_str(";\n");
    }
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get(0),
        )
        .map_err(|e| format!("检查表 {table} 是否存在失败: {e}"))?;
    Ok(count > 0)
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let quoted = quote_identifier(table);
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({quoted})"))
        .map_err(|e| format!("读取表 {table} 结构失败: {e}"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("读取表 {table} 结构失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取表 {table} 结构失败: {e}"))?;
    Ok(columns)
}

/// 表名/列名入 SQL：双引号包裹 + 内部双引号翻倍，杜绝拼串注入。
fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// SQL 字面量格式化：与 SQLite 自身规则对齐（NaN 归 NULL、整数 REAL 补小数点、
/// 含 NUL 的文本用 hex CAST —— SQL 解析器把 NUL 当语句结束，字面量放不下它）。
fn format_sql_value(value: rusqlite::types::ValueRef<'_>) -> Result<String, String> {
    match value {
        rusqlite::types::ValueRef::Null => Ok("NULL".to_string()),
        rusqlite::types::ValueRef::Integer(i) => Ok(i.to_string()),
        rusqlite::types::ValueRef::Real(f) => Ok(format_sql_real(f)),
        rusqlite::types::ValueRef::Text(t) => match std::str::from_utf8(t) {
            Ok(text) if !text.contains('\0') => Ok(format!("'{}'", text.replace('\'', "''"))),
            _ => Ok(format!("CAST({} AS TEXT)", format_sql_blob(t))),
        },
        rusqlite::types::ValueRef::Blob(bytes) => Ok(format_sql_blob(bytes)),
    }
}

fn format_sql_real(value: f64) -> String {
    if value.is_nan() {
        return "NULL".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-9.0e999".to_string()
        } else {
            "9.0e999".to_string()
        };
    }
    if value == 0.0 && value.is_sign_negative() {
        return "-0.0".to_string();
    }
    let mut literal = value.to_string();
    if !literal.contains(['.', 'e', 'E']) {
        literal.push_str(".0");
    }
    literal
}

fn format_sql_blob(bytes: &[u8]) -> String {
    let mut s = String::from("X'");
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02X}");
    }
    s.push('\'');
    s
}

/// 快照里各业务表的行数（导出统计；只数被导出的表）。
fn table_row_counts(conn: &Connection, skip: &[&str]) -> Vec<Value> {
    let known = [
        "kv",
        "accounts",
        "logs",
        "requests",
        "request_daily",
        "debug_traffic",
        "request_raw",
        "account_usage_records",
        "account_limiter_events",
        "checkin_records",
        "assistant_sessions",
        "assistant_images",
    ];
    known
        .iter()
        .filter(|table| !skip.contains(table))
        .filter_map(|table| {
            let count: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {}", quote_identifier(table)),
                    [],
                    |row| row.get(0),
                )
                .ok()?;
            Some(json!({ "table": table, "rows": count }))
        })
        .collect()
}

// ── 导入 ────────────────────────────────────────────────────────

/// 从备份文件头部解析 `-- account-key-fingerprint:` 行的值。
///
/// 只扫开头 `HEADER_SCAN_LINES` 行（头部是固定几行注释），不为一行标记遍历正文。
/// 旧版本导出的文件没有这一行 → `None`（无法比对，调用方按「无信息」处理）。
fn parse_account_key_fingerprint(sql: &str) -> Option<String> {
    sql.lines()
        .take(HEADER_SCAN_LINES)
        .find_map(|line| line.trim().strip_prefix(KEY_FINGERPRINT_LINE))
        .map(|value| value.trim().to_string())
}

/// 比对备份头部记录的密钥指纹与本机现状，返回**给用户看的警告**（未比对出问题 →
/// `None`）。
///
/// 场景与结论：
///   · 两边指纹一致（最常见：同机恢复）→ 不警告；
///   · 备份写的是「未生成」→ 警告（导出时那把钥匙还是「不存在」状态，本机若已有
///     密钥，备份里的账号密文多半解不开）；
///   · 指纹不同 / 本机没有密钥 → 警告（这份备份是**另一把密钥**加密的）。
///
/// **绝不因此阻断导入**：用户常常就是要「先把数据导回来，再单独处理密钥」
/// （例如先导数据、再把 `account.key` 拷过来重导，或先看看数据再决定）。
/// 所以这里只产出文案，由调用方放进返回值冒泡给界面 / 日志。
fn account_key_warning(backup_marker: Option<String>) -> Option<String> {
    // 旧备份没有这一行：没有任何可比较的信息，静默（不拿「没记」当「不匹配」报）
    let backup_marker = backup_marker?;
    if backup_marker == KEY_FINGERPRINT_ABSENT {
        return Some(format!(
            "这份备份导出时账号加密密钥尚未生成，无法校验密钥是否一致：若账号数据无法解密（账号列表为空），\
             请把备份那台机器的 {ACCOUNT_KEY_FILE} 一并带过来后重新导入。"
        ));
    }
    let local = crypto::key_fingerprint(crypto::SecretScope::Accounts);
    if local.as_deref() == Some(backup_marker.as_str()) {
        return None;
    }
    Some(format!(
        "这份备份是用另一把加密密钥加密的，导入后账号数据将无法解密（账号列表可能为空）。\
         若要在别的机器恢复，请把备份那台机器的 {ACCOUNT_KEY_FILE} 一并带过来。"
    ))
}

/// 从加密包或旧 SQL 恢复整库，失败时尝试从安全备份还原。
///
/// 返回表统计、安全备份和凭证迁移结果；旧 SQL 密钥不匹配时附加警告。
pub fn import_from_file(
    db: &Db,
    source: &Path,
    config_dir: &Path,
    password: &str,
) -> Result<Value, String> {
    let _guard = match RESTORE_MUTEX.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            return Err("整库恢复正在进行，请等待当前恢复完成".into())
        }
    };
    let run = RestoreRun::begin();
    let result = restore::import(db, source, config_dir, password);
    run.finish(result.clone());
    result
}

/// 导入期间挂到暂存库上的守卫：拒绝一切能**离开暂存库文件**的动作。
/// 按解析结果在 prepare 阶段拦截（见模块头「导入的信任模型」）。
fn import_authorizer(context: AuthContext<'_>) -> Authorization {
    let escapes_temp_db = match context.action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => true,
        AuthAction::CreateVtable { .. } | AuthAction::DropVtable { .. } => true,
        AuthAction::Unknown { .. } => true,
        AuthAction::Pragma { pragma_name, .. } => !IMPORT_ALLOWED_PRAGMAS
            .iter()
            .any(|allowed| pragma_name.eq_ignore_ascii_case(allowed)),
        _ => false,
    };
    if escapes_temp_db {
        logging::log(
            "[Storage]",
            &format!("备份导入拒绝了越界语句: {:?}", context.action),
        );
        Authorization::Deny
    } else {
        Authorization::Allow
    }
}

fn validate_required_tables(conn: &Connection) -> Result<(), String> {
    let mut missing = Vec::new();
    for table in REQUIRED_TABLES {
        if !table_exists(conn, table)? {
            missing.push(*table);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "备份缺少必需的数据表：{}（文件可能不完整）",
        missing.join(", ")
    ))
}
