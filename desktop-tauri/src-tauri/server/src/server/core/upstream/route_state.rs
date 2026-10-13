//! 账号路由的**运行期状态**：轮转游标 / 会话粘性 / 失败退避 / 限流垫底 / 熔断。
//!
//! 对应账号页「路由」区域的五样设置在**转发时**要记的那点状态。全部是
//! **进程内存**（不落盘、不跨进程）：这些判定回答的是「接下来这一会儿请求
//! 怎么分」，重启清零是正确语义（与 magpie 的 Sink / resting 同口径）；
//! 真正要跨重启保留的读数是账号记录里的 `rateLimits`（限额冷却，已有机制）。
//!
//! ── 与选路纯函数的分工 ────────────────────────────────────────
//! `routing.rs` 只做**纯判定**（候选过滤 + 排序 + 原因句，输入即输出）；
//! 本模块是那套判定要读 / 要写的**状态**。于是：
//!   - 选路时：从本模块取一份快照（克隆小 Map），传进纯函数；
//!   - 请求有了结果时：调本模块的 mark_* 更新状态。
//! 「决策」与「状态」分开后，queue 端点（只看不写）与转发路径（看 + 写）
//! 读到的口径永远一致 —— 它们调的是同一组纯函数。
//!
//! ── 粘性键为什么是「模型 + 首条用户消息」的哈希 ────────────────
//! 网关不做会话管理（那是客户端的事），拿不到 magpie 那种 session id。
//! 但「同一个对话」在下游请求里的**稳定特征**是：模型不变、首条用户消息
//! 不变（后续轮次只是在尾部追加）。哈希它得到会话键，进程内 DefaultHasher
//! 即可（不落盘、不需要抗碰撞的密码学强度，键空间是本进程自己产生的串）。
//!
//! ── 退避与熔断的分工（两个都开时互不吞并）─────────────────────
//!   - 退避（backoff）：**单次失败**后的短期休息（15 分钟起，连续失败翻倍，
//!     上限 1 小时）。目的是「让这次失败先凉一会儿」，惩罚轻、恢复自动；
//!   - 熔断（breaker）：**连续失败到阈值**才打开，且要连续成功若干次才关。
//!     目的是「这个账号现在整体不对劲，别再一条条地试」。
//! 两者都以账号为键、都影响排序（垫底），但判定独立：退避只影响位次，
//! 熔断打开期间直接跳过（除非全场只剩它是开的 —— 那时仍按「试一次」放行，
//! 与并发挤占同一取舍：宁可打一次试试，也不把请求直接拒掉）。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// 失败退避的起步时长（15 分钟）与上限（1 小时）。
pub(super) const BACKOFF_BASE_MS: i64 = 15 * 60 * 1000;
const BACKOFF_MAX_MS: i64 = 60 * 60 * 1000;

/// 一次退避记录：休息到 `until`；`strikes` 是连续失败笔数（决定翻倍档）。
#[derive(Clone, Copy, Debug)]
struct BackoffEntry {
    until: i64,
    strikes: u32,
}

/// 一个账号的熔断状态。
#[derive(Clone, Copy, Debug, Default)]
struct BreakerEntry {
    /// 当前连续失败数（成功清零）
    failures: u32,
    /// 熔断打开到什么时候（毫秒；0 = 未打开）
    open_until: i64,
    /// 打开期过后（半开）已连续成功多少次
    half_open_successes: u32,
}

