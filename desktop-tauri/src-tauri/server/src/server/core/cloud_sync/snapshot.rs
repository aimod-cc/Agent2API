//! 业务快照：**哪些数据进云同步、怎么取、怎么装回去**。
//!
//! ```text
//! build(db)   库 → snapshot.json（只含业务数据，值保持库里的原始形态）
//! apply(db, …) snapshot.json → 库（替换语义，单事务）
//! ```
//!
//! ── 为什么不是「整库导出」的复用 ─────────────────────────────
//! `db::backup` 导出的是**全部**表（含事件日志、请求明细、调试报文），当前
//! 这台机器的库有 200MB 量级且绝大部分是日志。云同步要的是「换台机器接着用」
//! 的那份数据 —— 账号、配置、签到与余额记录；日志是**本机的运行痕迹**，
//! 传上云既慢又没有意义（换机后看别人的历史请求日志只会误导）。
//!
//! 另外两者对「敏感数据」的口径也不同：整库备份是用户自己指定路径、自己保管；
//! 云同步是把文件交到第三方存储，所以面板登录态（`panelTokens` /
//! `panelAccessSessions` / `panelAdmin`）、桌面设置与后台任务排期一律不带。
//!
//! ── 为什么快照里存的是**库里的原始值**（密文原样）───────────
//! `accounts.data` 与 `kv.apiKeys` 在库里已经是 `enc1:` 信封（密钥是
//! `account.key`）。若在构建快照时解密，就等于把全部账号凭证以明文写进一个
//! 要上传到云盘的文件 —— 与「加密落库」的初衷正好相反。原样搬运的好处：
//!   · 快照文件本身不含明文凭证（只有密文 + 被同步密码封装的密钥文件）；
//!   · 应用时不需要「解密再加密」，少一次密钥依赖与一处失败点；
//!   · 与整库恢复的语义一致（那里也是搬 SQL 里的密文）。
//!
//! ── 哪些 kv 键进快照（唯一判定入口）─────────────────────────
//! 见 [`is_synced_kv_key`]。三类排除：
//!   1. **本机专属路径**：`logDir` / `requestStatsDir` / `debugDir` / `kernelDir`
//!      是绝对路径，换台机器指向不存在的目录；
//!   2. **另一把密钥加密的**：`githubToken` 用 `update-token.key`，而那个密钥
//!      文件不在同步范围（它只服务本机的更新检查）—— 带过去也解不开；
//!   3. **本机运行状态**：面板登录态、桌面设置、后台任务排期、各类迁移标记、
//!      模型缓存。它们要么与这台机器绑定，要么可以从零重建。
//!
//! ── 应用为什么是「替换」而不是「合并」────────────────────────
//! 与整库恢复同一取向：云同步服务的是「把这台机器变成另一台机器的状态」。
//! 合并语义（账号导入那套）解决的是另一个问题（保留本机顺序、只补凭证），
//! 两者混用会让「我明明点了下载，为什么本机多出来的账号还在」无从解释。

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::server::db::schema::{self, SCHEMA_VERSION};
use crate::server::db::Db;
use crate::server::logging;

use super::manifest::{ArtifactMeta, SyncManifest};

/// 快照文件里的格式标识（与 manifest 的 format 分开：一个是「信封」，
/// 一个是「内容」，将来可能只换其中一边）
pub(crate) const SNAPSHOT_FORMAT: &str = "agent2api-cloud-sync-snapshot";
pub(crate) const SNAPSHOT_VERSION: u32 = 1;

/// 进快照的表（顺序即插入顺序：账号先于其它，便于失败时一眼看出走到哪一步）
pub(crate) const SYNCED_TABLES: &[&str] = &["accounts", "account_usage_records", "checkin_records"];

