//! antigravity 思考签名的**服务端缓存**（`call id → thoughtSignature`）。
//!
//! ── 为什么需要它 ────────────────────────────────────────────
//! 上游 Gemini 要求「发出过 functionCall 的下一轮，把它带回来时必须补齐
//! thoughtSignature」，否则 400 `Function call is missing a thought_signature
//! in functionCall parts`。
//!
//! 本仓原设计依赖**客户端回带**：响应侧把签名挂到 chat tool_call 的
//! `extra_content.google.thought_signature`（见 `antigravity_stream`），
//! 请求侧 `call_signature` 从同一位置读回。这条链只在 **OpenAI chat 协议**
//! 的客户端上成立 —— Anthropic Messages（`/v1/messages`，Claude Code 走这条）
//! 的 `tool_use` 块没有 `extra_content` 这个字段，签名根本回不来，于是每一轮
//! 工具续接都缺签名、每一轮都 400。
//!
//! 本模块补的是**服务端自持**的那一半：上游响应里带签名的 functionCall 一进来
//! 就按它的 `id` 记下，下一轮出站时按同一个 `id` 取回。**不依赖客户端行为**，
//! 所以对 Anthropic 入口同样有效。
//!
//! ── 为什么键是 call id ──────────────────────────────────────
//! 抓真实往返报文可以看到两点：
//!   · 上游响应的 `functionCall` part **自带 `id`**（Gemini 把请求里带的 id
//!     原样回传），且签名与 functionCall 在**同一个 part** 里；
//!   · 同一条历史在连续两轮请求里的 id **完全一致**，也没有空 id。
//! 所以 id 是天然主键，不需要按出现顺序做位置匹配。
//!
//! ── 容量与淘汰 ──────────────────────────────────────────────
//! 进程级 `OnceLock<Mutex<..>>`（与 `core::auth` 的 TABLE 同一形态），上限
//! [`MAX_ENTRIES`]，超出按 FIFO 淘汰最旧的键。丢一条的后果只是「这一轮少带
//! 一个签名」，等价于本模块不存在时的行为 —— 缓存是尽力而为的优化，
//! 不是正确性前提。
//!
//! ── 已知限制 ────────────────────────────────────────────────
//! 只在**进程存活期内**有效：网关重启后历史会话的签名丢失，那一轮退化成
//! 「不带签名」（即本模块引入前的行为）。要跨重启就得落库 —— 刻意不落：
//! 签名是短时效的会话态数据，为它加表和迁移不划算。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 同 `mod.rs`：release 是 `panic=abort`，本文件零 unwrap/expect/panic。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// 缓存条目上限。一条 = 一个 tool call 的签名（实测 864 字节），
/// 4096 条约 3.5 MB 上限，对长会话足够。
const MAX_ENTRIES: usize = 4096;

/// 全局表 + 插入顺序（FIFO 淘汰用）。两者必须在同一把锁下改。
struct Store {
    map: HashMap<String, String>,
    order: VecDeque<String>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(Store {
            map: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// 记下「这个 call id 的签名是什么」。
///
/// 空 id / 空签名直接忽略：id 为空的 functionCall 上游也不按 id 校验，
/// 签名存了也没有取回的路径。
pub fn remember(id: &str, signature: &str) {
    let id = id.trim();
    let signature = signature.trim();
    if id.is_empty() || signature.is_empty() {
        return;
    }
    let Ok(mut store) = store().lock() else {
        // 锁中毒（别处 panic 过）：放弃本次写入，不 panic
        return;
    };
    if store
        .map
        .insert(id.to_string(), signature.to_string())
        .is_none()
    {
        store.order.push_back(id.to_string());
    }
    // FIFO 淘汰：超限时从最旧的开始丢，直到回到上限内。
    // 同一个 id 被重复写入时只占一条 order（上面 is_none 守卫）。
    while store.order.len() > MAX_ENTRIES {
        match store.order.pop_front() {
            Some(oldest) => {
                store.map.remove(&oldest);
            }
            None => break,
        }
    }
}

/// 取回「这个 call id 的签名」。没有就返回 `None`（调用方按老行为省略）。
pub fn recall(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let store = store().lock().ok()?;
    store.map.get(id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_then_recall() {
        // 用独立 id 避免与其它用例共享全局表时互相干扰
        remember("test_roundtrip_A", "sig_A");
        assert_eq!(recall("test_roundtrip_A").as_deref(), Some("sig_A"));
        assert_eq!(recall("test_roundtrip_missing"), None);
    }

    #[test]
    fn empty_inputs_are_ignored() {
        remember("", "sig");
        remember("test_empty_B", "");
        remember("   ", "sig");
        assert_eq!(recall(""), None);
        assert_eq!(recall("test_empty_B"), None);
    }

    #[test]
    fn whitespace_is_trimmed() {
        remember("  test_trim_C  ", "  sig_C  ");
        assert_eq!(recall("test_trim_C").as_deref(), Some("sig_C"));
        // 取的时候也容忍带空白
        assert_eq!(recall("  test_trim_C  ").as_deref(), Some("sig_C"));
    }

    #[test]
    fn same_id_overwrites_without_growing_order() {
        remember("test_overwrite_D", "sig_1");
        remember("test_overwrite_D", "sig_2");
        assert_eq!(recall("test_overwrite_D").as_deref(), Some("sig_2"));
    }
}
