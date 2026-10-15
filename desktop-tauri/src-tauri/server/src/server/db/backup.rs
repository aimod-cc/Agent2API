//! 整库 SQL 备份 / 恢复（设置页「整库备份 / 恢复」）。
//!
//! ```text
//! export_to_file  库 → SQL 文本（schema + 全部表数据 + 自增序列 + 触发器）
//! import_from_file SQL 文本 → 暂存库校验 → 整体覆盖主库（替换语义，不是合并）
//! ```
//!
//! ── 为什么是「整体替换」而不是账号导入那套 merge ───────────────
//! 账号导入（`core::account_transfer`）按「provider + 业务身份」逐条合并，
//! 服务的是「换机器继续用某个账号池」；本模块服务的是「把这台机器**整个**
//! 变成备份那台机器的状态」——账号、配置、日志、统计、面板凭据一起换。
//! 两者的隔离规则互相冲突（合并要保留本机顺序，替换要把本机也换掉），
//! 因此各走各的入口，旧 JSON 端点原样保留。
//!
//! ── 导出为什么先快照到内存库再 dump ─────────────────────────
//! dump 是一次遍历全部表的长操作，直接在主连接上做会把那把连接锁持几十秒，
//! 期间所有管理 API 与转发记账全部排队。先用 SQLite Backup API 把主库拷进
//! 内存库（锁只持拷贝那一下），再从内存库慢慢 dump —— 拿到的是**一致性
//! 时点**的快照，服务却只停顿拷贝那一下（与 `db` 模块「持锁绝不做慢操作」
//! 的纪律同源；cc-switch 同款做法）。
//!
//! ── 导入的信任模型与 authorizer ─────────────────────────────
//! 头部校验只比较一个注释前缀，任何人都能在合法前缀后面接着写别的语句。
//! 外部 SQL 在 `NamedTempFile` 建的一次性暂存库上执行，那段 SQL 里**唯一
//! 有意义的边界是暂存库文件本身**（`DELETE` / `DROP` 给不了攻击者任何新
//! 东西——库的内容本来就由这份 SQL 决定），因此 authorizer 拒绝的是
//! 「能离开暂存库」的动作：`ATTACH`（含 `VACUUM INTO`，实测同报
//! `AuthAction::Attach`）、文件后端虚拟表、未识别动作（SQLite 将来新增的
//! 跨文件语句默认落进这里）。按解析结果在 prepare 阶段拦截，字符串扫描
//! 绕不过。这份输入还可能是 WebDAV 同步回来的（未来路径），所以按不可信
//! 输入设防，而不是信「只会有自家导出」。
//!
//! ── 替换前必先安全备份 ─────────────────────────────────────
//! 替换是破坏性的：先在 `backups/` 目录用 Backup API 给当前主库拍一份
//! `.db` 快照，替换失败或用户后悔时可整文件拷回（数据目录里，无需工具
//! 即可辨认）。这份快照同时是「替换窗口」的一致性留影。
//!
//! ── 替换后要重装哪些内存状态（最容易漏的一处）───────────────
//! 逐个核实过（与 `upgrade_api` 的清单同一套论证）：
//!   - `config`：必须 `reload()` —— 快照在 bootstrap 装入，替换后库才是真相；
//!   - `access`：刷新令牌有内存副本、且 `persist_refresh_tokens` 会把内存表
//!     **整体写回** kv，不重载的话下一次任意设备刷新令牌就会把旧库的令牌
//!     表盖回新库；短效令牌直接清空（备份里的 refresh 记录对不上本机浏览
//!     器的 cookie，反正要重登）；
//!   - 账号 / 日志 / 统计 / 调试报文 / 桌面设置（壳侧）：都无内存快照，
//!     下一个查询自然看到新库。
//!
//! ── 账号密钥指纹（为什么头部要记一行哈希）─────────────────────
//! 整库备份只 dump 表数据，**不含** `{config_dir}/account.key` 密钥文件。把备份
//! 拿到另一台机器恢复、或本机密钥文件丢失后恢复，`accounts.data` 里的密文全部
//! 解不开 —— 用户只看到「账号列表为空」，查不出原因。因此导出时在文件头写一行
//! `-- account-key-fingerprint: <sha256(密钥)[:16]>`（**哈希，不是密钥**，见
//! `core::crypto::key_fingerprint`；密钥尚不存在时写「(未生成)」），导入时与本机
//! 指纹比对：不一致或「(未生成)」就在返回值里附一条 `warning` 说明原因与补救办法
//! （把 `account.key` 一起带过来）。**不阻断导入** —— 用户可能就是要先把数据导
//! 回来再处理密钥。该行是文件头前缀之后的注释，不影响「以 BACKUP_SQL_HEADER
//! 开头」的判定；旧备份没有这一行则无从比对，静默放行。
//!
//! ── schema 版本 ────────────────────────────────────────────
//! 导出文件头与 `PRAGMA user_version` 都带库版本。导入侧先看暂存库执行后
//! 的真实版本：**高于**当前 `SCHEMA_VERSION` 直接拒绝（旧程序打开新库，
//! 缺对应代码）；更低的在暂存库上跑 `schema::migrate` 升上来再替换 ——
//! 迁移是幂等 DDL，老版本备份照样可导。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Local;
use rusqlite::backup::{Backup, StepResult};
use rusqlite::hooks::{AuthAction, Authorization, AuthContext};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