/// 非保留配置键里**不进快照**的项（理由见模块头三类排除）。
const EXCLUDED_CONFIG_KEYS: &[&str] = &[
    // 本机绝对路径
    "logDir",
    "requestStatsDir",
    "debugDir",
    "kernelDir",
    // 另一把密钥（update-token.key）加密，带过去解不开
    "githubToken",
    // 代理内核的下载出网线路：绑本机网络环境
    "kernelProxy",
];

/// 保留键里**进快照**的项（其余保留键一律不进）。
///
/// 这三项的语义是「用户配好的数据」而不是「这台机器的运行状态」：
///   · `agentProfiles`：「Agent」页的客户端配置（要写到本机配置文件里的那套）
///   · `proxyPool` / `proxySettings` / `subscriptions`：「网络代理」页的节点、
///     端口范围与订阅链接
const SYNCED_RESERVED_KEYS: &[&str] = &["agentProfiles", "proxyPool", "proxySettings", "subscriptions"];

/// 这个 kv 键要不要进云同步。**唯一判定入口** —— 构建与比较都用它，
/// 避免「构建时排除、变更检测时算进去」这种自相矛盾。
pub(crate) fn is_synced_kv_key(key: &str) -> bool {
    if SYNCED_RESERVED_KEYS.contains(&key) {
        return true;
    }
    if schema::is_reserved(key) {
        // 其余保留键：本机状态（面板登录态 / 桌面设置 / 任务排期 / 迁移标记 /
        // 模型缓存 / cloudSync 自己）
        return false;
    }
    !EXCLUDED_CONFIG_KEYS.contains(&key)
}

/// 快照文件结构
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub format: String,
    pub version: u32,
    pub schema_version: i64,
    /// 导出时刻（毫秒）
    pub exported_at: i64,
    /// 表名 → 行数组（每行是「列名 → 值」）
    pub tables: BTreeMap<String, Vec<Map<String, Value>>>,
    /// kv 键 → **原始 JSON 文本**（库里 value 列的原样内容）
    pub kv: BTreeMap<String, String>,
}

impl Snapshot {
    /// 内容哈希：用于「本地内容没变就不上传」与 manifest 的 snapshotId。
    ///
    /// 直接哈希**序列化后的字节**：两次构建的字段顺序由 `BTreeMap` 与
    /// `serde_json` 的结构化序列化保证稳定，不必自己做规范化。
    pub(crate) fn content_hash(&self) -> String {
        match serde_json::to_vec(self) {
            Ok(bytes) => super::sha256_hex(&bytes),
            Err(_) => String::new(),
        }
    }

    /// 行数摘要（界面展示「这份快照里有多少数据」）
    pub(crate) fn row_counts(&self) -> Value {
        let mut counts = Map::new();
        for (table, rows) in &self.tables {
            counts.insert(table.clone(), json!(rows.len()));
        }
        counts.insert("kv".to_string(), json!(self.kv.len()));
        Value::Object(counts)
    }
}

/// 构建本机业务快照。
///
/// 单连接锁内完成全部读：账号、用量、签到三张表与 kv 之间没有跨表一致性要求
/// （它们不是同一个事务里写的），但**同一个快照里**的账号与配置应当尽量来自
/// 相近的时刻 —— 一次持锁读完全部是最省事也最接近的做法。
pub(crate) fn build(db: &Db) -> Result<Snapshot, String> {
    let exported_at = logging::now_ms();
    db.with(|conn| build_conn(conn, exported_at))
        .ok_or_else(|| "数据库连接不可用，无法生成同步快照".to_string())?
}

