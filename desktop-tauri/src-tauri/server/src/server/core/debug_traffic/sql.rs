//! `debug_traffic` 表的**行级 SQL 访问层** —— 本模块里唯一出现 SQL 的地方。
//!
//! ── 为什么单独一层 ──────────────────────────────────────────
//! 改造前的写入是「内存 `VecDeque` 快照 + 追加写 / 超限整文件重写」，读取是
//! 「在内存快照里倒着找」。进数据库后那套双份状态整体消失：写入是一条 UPSERT、
//! 淘汰是一条 DELETE、读取是一次主键查询 —— 本文件承接这些语句，上层
//! （`debug_traffic.rs`）只做采集、脱敏、降级与 API 形状。
//!
//! ── `size` 列 ────────────────────────────────────────────
//! 保存完整报文落库后的文本字节合计，不含 SQLite 页与行开销。
//! 留存同时按条数与该字段的合计值裁剪，避免大报文堆到近 1 GiB。
//!
//! ── 排序键：`ts DESC, rowid DESC` ────────────────────────────
//! 「最新」按 **ts** 判（`idx_debug_traffic_ts` 正为此存在），同 ts 时用 `rowid`
//! 兜底让结果确定。这与改造前「按插入顺序（`VecDeque` 的 push 序）淘汰」在
//! **慢请求**上会不同：`entry.ts` 是采集**开始**时刻，而插入发生在响应**结束**
//! 时刻 —— 一个慢 SSE（早开始、晚结束）在旧实现里排在最年轻的一端，在这里会
//! 按 ts 排到较老的一端。选 ts 的理由：它才是「这条报文属于哪次请求」的时间，
//! 也是详情与列表展示用的时间轴；按插入顺序淘汰会让「最新完成的那个请求」
//! 反而先被丢掉。列/索引都属于既有 schema，本切片不改表结构。
//!
//! ── 并发：本层不加锁 ────────────────────────────────────────
//! 所有函数取裸 `&Connection`，串行化由 `Db` 那把 Mutex 负责（上层每个操作都在
//! **一次** `Db::with` / `with_mut` 调用里跑完，多语句操作用事务包住）。
//! **硬约束**：持这把锁期间绝不能再调 `logging::log` / `logging::verbose` ——
//! 它们要写同一个库，`std::sync::Mutex` 不可重入，会当场死锁（本层所有函数
//! 都不应自己打日志，错误一律 `Err` 交回上层处理）。

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, Connection};

use super::{max_entries, TrafficEntry, MAX_TOTAL_BYTES};

/// 全部列（所有 SELECT 都按这个顺序取，[`decode_row`] 依赖它；
/// 不写 `SELECT *` 是为了「加列时读侧要显式跟上」这件事在 diff 里可见）
const COLUMNS: &str = "id, ts, url, provider, request_headers, request_body, status, \
     response_headers, response_body, truncated, size";

/// 一条记录落库后占的文本字节合计（写进 `size` 列，见模块头）
pub(super) fn payload_bytes(entry: &TrafficEntry) -> i64 {
    let headers = entry.request_headers.to_string().len();
    let body = entry.request_body.to_string().len();
    let response_headers = entry.response_headers.as_ref().map(|item| item.to_string().len()).unwrap_or(0);
    let response_body = entry.response_body.as_deref().map(str::len).unwrap_or(0);
    let identity = entry.id.len() + entry.url.len() + entry.provider.len();
    (headers + body + response_headers + response_body + identity) as i64
}

