//! 会话粘性：trace / thread → 账号 的亲和记录（对齐 AxonHub 的实现）。
//!
//! ── 这个模块解决什么 ────────────────────────────────────────
//! 同一个对话的多轮请求若落到不同账号上，上游侧的一切「会话态」都会断：
//! prompt 缓存命中率下降、上游按账号记的上下文丢失、部分有状态上游直接
//! 报「会话不存在」。粘性记录回答的问题很小：**这条会话上一次用的是哪个
//! 账号**，选路时优先还用它。
//!
//! ── 标识从哪来（提取链照抄 AxonHub，顺序即优先级）───────────────
//!   1. 头 `AH-Trace-Id`（AxonHub 的约定，从它迁来的客户端直接可用）
//!   2. OpenCode 系头：`X-Opencode-Session` → `X-Session-Id` → `X-Session-Affinity`
//!   3. Claude Code：`POST /v1/messages` 时解析 body 的 `metadata.user_id`
//!      （v2 JSON 与 legacy 两种格式，见 [`parse_claude_code_user_id`]）
//!   4. Codex：头 `Session_id` / `Session-Id`，或头 `X-Codex-Turn-Metadata`
//!      里的 `session_id` 字段
//!   thread 单独一条：头 `AH-Thread-Id`（无 body 回退，与 AxonHub 相同）。
//!
//! 取不到 trace / thread 就**完全没有粘性**（普通选路），这是设计而非降级：
//! 凭空编一个会话 id 会让互不相干的请求互相粘住。
//!
//! ── 头值必须是 ASCII（2026-10-10 实测补充）─────────────────────
//! 头值里出现非 ASCII 字符（中文、emoji…）时 `HeaderValue::to_str` 失败，
//! 该头被当作「没有」—— 粘性随之静默失效。这是 HTTP 规范的限制（头值是
//! ISO-8859-1 字节串），不是本模块的选择：真实客户端发的 trace 都是
//! UUID 或十六进制串，不会踩到。**手工用 curl 测粘性时要留意**：
//! `-H "AH-Trace-Id: 中文值"` 发出去的请求不带可用标识，看起来就像
//! 「粘性不生效」。
//!
//! ── 记录怎么存（与 AxonHub 的取舍一致）─────────────────────────
//!   - **内存 Map**（本应用是单进程网关，AxonHub 用 Redis 是为了多实例）；
//!   - **TTL 固定 30 分钟**（AxonHub 同值）：写入即续期、读取不续期 ——
//!     活跃会话每来一次请求就续 30 分钟，静默 30 分钟后亲和自动消失；
//!   - 过期判定**惰性**做（读时比对时间戳），不跑后台清理任务；
//!     写入时若表明显偏大（超过 [`PRUNE_THRESHOLD`]）顺手扫一遍死条目。
//!   - **miss 不查历史请求**（AxonHub 明确不做，这里也不做）：粘性只是
//!     一层便宜的加速，为它回查请求日志不值得。
//!
//! ── 谁写、什么时候写 ────────────────────────────────────────
//! 选路函数**选中账号、即将发送时**写（`note_selected`），失败换号选中
//! 新账号时覆盖 —— 于是亲和会自然漂移到实际可用的账号（AxonHub 的
//! `persistRequestExecution` 同款语义）。失败**不删记录**：靠 TTL 与
//! 「下次成功写入覆盖」自愈，账号的惩罚交给限流冷却与并发上限那两套机制。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::http::HeaderMap;
use serde_json::Value;

/// 粘性记录的有效期（与 AxonHub 的 30 分钟逐字一致）。
pub const STICKY_TTL_MS: i64 = 30 * 60 * 1000;

/// 惰性清理的触发阈值：条目数超过它时，写入路径顺手扫一遍死条目。
///
/// 阈值取一个「正常用不到、出事时能兜住」的量级：一个活跃用户同时进行的
/// 会话数是几十的量级，30 分钟窗口内上千条已经说明有大量一次性会话在刷
/// （每个请求都可能带一个新 trace id）。超过就扫，摊薄成本 —— 不做后台
/// 任务是因为粘性表只是加速层，为它起一条定时循环不划算。
const PRUNE_THRESHOLD: usize = 4096;

