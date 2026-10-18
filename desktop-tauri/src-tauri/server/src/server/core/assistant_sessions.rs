//! AI 助手的对话会话存储（`assistant_sessions` 表，schema v16）。
//!
//! ── 解决什么问题 ────────────────────────────────────────────
//! 改造前助手是「一次性对话」：关掉页面（或点「新对话」）之后那串消息就没了，
//! 没有任何地方能翻回上一次聊了什么。会话列表要的是**可翻阅、可命名、可删除**
//! 的一列对话 —— 这张表就是那个落点。
//!
//! ── 存整条消息流而不是逐条消息一行 ──────────────────────────
//! 读写单元天然是「一整个会话」（打开 = 取全部、发一句 = 存全部），逐条存只会
//! 让每次保存变成 N 次写入 + 一次事务，而没有任何一处需要「按单条消息查」。
//! 会话量级是几十条、每条几十到几百条消息，整份 JSON 进出没有性能问题。
//!
//! ── 为什么 id 由前端生成 ────────────────────────────────────
//! 新建会话时前端立刻要用这个 id 挂消息。让后端自增/生成的话，「点新建 → 立刻
//! 发第一句」之间会多一个必须等待的往返；而前端生成（`crypto.randomUUID()`）
//! 让这两步在同一次渲染里完成，保存时按 id upsert 即可。
//!
//! ── 写失败不报错（与 `checkin_records` 同一取舍）────────────
//! 保存发生在每轮回答之后，那时用户已经看到了内容 —— 落库失败不该让那次对话
//! 报错（那会把人引到「是不是模型挂了」的方向）。失败打一行 **verbose** 日志，
//! 界面上的表现是「这次会话没进历史」，下次保存会再试。
//!
//! ── 硬约束：持锁（连接锁）期间不打日志 ──────────────────────
//! 与 `core::usage_records` / `core::checkin_records` 同一条：`Db::with*` 拿的是
//! 全局唯一的连接锁，`logging::log` 要往同一个库写 `logs` 表 —— 闭包里打日志
//! 会当场死锁。本模块的所有日志都在 `with` 返回之后打。

use std::sync::OnceLock;

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::server::db::Db;
use crate::server::logging;

static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 标题长度上限（字符）。侧边栏一行放不下更长的；超出的部分在写入侧就裁掉，
/// 不留一条读回来还得再截一次的脏数据。
const MAX_TITLE_CHARS: usize = 120;
/// 会话 id 的长度上限。前端给的是 UUID（36 字符），留足余量但不接受任意长串
/// （那是拿管理接口当文件存储用）。
const MAX_ID_CHARS: usize = 128;
/// 单条会话消息流的**字符**上限。
///
/// 不是防用户，是防「一条会话聊了几个月、每次保存都把几 MB 的 JSON 写一遍」：
/// 8 MB 已经远超一场正常长会话（每条消息按 2 KB 算 = 4000 条），到这个量级
/// 更该做的是开新会话，而不是继续往同一条里堆。
const MAX_MESSAGES_CHARS: usize = 8 * 1024 * 1024;

pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn database() -> Option<&'static Db> {
    DB.get().and_then(Option::as_ref)
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect()
}

/// 一条会话的公开形态（侧边栏 / 打开会话都读它）。
///
/// `messages` 只在「打开某一条」时给全量；列表接口给的是**不带消息体**的摘要
/// （见 [`list`] 的说明）—— 侧边栏只需要 id / 标题 / 时间，把几十条会话的
/// 全部消息都拉一遍是纯浪费。
fn row_to_summary(id: &str, title: &str, model: Option<String>, reasoning: Option<String>, created_at: i64, updated_at: i64) -> Value {
    json!({
        "id": id,
        "title": title,
        "model": model,
        "reasoning": reasoning,
        "createdAt": created_at,
        "updatedAt": updated_at,
    })
}

/// 全部会话的**摘要**（按 `updated_at` 倒序 —— 最近聊过的在最上面）。
///
/// 刻意不带 `messages`：侧边栏只用得到标题与时间。会话全量在几十条量级，
/// 一次全取、前端自己排/筛都够用，不做分页（那会让「列表」这个动作多一层
/// 状态，而收益为零）。
pub fn list() -> Vec<Value> {
    let Some(db) = database() else { return Vec::new() };
    // 闭包的返回类型是 `T`（不是 `Option<T>` / `Result<_, _>`），所以这里不能
    // 用 `?` —— prepare / query_map 的失败都用显式 match 收敛成「空表」。
    let rows = db.with(|conn| {
        let Ok(mut statement) = conn.prepare(
            "SELECT id, title, model, reasoning, created_at, updated_at
             FROM assistant_sessions
             ORDER BY updated_at DESC",
        ) else {
            return Vec::new();
        };
        let Ok(mapped) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        }) else {
            return Vec::new();
        };
        mapped.filter_map(Result::ok).collect()
    });
    let Some(rows) = rows else {
        logging::verbose("[Assistant]", "⚠️  读取会话列表失败");
        return Vec::new();
    };
    rows.into_iter()
        .map(|(id, title, model, reasoning, created_at, updated_at)| {
            row_to_summary(&id, &title, model, reasoning, created_at, updated_at)
        })
        .collect()
}