/// 行 → `TrafficEntry`。
///
/// 三个可空列（`status` / `response_headers` / `response_body`）原样读成 `Option`
/// —— **不把 NULL 折成空串**：前端的详情弹窗靠这个区分「这一侧没采到」与
/// 「采到了但内容为空」（`ui/requests-panel.js` 的 `renderDetail`：状态码 `null`
/// 显示 `-`，响应体为空则不渲染那一块）。写入侧同理（见 [`encode`]）。
/// `request_headers` / `request_body` 是 `NOT NULL`，但为防手改库留下 NULL，
/// 这里用 `Option` 读再回落默认值 —— 读侧不该因为一个脏值让整条查询失败。
fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrafficEntry> {
    let headers: Option<String> = row.get(4)?;
    let body: Option<String> = row.get(5)?;
    let status: Option<i64> = row.get(6)?;
    let response_headers: Option<String> = row.get(7)?;
    let truncated: i64 = row.get(9)?;
    Ok(TrafficEntry {
        id: row.get(0)?,
        ts: row.get(1)?,
        url: row.get(2)?,
        provider: row.get(3)?,
        request_headers: headers
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or(serde_json::Value::Null),
        request_body: body
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or(serde_json::Value::Null),
        // 状态码越界（手改库）按「没采到」处理：`u16` 装不下的值没有意义
        status: status.and_then(|value| u16::try_from(value).ok()),
        response_headers: response_headers
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok()),
        response_body: row.get(8)?,
        truncated: truncated != 0,
    })
}

/// JSON 值 → 文本列（序列化失败给 `'null'`／`'{}'`，与 DDL 的默认值同形）
fn encode_json(value: &serde_json::Value, fallback: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| fallback.to_string())
}

/// 写入用的列绑定（`insert_entry` 与 `upsert_entry` 共用一份，保证两条路径写出的
/// 列值完全一致 —— 迁移进来的行与运行期写入的行不该有任何差别）
fn encode(entry: &TrafficEntry) -> [SqlValue; 11] {
    [
        SqlValue::Text(entry.id.clone()),
        SqlValue::Integer(entry.ts),
        SqlValue::Text(entry.url.clone()),
        SqlValue::Text(entry.provider.clone()),
        SqlValue::Text(encode_json(&entry.request_headers, "{}")),
        SqlValue::Text(encode_json(&entry.request_body, "null")),
        // 三个可空列：None → NULL（**不写空串**，见 `decode_row` 的说明）
        match entry.status {
            Some(value) => SqlValue::Integer(i64::from(value)),
            None => SqlValue::Null,
        },
        match entry.response_headers.as_ref() {
            Some(value) => SqlValue::Text(encode_json(value, "null")),
            None => SqlValue::Null,
        },
        match entry.response_body.as_ref() {
            Some(value) => SqlValue::Text(value.clone()),
            None => SqlValue::Null,
        },
        SqlValue::Integer(i64::from(entry.truncated)),
        SqlValue::Integer(payload_bytes(entry)),
    ]
}

/// 插入一条（**纯 INSERT**：主键冲突即报错）。
///
/// ⚠️ **迁移项已不再用它**（改用 [`insert_entry_ignore`]）：T11 把迁移改成
/// 用户点「升级」触发之后，「重复点击」成了常态，而纯 INSERT 会在撞主键时
/// 让整批回滚 —— 用户看到「迁移失败」，原因却只是「导过了」。
/// 保留它是为了「同 id 两行 = 坏数据」这类**未来可能需要严格模式**的场合；
/// 当前无调用方（`#[allow(dead_code)]` 与 `upsert_entry` 同理：
/// 它是这张表写入语义的第三种形态，删掉会让「三种语义」这件事在代码里消失）。
#[allow(dead_code)]
pub(super) fn insert_entry(conn: &Connection, entry: &TrafficEntry) -> rusqlite::Result<()> {
    let binds = encode(entry);
    conn.execute(
        "INSERT INTO debug_traffic (id, ts, url, provider, request_headers, request_body, \
         status, response_headers, response_body, truncated, size) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params_from_iter(binds.iter()),
    )?;
    Ok(())
}