fn build_conn(conn: &Connection, exported_at: i64) -> Result<Snapshot, String> {
    let mut tables = BTreeMap::new();
    for table in SYNCED_TABLES {
        tables.insert((*table).to_string(), read_table(conn, table)?);
    }

    let mut kv = BTreeMap::new();
    let mut stmt = conn
        .prepare("SELECT key, value FROM kv ORDER BY key")
        .map_err(|error| format!("读取配置失败: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("读取配置失败: {error}"))?;
    for row in rows {
        let (key, value) = row.map_err(|error| format!("读取配置失败: {error}"))?;
        if is_synced_kv_key(&key) {
            kv.insert(key, value);
        }
    }

    Ok(Snapshot {
        format: SNAPSHOT_FORMAT.to_string(),
        version: SNAPSHOT_VERSION,
        schema_version: SCHEMA_VERSION,
        exported_at,
        tables,
        kv,
    })
}

/// 读一张表的全部行（列名 → JSON 值）。
///
/// 列清单来自 `PRAGMA table_info`，与整库导出的做法一致 —— 硬编码列名会在
/// 下次加列时悄悄漏掉新字段（而这里漏掉的后果是「同步过去少了一列数据」）。
fn read_table(conn: &Connection, table: &str) -> Result<Vec<Map<String, Value>>, String> {
    let columns = table_columns(conn, table)?;
    if columns.is_empty() {
        return Ok(Vec::new());
    }
    let quoted = columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {quoted} FROM {}",
            quote_identifier(table)
        ))
        .map_err(|error| format!("读取 {table} 失败: {error}"))?;
    let mut rows = stmt
        .query([])
        .map_err(|error| format!("读取 {table} 失败: {error}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("读取 {table} 失败: {error}"))?
    {
        let mut entry = Map::new();
        for (index, column) in columns.iter().enumerate() {
            let value = row
                .get_ref(index)
                .map_err(|error| format!("读取 {table}.{column} 失败: {error}"))?;
            entry.insert(column.clone(), sql_to_json(value));
        }
        out.push(entry);
    }
    Ok(out)
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))
        .map_err(|error| format!("读取 {table} 结构失败: {error}"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("读取 {table} 结构失败: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取 {table} 结构失败: {error}"))?;
    Ok(columns)
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn sql_to_json(value: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(number) => json!(number),
        ValueRef::Real(number) => json!(number),
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => Value::String(text.to_string()),
            // 非 UTF-8 的文本列在本项目里不存在（全部由我们自己写入），
            // 真出现就按 base64 之外的形态给 null —— 不该让它毁掉整份快照
            Err(_) => Value::Null,
        },
        ValueRef::Blob(bytes) => json!(base64_encode(bytes)),
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    STANDARD.encode(bytes)
}

// ─── 应用 ─────────────────────────────────────────────────────

/// 应用一份快照到库（替换语义）。
///
/// 返回 `{ tables: {表: 行数}, kv: n }` 给界面展示。
///
/// ── 列交集 ──────────────────────────────────────────────────
/// 快照里的列可能与当前库不完全一致（远端来自更旧 / 更新的版本）。这里按
/// **当前表的列**取交集：快照多出来的列忽略（本程序不认识），快照缺少的列
/// 走建表默认值。`schema_version` 高于本机的快照在 `manifest::validate_compat`
/// 就已经拒了，所以「多出来的列」只可能是同版本下的手改数据。
///
/// ── 只删同步范围内的 kv ─────────────────────────────────────
/// 若 `DELETE FROM kv` 全删，本机的面板登录态、桌面设置、任务排期会一起消失 ——
/// 用户下载一次云同步就被踢出登录。所以 kv 只删 `is_synced_kv_key` 命中的键。
pub(crate) fn apply(db: &Db, snapshot: &Snapshot) -> Result<Value, String> {
    if snapshot.format != SNAPSHOT_FORMAT {
        return Err(format!(
            "远端快照格式不兼容（format = {}）",
            snapshot.format
        ));
    }
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(format!(
            "远端快照协议版本不兼容（v{}，本机 v{SNAPSHOT_VERSION}）",
            snapshot.version
        ));
    }
    if snapshot.schema_version > SCHEMA_VERSION {
        return Err(format!(
            "远端快照来自更新版本的应用（库版本 {} > 当前 {SCHEMA_VERSION}）",
            snapshot.schema_version
        ));
    }

    db.with_mut(|conn| apply_conn(conn, snapshot))
        .ok_or_else(|| "数据库连接不可用，无法应用同步快照".to_string())?
}