/// 取一条会话（含 `messages` 全量）。不存在返回 `None`。
pub fn get(id: &str) -> Option<Value> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let db = database()?;
    // `Db::with` 给 `Option<T>`（拿不到连接锁 = None），`optional()` 再包一层
    // `Option`（行不存在），`query_row` 的 `Result` 第三层 —— 三层都要解开，
    // 用 `?` 串起来读不出来「是哪一层空」，所以显式 match。
    let queried = db.with(|conn| {
        conn.query_row(
            "SELECT id, title, messages, model, reasoning, created_at, updated_at
             FROM assistant_sessions WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()
    });
    let Some(Ok(Some(row))) = queried else { return None };
    let (id, title, messages, model, reasoning, created_at, updated_at) = row;    let parsed: Value = serde_json::from_str(&messages).unwrap_or_else(|_| Value::Array(Vec::new()));
    let mut value = row_to_summary(&id, &title, model, reasoning, created_at, updated_at);
    value["messages"] = parsed;
    Some(value)
}

/// 保存（新建或覆盖）一条会话，返回写入后的摘要。
///
/// ── 校验为什么这么写 ────────────────────────────────────────
///   - `id` 必须有（见模块头「为什么 id 由前端生成」）；
///   - `messages` 必须是数组：它是发给上游的 wire 形态，形状错了下游会用一次
///     400 才告诉你；
///   - 标题按字符截断而不是拒绝：标题是前端从第一条消息里截出来的，超长是
///     正常的（用户第一句写了一大段），为此报错没有意义。
///
/// `createdAt` 只在**首次插入**时落定：覆盖已有会话时保留原来的创建时刻
/// （否则「按创建时间」这类排序会随每次保存漂移），`updatedAt` 每次都刷新。
pub fn save(payload: &Value) -> Result<Value, String> {
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if id.is_empty() {
        return Err("缺少会话 id".to_string());
    }
    if id.chars().count() > MAX_ID_CHARS {
        return Err("会话 id 过长".to_string());
    }
    let messages = payload.get("messages").cloned().unwrap_or_else(|| json!([]));
    if !messages.is_array() {
        return Err("messages 必须是数组".to_string());
    }
    let messages_text = serde_json::to_string(&messages)
        .map_err(|error| format!("messages 序列化失败：{error}"))?;
    if messages_text.chars().count() > MAX_MESSAGES_CHARS {
        return Err("这条会话的消息量过大，请开一条新会话".to_string());
    }
    let title = truncate_chars(
        payload.get("title").and_then(Value::as_str).map(str::trim).unwrap_or(""),
        MAX_TITLE_CHARS,
    );
    let model = payload.get("model").and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let reasoning = payload.get("reasoning").and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let now = logging::now_ms();

    let Some(db) = database() else {
        return Err("会话存储不可用".to_string());
    };
    // 一条 UPSERT 同时覆盖「新建」与「保存」：`created_at` 用 `COALESCE` 保住
    // 首次插入的值（冲突分支里 `excluded.created_at` 是本次算的 now，不能直接用）。
    let written = db.with(|conn| {
        conn.execute(
            "INSERT INTO assistant_sessions (id, title, messages, model, reasoning, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title,
               messages = excluded.messages,
               model = excluded.model,
               reasoning = excluded.reasoning,
               updated_at = excluded.updated_at",
            params![id, title, messages_text, model, reasoning, now],
        )
        .is_ok()
    });
    if written != Some(true) {
        return Err("会话写入失败".to_string());
    }
    // 回读 `created_at`：UPSERT 的冲突分支不覆盖它，所以已有会话读到的是原来的
    // 创建时刻、新会话读到的是刚写的 now。三层 `Option`（连接锁 / 行 / 列）里
    // 任何一层缺失都按 now 兜底 —— 这里只是给响应拼一个展示字段，不值得报错。
    let created_at = match db.with(|conn| {
        conn.query_row(
            "SELECT created_at FROM assistant_sessions WHERE id = ?1",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
    }) {
        Some(Ok(Some(value))) => value,
        _ => now,
    };
    Ok(row_to_summary(id, &title, model.map(str::to_string), reasoning.map(str::to_string), created_at, now))
}

/// 删除一条会话。返回是否真的删掉了（不存在 = `false`，调用方据此给 404）。
pub fn remove(id: &str) -> bool {
    let id = id.trim();
    if id.is_empty() {
        return false;
    }
    let Some(db) = database() else { return false };
    db.with(|conn| {
        conn.execute("DELETE FROM assistant_sessions WHERE id = ?1", params![id])
            .map(|count| count > 0)
            .unwrap_or(false)
    })
    .unwrap_or(false)
}

/// 清空全部会话，返回删掉的行数。
pub fn clear() -> usize {
    let Some(db) = database() else { return 0 };
    db.with(|conn| {
        conn.execute("DELETE FROM assistant_sessions", [])
            .unwrap_or(0)
    })
    .unwrap_or(0)
}