/// 全部路由运行期状态。挂在 `UpstreamService` 上（与 `Connections` 同款）。
#[derive(Default)]
pub(super) struct RouteState {
    /// 轮转游标：模型键（请求名）→ 下一个起始位。
    ///
    /// 为什么按模型分键：不同模型的可用账号集合不同，共用一个游标会让
    /// 「A 模型的第 2 位」错误地决定 B 模型从谁开始。空模型（客户端没点名）
    /// 用空串键，全局一条。
    cursors: Mutex<HashMap<String, usize>>,
    /// 会话粘性：会话键 → (账号 id, 最近一次使用时刻)
    sticky: Mutex<HashMap<String, (String, i64)>>,
    /// 失败退避：账号 id → 休息记录
    backoff: Mutex<HashMap<String, BackoffEntry>>,
    /// 限流垫底：账号 id → 沉底时刻（还有额度时被 429 的；重启前一直沉底）
    sink: Mutex<HashMap<String, i64>>,
    /// 熔断：账号 id → 状态
    breaker: Mutex<HashMap<String, BreakerEntry>>,
}

/// 粘性 / 垫底 / 退避的快照（选路纯函数的输入）。
pub(super) struct RouteSnapshot {
    /// 会话键命中的账号 id（没有 = None）
    pub sticky_hit: Option<String>,
    /// 沉底账号（值 = 沉底时刻，仅排障展示用）
    pub sunk: HashMap<String, i64>,
    /// 休息中的账号（值 = 休息截止时刻）
    pub resting: HashMap<String, i64>,
    /// 熔断打开的账号
    pub open: std::collections::HashSet<String>,
}