/// 一次请求的会话标识（trace 与 thread 各自可为空）。
///
/// 提取在**入口 handler** 做一次（那里才有 headers 与 body），随
/// `ForwardRequest` 带进转发层 —— 转发层不认识 HTTP，也不该为了取一个
/// 头去把 body 再解析一遍。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionKeys {
    /// 会话链标识（提取链见模块头）；空串 = 无
    pub trace: String,
    /// 会话线程标识（`AH-Thread-Id`）；空串 = 无
    pub thread: String,
}

impl SessionKeys {
    /// 两个标识都为空（无粘性可用）
    pub fn is_empty(&self) -> bool {
        self.trace.is_empty() && self.thread.is_empty()
    }
}

/// 从入站请求头提取会话标识。
///
/// `path` 用于 Claude Code 那条的端点判定：`metadata.user_id` 只在
/// `POST /v1/messages` 上解析（AxonHub 同款收窄）—— 其它协议的 body 里
/// 出现同名键是巧合，拿它当会话 id 会把不相干的请求粘到一起。
/// `body` 是**下游原始请求体**（翻译前），Claude Code 读的是它自己的字段。
pub fn session_keys_from(headers: &HeaderMap, path: &str, body: Option<&Value>) -> SessionKeys {
    SessionKeys {
        trace: trace_from(headers, path, body),
        thread: header_text(headers, "ah-thread-id"),
    }
}

/// trace 标识的提取链（顺序即优先级，见模块头）
fn trace_from(headers: &HeaderMap, path: &str, body: Option<&Value>) -> String {
    // ① AxonHub 约定头
    let explicit = header_text(headers, "ah-trace-id");
    if !explicit.is_empty() {
        return explicit;
    }
    // ② OpenCode 系（三个头按新旧顺序）
    for name in ["x-opencode-session", "x-session-id", "x-session-affinity"] {
        let value = header_text(headers, name);
        if !value.is_empty() {
            return value;
        }
    }
    // ③ Claude Code：只认 messages 端点的 metadata.user_id。
    // 用**精确相等**而不是 `ends_with`：当前路由表里只有 `/v1/messages`
    // 这一个（`/v1/messages/count_tokens` 是另一个端点，本就不该读 body），
    // 后缀匹配在将来出现 `/proxy/v1/messages` 之类的挂载路径时会误命中。
    if path == "/v1/messages" {
        if let Some(session) = body
            .and_then(|body| body.get("metadata"))
            .and_then(|metadata| metadata.get("user_id"))
            .and_then(Value::as_str)
            .and_then(parse_claude_code_user_id)
        {
            return session;
        }
    }
    // ④ Codex：会话头，或 turn metadata 里的 session_id
    for name in ["session_id", "session-id"] {
        let value = header_text(headers, name);
        if !value.is_empty() {
            return value;
        }
    }
    if let Some(metadata) = headers
        .get("x-codex-turn-metadata")
        .and_then(|value| value.to_str().ok())
    {
        if let Ok(payload) = serde_json::from_str::<Value>(metadata) {
            if let Some(session) = payload
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return session.to_string();
            }
        }
    }
    String::new()
}

/// 头值读取：缺失 / 非 ASCII 一律当「没有」（不做 URL 解码等额外处理）。
///
/// `HeaderMap::get` 本身大小写不敏感（HTTP 头名不区分大小写），所以这里
/// 用小写名查询即可 —— 与 AxonHub 的 Go 侧行为一致。
fn header_text(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_string()
}

/// 从 Claude Code 的 `metadata.user_id` 里取 `session_id`。
///
/// 两种格式（照抄 AxonHub 的 `claudecode.ParseUserID`）：
///   - **v2（>=2.1.78）**：JSON 文本
///     `{"device_id":"…","account_uuid":"…","session_id":"…"}`；
///   - **legacy**：`user_<64位十六进制>_account__session_<uuid>`。
///
/// 解析不出（别的客户端往 metadata.user_id 塞了别的东西）返回 `None` ——
/// 宁可没有粘性，也不要把一个不认识的串当会话 id。
pub fn parse_claude_code_user_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // v2：JSON 形态
    if raw.starts_with('{') {
        let payload: Value = serde_json::from_str(raw).ok()?;
        let session = payload.get("session_id").and_then(Value::as_str)?;
        let session = session.trim();
        return if session.is_empty() { None } else { Some(session.to_string()) };
    }
    // legacy：user_<64hex>_account__session_<uuid>
    let rest = raw.strip_prefix("user_")?;
    let (device, tail) = rest.split_once("_account__session_")?;
    let is_hex64 = device.len() == 64 && device.chars().all(|c| c.is_ascii_hexdigit());
    let is_uuid = is_uuid_shape(tail);
    if is_hex64 && is_uuid {
        return Some(tail.to_string());
    }
    None
}