use super::schema;
use super::Db;
use crate::server::core::crypto;
use crate::server::logging;

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
/// 事实来源是 `crypto::SecretScope::key_file`，但那是私有实现细节；这里重复的
/// 只是一个**用户可见的文件名**，改动概率极低（改了它等于换密钥文件，见 crypto 模块头）。
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

/// 导出整库为 SQL 文本并写到 `target`。
///
/// `include_logs = false` 为精简导出（跳过日志与调试报文五张表）。
/// 返回 `{ bytes, tables: {表名: 行数} }` 给前端展示。
pub fn export_to_file(db: &Db, target: &Path, include_logs: bool) -> Result<Value, String> {
    let snapshot = snapshot_to_memory(db)?;
    let skip: &[&str] = if include_logs { &[] } else { LOG_TABLES };
    let dump = dump_sql(&snapshot, skip)?;
    atomic_write(target, dump.as_bytes())?;
    let tables = table_row_counts(&snapshot, skip);
    logging::log(
        "[Storage]",
        &format!(
            "📤 整库导出完成: {}（{} 字节，{} 张表）",
            target.display(),
            dump.len(),
            tables.len()
        ),
    );
    Ok(json!({ "bytes": dump.len(), "tables": tables }))
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
    let result = backup
        .step(-1)
        .map_err(|e| format!("{context}失败: {e}"))?;
    match result {
        StepResult::Done => Ok(()),
        other => Err(format!("{context}未完成: SQLite Backup 返回 {other:?}")),
    }
}

/// 把整库 dump 成 SQL 文本（cc-switch 同款格式；多行 VALUES 批量插入）。
fn dump_sql(conn: &Connection, skip_tables: &[&str]) -> Result<String, String> {
    let mut output = String::new();
    let user_version: i64 = conn
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .map_err(|e| format!("读取库版本失败: {e}"))?;

    output.push_str(&format!(
        "{BACKUP_SQL_HEADER}\n-- 生成时间: {}\n-- user_version: {user_version}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
    ));
    // 追加账号密钥指纹（注释行，紧跟头部其它注释）：本备份**不含** `account.key`
    // 密钥文件，换了机器或密钥文件丢失后恢复，`accounts.data` 的密文就再也解不开，
    // 界面只表现为「账号列表为空」。把导出时那把密钥的**指纹**（哈希，不是密钥
    // 本身，见 `crypto::key_fingerprint`）记下来，导入侧才能比对并明确告知原因。
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

    for table in &tables {
        if skip_tables.contains(&table.as_str()) {
            continue;
        }
        dump_table_rows(conn, table, &mut output)?;
    }

    dump_sqlite_sequences(conn, skip_tables, &mut output)?;

    for sql in triggers {
        output.push_str(&sql);
        output.push_str(";\n");
    }

    output.push_str("COMMIT;\nPRAGMA foreign_keys=ON;\n");
    Ok(output)
}

/// 文件头那行账号密钥指纹注释（含结尾换行）。
///
/// 密钥存在 → 写指纹；不存在 → 写「未生成」标记。**绝不写密钥本身**：备份是明文
/// SQL，会被拷到别的机器，写进去就等于把钥匙交给拿到备份的人（见 `crypto` 模块头）。
/// 读指纹用 `crypto::key_fingerprint`（只读），**不会**因为有这个调用就生成密钥 ——
/// 导出是纯读动作，不能在用户机器上凭空造出一把 `account.key`。
fn account_key_fingerprint_line() -> String {
    let fingerprint = crypto::key_fingerprint(crypto::SecretScope::Accounts)
        .unwrap_or_else(|| KEY_FINGERPRINT_ABSENT.to_string());
    format!("{KEY_FINGERPRINT_LINE} {fingerprint}\n")
}