fn apply_conn(conn: &mut Connection, snapshot: &Snapshot) -> Result<Value, String> {
    let tx = conn
        .transaction()
        .map_err(|error| format!("开启同步应用事务失败: {error}"))?;

    let mut applied = Map::new();
    for table in SYNCED_TABLES {
        let Some(rows) = snapshot.tables.get(*table) else {
            // 快照里没有这张表 = 远端那一版还没这个概念，跳过（不清空本机）
            continue;
        };
        let current_columns = table_columns(&tx, table)?;
        if current_columns.is_empty() {
            continue;
        }
        tx.execute(&format!("DELETE FROM {}", quote_identifier(table)), [])
            .map_err(|error| format!("清空 {table} 失败: {error}"))?;
        let mut inserted = 0usize;
        for row in rows {
            insert_row(&tx, table, &current_columns, row)?;
            inserted += 1;
        }
        applied.insert((*table).to_string(), json!(inserted));
    }

    // kv：只删同步范围内的键，再写快照里的键
    let mut stale = Vec::new();
    {
        let mut stmt = tx
            .prepare("SELECT key FROM kv")
            .map_err(|error| format!("读取配置键清单失败: {error}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("读取配置键清单失败: {error}"))?;
        for row in rows {
            let key = row.map_err(|error| format!("读取配置键清单失败: {error}"))?;
            if is_synced_kv_key(&key) {
                stale.push(key);
            }
        }
    }
    for key in &stale {
        tx.execute("DELETE FROM kv WHERE key = ?1", rusqlite::params![key])
            .map_err(|error| format!("删除配置项 {key} 失败: {error}"))?;
    }
    for (key, value) in &snapshot.kv {
        // 快照里若混进了本机专属键（旧版本写的 / 手改的），一律忽略：
        // 应用侧与构建侧必须是同一口径，否则下载会改掉本机的日志目录。
        if !is_synced_kv_key(key) {
            continue;
        }
        tx.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )
        .map_err(|error| format!("写入配置项 {key} 失败: {error}"))?;
    }
    applied.insert("kv".to_string(), json!(snapshot.kv.len()));

    tx.commit()
        .map_err(|error| format!("提交同步应用事务失败: {error}"))?;
    Ok(Value::Object(applied))
}

fn insert_row(
    conn: &Connection,
    table: &str,
    columns: &[String],
    row: &Map<String, Value>,
) -> Result<(), String> {
    // 只取两边都有的列；顺序按当前表的列顺序（与占位符一一对应）
    let used: Vec<&String> = columns.iter().filter(|column| row.contains_key(*column)).collect();
    if used.is_empty() {
        return Ok(());
    }
    let names = used
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (1..=used.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let values: Vec<rusqlite::types::Value> = used
        .iter()
        .map(|column| json_to_sql(row.get(*column).unwrap_or(&Value::Null)))
        .collect();
    conn.execute(
        &format!(
            "INSERT INTO {} ({names}) VALUES ({placeholders})",
            quote_identifier(table)
        ),
        rusqlite::params_from_iter(values),
    )
    .map(|_| ())
    .map_err(|error| format!("写入 {table} 失败: {error}"))
}

fn json_to_sql(value: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as SqlValue;
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(flag) => SqlValue::Integer(i64::from(*flag)),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                SqlValue::Integer(integer)
            } else if let Some(real) = number.as_f64() {
                SqlValue::Real(real)
            } else {
                SqlValue::Null
            }
        }
        Value::String(text) => SqlValue::Text(text.clone()),
        // 数组 / 对象不是这三张表的列形态；真出现就序列化成文本，而不是静默丢值
        other => SqlValue::Text(other.to_string()),
    }
}

// ─── 安全备份 ─────────────────────────────────────────────────