impl RouteState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// 会话键：模型 + 首条用户消息（截断到 4KB）的进程内哈希。
    ///
    /// 空串 = 这条请求没有可用的会话特征（没有 messages / 没有用户消息），
    /// 调用方按「不粘」处理。
    pub(super) fn conversation_key(body: &serde_json::Value) -> String {
        let model = body.get("model").and_then(serde_json::Value::as_str).unwrap_or("");
        let first_user = body
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .and_then(|messages| {
                messages.iter().find(|message| {
                    message.get("role").and_then(serde_json::Value::as_str) == Some("user")
                })
            })
            .map(|message| message.get("content").cloned().unwrap_or(serde_json::Value::Null));
        let Some(content) = first_user else {
            return String::new();
        };
        let material = match &content {
            serde_json::Value::String(text) => {
                let end = text
                    .char_indices()
                    .take_while(|(index, _)| *index < 4096)
                    .last()
                    .map(|(index, ch)| index + ch.len_utf8())
                    .unwrap_or(0);
                text[..end].to_string()
            }
            other => other.to_string(),
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        model.hash(&mut hasher);
        0x1fu8.hash(&mut hasher);
        material.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// 取粘性命中的账号（命中即刷新最近使用时刻；过期的顺手清掉）。
    pub(super) fn sticky_lookup(&self, key: &str, now: i64, ttl_ms: i64) -> Option<String> {
        if key.is_empty() || ttl_ms <= 0 {
            return None;
        }
        let mut table = self.sticky.lock().ok()?;
        // 顺手清过期项：表规模 = 活跃会话数，一次线性扫描可接受；
        // 不在后台起清扫线程 —— 网关没有「会话结束」的信号，惰性清理最省事。
        table.retain(|_, (_, last_seen)| now - *last_seen <= ttl_ms * 8);
        let entry = table.get_mut(key)?;
        if now - entry.1 > ttl_ms {
            table.remove(key);
            return None;
        }
        entry.1 = now;
        Some(entry.0.clone())
    }

    /// 绑定会话 → 账号（成功转发后调用；只对新账号改写）
    pub(super) fn sticky_bind(&self, key: &str, account_id: &str, now: i64) {
        if key.is_empty() || account_id.is_empty() {
            return;
        }
        if let Ok(mut table) = self.sticky.lock() {
            table.insert(key.to_string(), (account_id.to_string(), now));
        }
    }

    /// 取轮转游标并前进一位（返回本次的起始位）。
    ///
    /// `len` 是当前候选数：游标对 len 取模，候选集合变了也稳定。
    pub(super) fn rotate_take(&self, model_key: &str, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        let Ok(mut table) = self.cursors.lock() else {
            return 0;
        };
        let cursor = table.entry(model_key.to_string()).or_insert(0);
        let start = *cursor % len;
        *cursor = (start + 1) % len;
        start
    }

    /// 记一笔失败退避（连续失败翻倍，上限 1 小时），返回休息截止时刻。
    pub(super) fn backoff_mark(&self, account_id: &str, now: i64) -> i64 {
        let Ok(mut table) = self.backoff.lock() else {
            return 0;
        };
        let entry = table.entry(account_id.to_string()).or_insert(BackoffEntry {
            until: 0,
            strikes: 0,
        });
        entry.strikes = entry.strikes.saturating_add(1);
        let factor = 1i64 << entry.strikes.saturating_sub(1).min(3); // 1x / 2x / 4x / 8x
        let wait = (BACKOFF_BASE_MS * factor).min(BACKOFF_MAX_MS);
        entry.until = now + wait;
        entry.until
    }

    /// 成功一次 → 清退避记录（连续失败的链子断了）。
    pub(super) fn backoff_clear(&self, account_id: &str) {
        if let Ok(mut table) = self.backoff.lock() {
            table.remove(account_id);
        }
    }

    /// 记一笔垫底（还有额度时被 429 的账号）。
    pub(super) fn sink_mark(&self, account_id: &str, now: i64) {
        if let Ok(mut table) = self.sink.lock() {
            table.insert(account_id.to_string(), now);
        }
    }

    /// 成功一次 → 从垫底里放出来（它证明自己能干活）。
    pub(super) fn sink_clear(&self, account_id: &str) {
        if let Ok(mut table) = self.sink.lock() {
            table.remove(account_id);
        }
    }

    /// 记一笔失败；返回熔断是否**就此打开**（用于打日志）。
    pub(super) fn breaker_failure(
        &self,
        account_id: &str,
        threshold: i64,
        timeout_seconds: i64,
        now: i64,
    ) -> bool {
        let Ok(mut table) = self.breaker.lock() else {
            return false;
        };
        let entry = table.entry(account_id.to_string()).or_default();
        let was_open = entry.open_until > now;
        entry.failures = entry.failures.saturating_add(1);
        entry.half_open_successes = 0;
        if !was_open && entry.failures as i64 >= threshold.max(1) {
            entry.open_until = now + timeout_seconds.max(0) * 1000;
            return true;
        }
        // 半开期再失败：重新打开
        if entry.open_until > 0 && entry.open_until <= now {
            entry.open_until = now + timeout_seconds.max(0) * 1000;
            return true;
        }
        false
    }

    /// 成功一次 → 熔断的恢复计数（连续成功到阈值才真正关闭）。
    pub(super) fn breaker_success(&self, account_id: &str, success_threshold: i64) {
        let Ok(mut table) = self.breaker.lock() else {
            return;
        };
        let Some(entry) = table.get_mut(account_id) else {
            return;
        };
        entry.failures = 0;
        if entry.open_until > 0 {
            entry.half_open_successes = entry.half_open_successes.saturating_add(1);
            if entry.half_open_successes as i64 >= success_threshold.max(1) {
                table.remove(account_id);
            }
        }
    }

    /// 选路快照：粘性命中 + 沉底 / 休息 / 熔断三张表（一次取齐）。
    pub(super) fn snapshot(&self, conv_key: &str, now: i64, ttl_ms: i64) -> RouteSnapshot {
        let sticky_hit = self.sticky_lookup(conv_key, now, ttl_ms);
        let sunk = self.sink.lock().map(|table| table.clone()).unwrap_or_default();
        let resting = self
            .backoff
            .lock()
            .map(|table| {
                table
                    .iter()
                    .filter(|(_, entry)| entry.until > now)
                    .map(|(id, entry)| (id.clone(), entry.until))
                    .collect()
            })
            .unwrap_or_default();
        let open = self
            .breaker
            .lock()
            .map(|table| {
                table
                    .iter()
                    .filter(|(_, entry)| entry.open_until > now)
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default();
        RouteSnapshot {
            sticky_hit,
            sunk,
            resting,
            open,
        }
    }
}