/// 一张表的数据 → 多行 VALUES 的 INSERT 批。
fn dump_table_rows(conn: &Connection, table: &str, output: &mut String) -> Result<(), String> {
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
    while let Some(row) = rows.next().map_err(|e| format!("读取表 {table} 失败: {e}"))? {
        let mut values = Vec::with_capacity(columns.len());
        for idx in 0..columns.len() {
            let value = row.get_ref(idx).map_err(|e| format!("读取表 {table} 失败: {e}"))?;
            values.push(format_sql_value(value)?);
        }

        let row_sql = format!("({})", values.join(", "));
        let separator = usize::from(pending_rows > 0);
        if pending_rows > 0 && batch.len() + separator + row_sql.len() + 2 > INSERT_BATCH_MAX_BYTES
        {
            batch.push_str(";\n");
            output.push_str(&batch);
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
            output.push_str(&batch);
            pending_rows = 0;
        }
    }
    if pending_rows > 0 {
        batch.push_str(";\n");
        output.push_str(&batch);
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
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
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
            Ok(text) if !text.contains('\0') => {
                Ok(format!("'{}'", text.replace('\'', "''")))
            }
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
    let known = ["kv", "accounts", "logs", "requests", "request_daily", "debug_traffic", "request_raw", "account_usage_records"];
    known
        .iter()
        .filter(|table| !skip.contains(table))
        .filter_map(|table| {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {}", quote_identifier(table)), [], |row| row.get(0))
                .ok()?;
            Some(json!({ "table": table, "rows": count }))
        })
        .collect()
}

/// 原子写盘：先写 `.tmp` 再改名覆盖 —— 中途失败最多留下一个临时文件，
/// 目标要么是旧内容要么是完整新内容，绝不半截。
fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("写入临时文件失败: {e}"))?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换目标文件失败: {e}")
    })?;
    Ok(())
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