/// 应用前把当前业务快照写到 `{config_dir}/backups/` 下，返回文件名。
///
/// 与整库恢复的 `safety_backup` 同一职责：替换是破坏性的，用户后悔时要有
/// 一份可读的留影。这里存的是**同一份 JSON 快照**（不是 SQL）—— 云同步只会
/// 改业务数据，回滚也该只涉及业务数据；用 SQL 整库形态反而会让人以为能拿它
/// 恢复日志。
pub(crate) fn safety_backup(snapshot: &Snapshot, config_dir: &std::path::Path) -> Result<String, String> {
    let dir = config_dir.join("backups");
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建备份目录失败: {error}"))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = format!("cloud-sync-{stamp}.json");
    let target = dir.join(&name);
    let bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|error| format!("序列化安全备份失败: {error}"))?;
    // 先写临时文件再改名：中途失败不会留下半份 JSON 被误当成可用的备份
    let tmp = target.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|error| format!("写入安全备份失败: {error}"))?;
    std::fs::rename(&tmp, &target).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("保存安全备份失败: {error}")
    })?;
    Ok(name)
}

// ─── manifest 组装 ───────────────────────────────────────────

/// 由快照字节与（可选的）密钥文件字节组装 manifest。
pub(crate) fn build_manifest(
    snapshot_bytes: &[u8],
    key_bytes: Option<&[u8]>,
    key_fingerprint: Option<String>,
) -> SyncManifest {
    let mut artifacts = BTreeMap::new();
    artifacts.insert(
        super::manifest::REMOTE_SNAPSHOT.to_string(),
        ArtifactMeta {
            sha256: super::sha256_hex(snapshot_bytes),
            size: snapshot_bytes.len() as u64,
        },
    );
    if let Some(bytes) = key_bytes {
        artifacts.insert(
            super::manifest::REMOTE_KEY_FILE.to_string(),
            ArtifactMeta {
                sha256: super::sha256_hex(bytes),
                size: bytes.len() as u64,
            },
        );
    }
    SyncManifest {
        format: super::manifest::PROTOCOL_FORMAT.to_string(),
        version: super::manifest::PROTOCOL_VERSION,
        app_version: crate::server::core::update::CURRENT_VERSION.to_string(),
        schema_version: SCHEMA_VERSION,
        device_name: super::manifest::detect_device_name(),
        created_at: chrono::Local::now().to_rfc3339(),
        snapshot_id: super::manifest::snapshot_id_of(&artifacts),
        key_fingerprint,
        artifacts,
    }
}

/// 读一个 kv 键的原始值（调试 / 展示用）
#[allow(dead_code)]
pub(crate) fn kv_value(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| row.get(0))
        .optional()
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_key_filter_excludes_machine_local_state() {
        // 配置项：同步
        assert!(is_synced_kv_key("apiKeys"));
        assert!(is_synced_kv_key("customProviders"));
        assert!(is_synced_kv_key("promptMode"));
        // 本机绝对路径：不同步
        assert!(!is_synced_kv_key("logDir"));
        assert!(!is_synced_kv_key("kernelDir"));
        // 另一把密钥加密：不同步
        assert!(!is_synced_kv_key("githubToken"));
        // 保留键：白名单里的同步，其余不同步
        assert!(is_synced_kv_key("agentProfiles"));
        assert!(is_synced_kv_key("proxyPool"));
        assert!(!is_synced_kv_key("desktopSettings"));
        assert!(!is_synced_kv_key("panelTokens"));
        assert!(!is_synced_kv_key("cloudSync"));
        assert!(!is_synced_kv_key("backgroundTaskState"));
    }

    #[test]
    fn apply_rejects_newer_schema() {
        let snapshot = Snapshot {
            format: SNAPSHOT_FORMAT.to_string(),
            version: SNAPSHOT_VERSION,
            schema_version: SCHEMA_VERSION + 1,
            exported_at: 0,
            tables: BTreeMap::new(),
            kv: BTreeMap::new(),
        };
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("cloud-sync-newer-schema");
        assert!(apply(&db, &snapshot).is_err());
    }
}
