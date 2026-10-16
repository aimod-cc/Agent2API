//! Antigravity 思考签名（`thoughtSignature`）的**服务端缓存**。
//!
//! ── 为什么需要缓存（issue #291）──────────────────────────────
//! 上游把历史回放里的 `functionCall` 签名当**硬性要求**：不带直接 400（实测
//! 文案「Function call is missing a thought_signature in functionCall parts」）；
//! 带一个伪造的同样 400（「Corrupted thought signature」—— 签名本体被校验，
//! 「发明默认签名 / 哨兵值」的路走不通，Manager 的
//! `skip_thought_signature_validator` 字面量本仓拿不到，不猜）。
//! 网关把签名放在 `tool_calls[].extra_content.google.thought_signature`
//! （Google 自家 OpenAI 兼容端点的字段位置）随响应带出 —— 但那是非标字段，
//! 多数 OpenAI 兼容客户端下一轮会把它丢掉，于是「模型调工具 → 客户端回传
//! 结果」的多轮循环第二轮必 400；官方客户端没事，签名由它自己管理。
//!
//! 两套参考的解法都是**服务端**缓存签名、历史不带时回填（Manager 按 call id、
//! 还打哨兵；9router 按 call id / 会话）。本仓此前刻意不做跨请求状态缓存，
//! 那个决定建立在「上游不强制」的前提上；前提变了（上游 400），本模块就是
//! 补上的那份状态。写读两侧：
//!   - **写**（响应侧）：`antigravity_stream` 的 `tool_call_frames` —— 流式与
//!     非流式客户端共用那一台状态机，一处挂钩两路都覆盖；
//!   - **读**（请求侧）：`antigravity_outbound` 的信封组装 —— 客户端带回的
//!     签名仍最优先（那是最忠实的一份），缓存只是客户端没带时的回填。
//!
//! ── 两档键 ──────────────────────────────────────────────────
//!   1. `call_id → signature`（主键）：上游给 functionCall 的 id，OpenAI 形态
//!      的客户端必须原样引用（`tool_calls[].id` / `tool_call_id` 要配对），
//!      回放历史里就在手上 —— 绝大多数客户端命中这一档；
//!   2. `sessionId + 函数名 → signature`（兜底）：个别框架会重排 / 重生成 id，
//!      那时按会话键兜。sessionId 是信封里的稳定会话键（派生法见
//!      `antigravity_outbound::session_id`，写入侧从同一份信封带出，两边的键
//!      必然同源）。签名校验若绑定到「这一次调用」而非「会话」，这一档可能仍
//!      被上游拒 —— 那就退回 400（与没有缓存时同状），不会更糟。
//!
//! ── 容量与生命周期：两代（two-generation）有界缓存 ──────────
//! 写入只进当代；当代写满（[`CAPACITY`] 条）就整体转正为前代、当代清空重来；
//! 读先查当代再查前代，**命中前代不搬回当代**（搬家要可变借用，读路径就得
//! 加写锁 —— 不值得，热键在下一轮轮转里自然存活）。最坏常驻 2×CAPACITY 条
//! （一条约 1KB 的签名 + 键，上界 ~4MB）。不引 `lru` 依赖：本项目「零外部
//! 依赖的本地单机网关」定位，为一个缓存引 crate 不合算，两代缓存的语义对
//! 「最近对话的工具签名」这个访问模式足够。全在内存：网关重启即清空 ——
//! 重启前旧会话的签名找不回来，那些会话的下一轮工具续接仍可能 400，直到
//! 拿到新的 tool_call（参考实现同为内存缓存，同一限制）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：零 unwrap/expect/panic；毒锁按「未命中 / 忽略
//! 写入」处理 —— 这份缓存可丢弃，不值得让任何一次请求失败。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// 当代容量（最坏常驻 2×CAPACITY 条，见模块头）
const CAPACITY: usize = 2048;

/// 两个「世代」的键值表：写入只进 `current`，写满整体转正为 `previous`
#[derive(Default)]
struct Generations {
    current: HashMap<String, String>,
    previous: HashMap<String, String>,
}

static CACHE: OnceLock<Mutex<Generations>> = OnceLock::new();

/// 状态句柄（毒锁按「继续用」处理：这份缓存可丢弃，见模块头）
fn cache() -> &'static Mutex<Generations> {
    CACHE.get_or_init(|| Mutex::new(Generations::default()))
}

/// 键前缀：call id 档（与会话档同表混存，前缀互不撞）
const CALL_PREFIX: &str = "c|";
/// 键前缀：会话 + 函数名档
const SESSION_PREFIX: &str = "s|";

/// 记一个「call id → 签名」（响应侧：tool_call 带签名下发时调用）。
pub fn remember_call(call_id: &str, signature: &str) {
    if call_id.is_empty() || signature.is_empty() {
        return;
    }
    insert(format!("{CALL_PREFIX}{call_id}"), signature.to_string());
}

/// 记一个「会话 + 函数名 → 签名」（响应侧兜底档；会话键拿不到时不记，
/// 空键会变成跨会话串味的全局档 —— 那不是本模块任何一档的语义）。
pub fn remember_for_session(session: &str, name: &str, signature: &str) {
    if session.is_empty() || name.is_empty() || signature.is_empty() {
        return;
    }
    insert(format!("{SESSION_PREFIX}{session}|{name}"), signature.to_string());
}

/// 按 call id 取回签名（请求侧回填的主键档）。
pub fn lookup_call(call_id: &str) -> Option<String> {
    if call_id.is_empty() {
        return None;
    }
    lookup(&format!("{CALL_PREFIX}{call_id}"))
}

/// 按「会话 + 函数名」取回签名（请求侧回填的兜底档）。
pub fn lookup_for_session(session: &str, name: &str) -> Option<String> {
    if session.is_empty() || name.is_empty() {
        return None;
    }
    lookup(&format!("{SESSION_PREFIX}{session}|{name}"))
}

/// 写入：当代满且是新键时先整体轮转（旧键若还热，前代里还有一份可读）
fn insert(key: String, value: String) {
    if let Ok(mut guard) = cache().lock() {
        if guard.current.len() >= CAPACITY && !guard.current.contains_key(&key) {
            guard.previous = std::mem::take(&mut guard.current);
        }
        guard.current.insert(key, value);
    }
    // 锁中毒（写侧 panic 过）→ 静默放弃这次写入：缓存可丢弃，不影响主流程
}

/// 读取：先当代后前代；毒锁折叠成未命中
fn lookup(key: &str) -> Option<String> {
    let guard = cache().lock().ok()?;
    guard
        .current
        .get(key)
        .or_else(|| guard.previous.get(key))
        .cloned()
}