/// 插入一条，**主键已存在时跳过**（迁移项用）。
///
/// 返回 `true` = 真的插进去了，`false` = 已存在而跳过 —— 调用方据此统计
/// 「本次点击导入了多少、跳过多少」（重复点击时全是 `false`）。
///
/// 与 [`upsert_entry`] 的区别是**冲突时不动库里那份**：旧文件是历史快照，
/// 而库里那份可能更新（重试路径重采、调试开关中途打开后重采）—— 迁移不该
/// 反过来盖掉更新的数据。
pub(super) fn insert_entry_ignore(
    conn: &Connection,
    entry: &TrafficEntry,
) -> rusqlite::Result<bool> {
    let binds = encode(entry);
    let changed = conn.execute(
        "INSERT OR IGNORE INTO debug_traffic (id, ts, url, provider, request_headers, \
         request_body, status, response_headers, response_body, truncated, size) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params_from_iter(binds.iter()),
    )?;
    Ok(changed > 0)
}

/// 落一条（同 id 覆盖）。
///
/// 为什么不是纯 INSERT：`id` 是主键，而同一个 id 写第二次是可达的
/// （重试路径的采集器重置、调试开关中途打开、同一请求被两条采集路径各写一次）。
/// 改造前是追加一行 JSONL（同 id 两行，`get` 取最后那条），这里收敛成一行 ——
/// 读取侧语义不变（`get` 仍拿到最后写入的那份），表里不再留重复行。
pub(super) fn upsert_entry(conn: &Connection, entry: &TrafficEntry) -> rusqlite::Result<()> {
    let binds = encode(entry);
    conn.execute(
        "INSERT INTO debug_traffic (id, ts, url, provider, request_headers, request_body, \
         status, response_headers, response_body, truncated, size) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT(id) DO UPDATE SET ts = excluded.ts, url = excluded.url, \
         provider = excluded.provider, request_headers = excluded.request_headers, \
         request_body = excluded.request_body, status = excluded.status, \
         response_headers = excluded.response_headers, response_body = excluded.response_body, \
         truncated = excluded.truncated, size = excluded.size",
        rusqlite::params_from_iter(binds.iter()),
    )?;
    Ok(())
}

/// 存储概况：条数与已保存文本字节合计。
pub(super) fn count_and_bytes(conn: &Connection) -> rusqlite::Result<(usize, i64)> {
    let (count, bytes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM debug_traffic",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((count.max(0) as usize, bytes.max(0)))
}

/// 按 id 取一条（同 id 多行时取 ts 最大的，再以 rowid 兜底 —— 与改造前
/// 「内存里从尾部往前找第一个匹配」等价）
pub(super) fn select_one(conn: &Connection, id: &str) -> rusqlite::Result<Option<TrafficEntry>> {
    let sql = format!("SELECT {COLUMNS} FROM debug_traffic WHERE id = ?1 ORDER BY ts DESC, rowid DESC LIMIT 1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(decode_row(row)?)),
        None => Ok(None),
    }
}

/// 按留存条数与正文合计字节双重上限淘汰旧记录，返回是否真的删除了行
pub(super) fn enforce_limits(conn: &Connection) -> rusqlite::Result<bool> {
    let (count, bytes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM debug_traffic",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut removed = false;
    if bytes > MAX_TOTAL_BYTES {
        // 按实际累计大小删除最旧记录：先算出总超出的字节，再按最新记录
        // 从后向前累计，保留不超过上限的最长尾部集合
        removed = conn.execute(
            "DELETE FROM debug_traffic WHERE id NOT IN (\
               SELECT id FROM (\
                 SELECT id, SUM(size) OVER (\
                   ORDER BY ts DESC, rowid DESC ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW\
                 ) AS cumulative FROM debug_traffic\
               ) WHERE cumulative <= ?1\
             )",
            params![MAX_TOTAL_BYTES],
        )? > 0;
    }
    let limit = max_entries() as i64;
    if count > limit {
        removed |= conn.execute(
            "DELETE FROM debug_traffic WHERE id NOT IN \
             (SELECT id FROM debug_traffic ORDER BY ts DESC, rowid DESC LIMIT ?1)",
            params![limit],
        )? > 0;
    }
    Ok(removed)
}

/// 新库启用增量回收时释放少量空闲页
///
/// 必须在事务提交后调用，SQLite 不允许在事务内执行回收
pub(super) fn incremental_vacuum(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA incremental_vacuum(1024);")
}