/// 从 SQL 文件恢复整库：校验 → 暂存库执行 → 安全备份 → 整体覆盖主库。
///
/// 返回 `{ tables, safetyBackup, warning? }`；替换后的内存态重装在本函数内完成
/// （见模块头「替换后要重装哪些内存状态」）。`warning` 只在密钥指纹对不上时出现
/// ——**不阻断导入**，只是把「账号为什么解不开」明确说出来（见 `account_key_warning`）。
pub fn import_from_file(db: &Db, source: &Path, config_dir: &Path) -> Result<Value, String> {
    let sql_raw = std::fs::read_to_string(source)
        .map_err(|e| format!("读取备份文件失败: {e}"))?;
    let sql_content = sql_raw.trim_start_matches('\u{feff}');
    if !sql_content.trim_start().starts_with(BACKUP_SQL_HEADER) {
        return Err(
            "仅支持导入由 Agent2API 导出的整库备份文件（缺少文件头标记）".to_string(),
        );
    }
    // 密钥指纹比对：在重活之前先算好文案，最后随结果一起返回。这里只看头部注释，
    // 不碰暂存库；比对结果**不影响**导入流程（见 account_key_warning 的注释）。
    let key_warning = account_key_warning(parse_account_key_fingerprint(sql_content));

    // 暂存库：外部 SQL 在一次性文件上执行，失败不污染主库。
    // journal_mode 先对齐主库的 WAL —— Backup 会把源文件头拷进目标，
    // 暂存库若停在默认的回滚日志模式，覆盖后主库会被拉出 WAL
    // （cc-switch 对 auto_vacuum 处理过同一类坑）。
    let temp = TempRestoreDb::new()?;
    let conn = &temp.conn;
    conn.execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(|e| format!("设置暂存库 WAL 失败: {e}"))?;

    // authorizer 只覆盖外部 SQL；紧随其后的 schema::migrate 是本程序自己的
    // 语句，不属于需要设防的输入，摘掉再跑。（两次设置在连接未执行语句时
    // 不会失败，Result 按惯例收掉。）
    let _ = conn.authorizer(Some(import_authorizer));
    let batch_result = conn.execute_batch(sql_content);
    let _ = conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    batch_result.map_err(|e| format!("执行备份 SQL 失败: {e}"))?;
    // 未回到自动提交 = 事务没有走到 COMMIT，文件多半被截断 —— 拒绝。
    if !conn.is_autocommit() {
        let _ = conn.execute_batch("ROLLBACK;");
        return Err("备份文件的事务未完成，文件可能被截断".to_string());
    }

    // 版本闸门：比当前程序新的库不收（migrate 只升不降，旧程序缺对应代码）。
    let restored_version: i64 = conn
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .map_err(|e| format!("读取备份库版本失败: {e}"))?;
    if restored_version > schema::SCHEMA_VERSION {
        return Err(format!(
            "备份来自更新版本的应用（库版本 {restored_version} > 当前 {}），请先升级本应用",
            schema::SCHEMA_VERSION
        ));
    }
    validate_required_tables(conn)?;
    // 补齐缺失表（精简备份的日志表）并把版本升到当前
    schema::migrate(conn).map_err(|e| format!("对齐备份库 schema 失败: {e}"))?;

    let tables = table_row_counts(conn, &[]);

    // 覆盖主库：进程锁内再持主连接锁（两把锁顺序固定，绝无嵌套死锁）。
    // 安全备份与覆盖共用一次持锁 —— 快照必须精确代表「被替换掉的状态」。
    let _guard = match RESTORE_MUTEX.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let safety = db
        .with_mut(|main| safety_backup(main, config_dir).and_then(|safety| {
            let backup = Backup::new(conn, main).map_err(|e| format!("初始化恢复失败: {e}"))?;
            complete_backup(&backup, "恢复主数据库")?;
            Ok(safety)
        }))
        .unwrap_or_else(|| Err("数据库连接不可用，恢复未执行".to_string()));
    let safety = safety?;

    // 锁已释放：重装内存态（config 快照、面板刷新令牌），短效令牌清空。
    crate::server::config::reload();
    crate::server::access::load_refresh_tokens();
    crate::server::access::clear_access_tokens();

    let safety_name = safety
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    logging::log(
        "[Storage]",
        &format!(
            "📥 整库恢复完成: {} 张表；替换前的数据已备份到 {}",
            tables.len(),
            safety.display()
        ),
    );
    let mut result = json!({ "tables": tables, "safetyBackup": safety_name });
    // 指纹警告放进返回值，让它冒泡到界面（HTTP handler 原样回传）与日志；
    // 键只在真有警告时出现，前端 `if (data.warning)` 即可。
    if let Some(warning) = key_warning {
        logging::log(
            "[Storage]",
            &format!("⚠️ 整库恢复密钥指纹不匹配（不阻断导入）: {warning}"),
        );
        result["warning"] = json!(warning);
    }
    Ok(result)
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

/// 替换前给当前主库拍 `.db` 快照到 `{config_dir}/backups/`。
/// 同一秒重复恢复时加序号避让，绝不覆盖已有备份。
fn safety_backup(main: &mut Connection, config_dir: &Path) -> Result<PathBuf, String> {
    let backup_dir = config_dir.join("backups");
    std::fs::create_dir_all(&backup_dir).map_err(|e| format!("创建备份目录失败: {e}"))?;

    let base = format!("pre-restore-{}", Local::now().format("%Y%m%d_%H%M%S"));
    let mut suffix = 0;
    let target = loop {
        let name = if suffix == 0 {
            format!("{base}.db")
        } else {
            format!("{base}_{suffix}.db")
        };
        let path = backup_dir.join(name);
        if !path.exists() {
            break path;
        }
        suffix += 1;
    };

    let mut dest = Connection::open(&target).map_err(|e| format!("创建安全备份失败: {e}"))?;
    {
        let backup =
            Backup::new(main, &mut dest).map_err(|e| format!("初始化安全备份失败: {e}"))?;
        complete_backup(&backup, "创建安全备份")?;
    }
    dest.close()
        .map_err(|(_, e)| format!("关闭安全备份失败: {e}"))?;
    Ok(target)
}

/// 一次性暂存库句柄：文件落在系统临时目录，Drop 时连 WAL/SHM 一起删
/// （只删主文件会把附属文件留在原地 —— `test_temp::TempDb` 同一条教训）。
struct TempRestoreDb {
    path: PathBuf,
    conn: Connection,
}

impl TempRestoreDb {
    fn new() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "agent2api-restore-{}-{}.db",
            std::process::id(),
            logging::now_ms()
        ));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .map_err(|e| format!("创建暂存库失败: {e}"))?;
        Ok(Self { path, conn })
    }
}

impl Drop for TempRestoreDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", self.path.display(), suffix));
        }
    }
}