/// `8-4-4-4-12` 的十六进制 UUID 形态（不校验版本位 —— 正则原版也没校验）
fn is_uuid_shape(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    parts.len() == lengths.len()
        && parts
            .iter()
            .zip(lengths)
            .all(|(part, len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

// ─── 记录表 ─────────────────────────────────────────────────

/// 一条记录：账号 id + 写入时刻（毫秒）
#[derive(Clone, Debug)]
struct Entry {
    account_id: String,
    written_at: i64,
}

/// 进程级记录表：`trace:<id>` / `thread:<id>` → 账号 id。
///
/// 一把 `Mutex` 包一张表：读写在选路热路径上，但临界区只是几次哈希操作
/// （纳秒级），与 `upstream::connections` 同一取舍。锁中毒不致命（取回
/// 内部值继续用）——粘性只是加速层，任何内部异常都不该让转发失败。
fn table() -> &'static Mutex<HashMap<String, Entry>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 记录本次选中的账号（trace 与 thread 各写一条；空标识跳过）。
///
/// 写入即续期（TTL 从这一刻重新计 30 分钟）。失败换号后由新账号覆盖 ——
/// 这就是「亲和漂移到实际可用账号」的全部实现。
pub fn note_selected(keys: &SessionKeys, account_id: &str, now: i64) {
    if account_id.is_empty() || keys.is_empty() {
        return;
    }
    let Ok(mut guard) = table().lock() else {
        return;
    };
    if guard.len() >= PRUNE_THRESHOLD {
        prune_expired(&mut guard, now);
    }
    let entry = Entry { account_id: account_id.to_string(), written_at: now };
    if !keys.trace.is_empty() {
        guard.insert(format!("trace:{}", keys.trace), entry.clone());
    }
    if !keys.thread.is_empty() {
        guard.insert(format!("thread:{}", keys.thread), entry);
    }
}

/// 查 trace 最近选中的账号（未命中 / 已过期返回 `None`）。
pub fn trace_account(keys: &SessionKeys, now: i64) -> Option<String> {
    lookup("trace", &keys.trace, now)
}

/// 查 thread 最近选中的账号（未命中 / 已过期返回 `None`）。
pub fn thread_account(keys: &SessionKeys, now: i64) -> Option<String> {
    lookup("thread", &keys.thread, now)
}

fn lookup(scope: &str, id: &str, now: i64) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    let guard = table().lock().ok()?;
    let entry = guard.get(&format!("{scope}:{id}"))?;
    if now - entry.written_at > STICKY_TTL_MS {
        return None;
    }
    Some(entry.account_id.clone())
}

/// 清掉所有过期条目（只在表偏大时由写入路径调用）。
fn prune_expired(table: &mut HashMap<String, Entry>, now: i64) {
    table.retain(|_, entry| now - entry.written_at <= STICKY_TTL_MS);
}

/// 当前记录条数（排障 / 测试用）。
#[cfg(test)]
pub fn len() -> usize {
    table().lock().map(|guard| guard.len()).unwrap_or(0)
}

/// 清空记录表（测试用：进程级状态在测试之间会互相污染）。
#[cfg(test)]
pub fn clear() {
    if let Ok(mut guard) = table().lock() {
        guard.clear();
    }
}

/// 测试辅助：把一个条目的写入时刻改成过去（模拟过期）
#[cfg(test)]
pub fn backdate(key: &str, written_at: i64) {
    if let Ok(mut guard) = table().lock() {
        if let Some(entry) = guard.get_mut(key) {
            entry.written_at = written_at;
        }
    }
}

/// TTL 的 `Duration` 形态（测试与文档用）
pub const STICKY_TTL: Duration = Duration::from_millis(STICKY_TTL_MS as u64);

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    /// 记录表是**进程级**的，而 cargo 默认并行跑测试：不串行化的话，
    /// 一个用例的 `clear` 会把另一个用例刚写进去的记录清掉。
    /// 这把锁只存在于测试里（生产路径没有它，也不需要）。
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::server::core::test_state_lock()
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("头名合法"),
                HeaderValue::from_str(value).expect("头值合法"),
            );
        }
        map
    }

    #[test]
    fn explicit_trace_header_wins() {
        let _guard = test_lock();
        let keys = session_keys_from(
            &headers(&[("ah-trace-id", "T1"), ("x-session-id", "T2")]),
            "/v1/chat/completions",
            None,
        );
        assert_eq!(keys.trace, "T1");
    }

    #[test]
    fn opencode_headers_in_order() {
        let _guard = test_lock();
        let keys = session_keys_from(
            &headers(&[("x-session-affinity", "A"), ("x-session-id", "B")]),
            "/v1/chat/completions",
            None,
        );
        assert_eq!(keys.trace, "B", "x-session-id 应优先于 x-session-affinity");
    }

    #[test]
    fn codex_session_header_and_turn_metadata() {
        let _guard = test_lock();
        let keys = session_keys_from(&headers(&[("Session-Id", "C1")]), "/v1/responses", None);
        assert_eq!(keys.trace, "C1");
        let keys = session_keys_from(
            &headers(&[("x-codex-turn-metadata", r#"{"session_id":"C2"}"#)]),
            "/v1/responses",
            None,
        );
        assert_eq!(keys.trace, "C2");
    }

    #[test]
    fn claude_code_user_id_v2_and_legacy() {
        let _guard = test_lock();
        let v2 = r#"{"device_id":"d","account_uuid":"a","session_id":"S2"}"#;
        assert_eq!(parse_claude_code_user_id(v2).as_deref(), Some("S2"));
        let legacy = format!(
            "user_{}_account__session_550e8400-e29b-41d4-a716-446655440000",
            "a".repeat(64)
        );
        assert_eq!(
            parse_claude_code_user_id(&legacy).as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(parse_claude_code_user_id("nonsense"), None);
    }

    #[test]
    fn claude_code_only_on_messages_endpoint() {
        let _guard = test_lock();
        let body: Value = serde_json::json!({
            "metadata": {"user_id": r#"{"session_id":"S3"}"#}
        });
        let on_messages =
            session_keys_from(&HeaderMap::new(), "/v1/messages", Some(&body));
        assert_eq!(on_messages.trace, "S3");
        let on_chat =
            session_keys_from(&HeaderMap::new(), "/v1/chat/completions", Some(&body));
        assert!(on_chat.trace.is_empty(), "其它端点不解析 metadata.user_id");
    }

    #[test]
    fn records_roundtrip_and_ttl() {
        let _guard = test_lock();
        clear();
        let keys = SessionKeys { trace: "T".into(), thread: "H".into() };
        note_selected(&keys, "acct-1", 1_000);
        assert_eq!(trace_account(&keys, 1_000).as_deref(), Some("acct-1"));
        assert_eq!(thread_account(&keys, 1_000).as_deref(), Some("acct-1"));
        // 29 分钟内仍有效
        assert_eq!(
            trace_account(&keys, 1_000 + STICKY_TTL_MS - 1).as_deref(),
            Some("acct-1")
        );
        // 超过 30 分钟失效
        assert_eq!(trace_account(&keys, 1_000 + STICKY_TTL_MS + 1), None);
        clear();
    }

    #[test]
    fn write_overwrites_previous_account() {
        let _guard = test_lock();
        clear();
        let keys = SessionKeys { trace: "T".into(), thread: String::new() };
        note_selected(&keys, "acct-1", 0);
        note_selected(&keys, "acct-2", 1);
        assert_eq!(trace_account(&keys, 1).as_deref(), Some("acct-2"));
        clear();
    }

    #[test]
    fn prune_removes_expired_only() {
        let _guard = test_lock();
        clear();
        let old = SessionKeys { trace: "OLD".into(), thread: String::new() };
        let fresh = SessionKeys { trace: "NEW".into(), thread: String::new() };
        note_selected(&old, "a", 0);
        note_selected(&fresh, "b", STICKY_TTL_MS + 10);
        // 手动触发一次清理：把表填到阈值之上不现实，直接调私有函数
        let mut map = table().lock().expect("锁可用");
        prune_expired(&mut map, STICKY_TTL_MS + 10);
        assert!(!map.contains_key("trace:OLD"));
        assert!(map.contains_key("trace:NEW"));
        drop(map);
        clear();
    }
}
