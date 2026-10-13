//! CodeArts 自带的**尺寸门**：按真值记一条「这一家至少多少字节发不出去」。
//!
//! ── 为什么要有这道门 ────────────────────────────────────────
//! 上游对请求体有两道墙，且都不在请求前告诉你：
//!   · 推理后端 ≈6 MiB —— 回 `InferHub.001001005.400 The request param is invalid`
//!     （判据在 `chat::size_rejection*`，它可能走在 HTTP 状态上，也可能走在
//!     **SSE 第一帧**里，所以登记点不止一处）；
//!   · API 网关 ≈12 MiB —— 回 HTTP 413 `APIG.0201`。
//! 撞墙的成本是一次完整的上行往返 + 几十秒等待，而且**同一条会话下一轮还会再来一次**。
//! 所以撞到就把量到的真值记下来，同尺寸或更大的请求在门内直接绕开这一家。
//!
//! ── 为什么放在本家而不是通用层 ──────────────────────────────
//! 目前只有这一家有「按体积挡」的需求与实测数据，通用层要为此加的是状态机 +
//! 各家接线；先在本家收成一个静态槽，等第二家出现同样形状再谈上提。
//! 单槽也够：codearts 只有一个 provider id，同一家不同账号共享同一道体积墙
//! （墙在上游，不在账号）。
//!
//! ── 三条口径 ────────────────────────────────────────────────
//!   · **只收窄不放宽**：同一分钟内量到更小的失败体积，门往下调（把墙的位置
//!     估高等于继续白撞）；
//!   · **TTL 到期自动放行**：门不持久，重启即清 —— 上游的墙会随负载与版本变，
//!     把它写进数据库就等于把一个瞬时事实变成永久事实；
//!   · 登记时**只在「从放行变成挡住」那一次**打一行日志：门存续期内每个大请求
//!     都会命中判据，逐请求打就是刷屏。

use std::sync::{Mutex, OnceLock};

/// 门的寿命：两分钟。够挡住同一条会话接下来几轮的白撞，又不至于把一个偶发故障
/// 记成"这家坏了"。
const GATE_TTL_MS: i64 = 120_000;

/// 出门前的本地拒绝用什么状态码：不是上游给的，是**我们自己知道发不出去**。
/// 选 503 而不是 502 —— 502 在本仓的既有语义是"上游回了个坏的"，会误导排查；
/// 503 配文案说清「本轮绕开这一家」，跨家降级照常按这条链往下走。
pub const BLOCKED_STATUS: u16 = 503;

#[derive(Debug)]
struct Gate {
    /// 已知发不出去的最小体积（门只往下收）
    floor_bytes: i64,
    expires_at_ms: i64,
}

fn slot() -> &'static Mutex<Option<Gate>> {
    static GATE: OnceLock<Mutex<Option<Gate>>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(None))
}

/// 登记一次体积失败。返回 `true` 表示**这一次**让门从"放行"变成了"挡住"
/// （调用方据此只在那一刻打日志，并把撞到的判据说清楚）。
pub fn hold(wire_bytes: i64, at_ms: i64) -> bool {
    let mut guard = match slot().lock() {
        Ok(guard) => guard,
        // 锁被毒过：里面的值仍是合法的 Option<Gate>，不该因此把判据丢掉
        Err(poisoned) => poisoned.into_inner(),
    };
    let blocked_before = guard.as_ref().is_some_and(|gate| gate.expires_at_ms > at_ms);
    match &mut *guard {
        // 门还在：只往下收，也不延长寿命（寿命由第一次撞墙决定，避免持续失败被记成永久）
        Some(gate) if gate.expires_at_ms > at_ms => {
            gate.floor_bytes = gate.floor_bytes.min(wire_bytes.max(0));
        }
        // 门已过期或从没建过：整条重来
        slot => *slot = Some(Gate { floor_bytes: wire_bytes.max(0), expires_at_ms: at_ms + GATE_TTL_MS }),
    }
    !blocked_before
}

/// 当前门内的下界（没门 / 已过期 = `None`）。
pub fn floor(at_ms: i64) -> Option<i64> {
    let guard = slot().lock().ok()?;
    guard.as_ref().filter(|gate| gate.expires_at_ms > at_ms).map(|gate| gate.floor_bytes)
}

/// 还剩多少毫秒到期（给文案说「多久后重新探」用）。
pub fn ms_left(at_ms: i64) -> i64 {
    let guard = match slot().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .as_ref()
        .map(|gate| gate.expires_at_ms.saturating_sub(at_ms).max(0))
        .unwrap_or(0)
}

#[cfg(test)]
mod gate_rules {
    use super::*;

    /// 静态槽会被并行用例互相踩，所以整组测试串行跑在同一把锁上，
    /// 并且每条进来先清空 —— 否则上一条用例留下的门会让这一条的断言变成顺序依赖。
    static SEQ: Mutex<()> = Mutex::new(());

    fn start_clean() -> std::sync::MutexGuard<'static, ()> {
        let guard = match SEQ.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Ok(mut slot) = slot().lock() {
            *slot = None;
        }
        guard
    }

    #[test]
    fn a_registered_failure_blocks_the_same_size_and_nothing_smaller() {
        let _guard = start_clean();
        let at = 10_000;
        assert!(hold(8_000_000, at), "第一次登记应当报「门建起来了」");
        assert_eq!(Some(8_000_000), floor(at));
        assert!(
            !hold(8_000_000, at + 1_000),
            "门已挡住时重复登记不能再报一次（否则日志会按每个大请求刷屏）"
        );
        // 更小的失败体积要把门往下收（估高了下界等于继续白撞）
        assert!(!hold(7_000_000, at + 2_000));
        assert_eq!(Some(7_000_000), floor(at + 2_000), "下界取的是量到的最小值");
        // 更大的失败体积不许把门放宽
        assert!(!hold(9_000_000, at + 3_000));
        assert_eq!(Some(7_000_000), floor(at + 3_000));
    }

    #[test]
    fn an_expired_gate_is_no_gate() {
        let _guard = start_clean();
        let at = 50_000;
        assert!(hold(123, at));
        assert_eq!(Some(123), floor(at + GATE_TTL_MS - 1));
        assert_eq!(None, floor(at + GATE_TTL_MS), "到期就放行：上游的墙会随负载与版本变");
        assert_eq!(0, ms_left(at + GATE_TTL_MS));
        assert!(ms_left(at + 1_000) > 0, "门内要说得出还剩多久，文案才有意义");
    }

    #[test]
    fn a_fresh_failure_replaces_an_expired_one() {
        let _guard = start_clean();
        assert!(hold(100, 0));
        // 过期后再撞：整条重来，寿命由新的这次决定
        assert!(hold(90, GATE_TTL_MS + 5));
        assert_eq!(Some(90), floor(GATE_TTL_MS + 5));
        assert_eq!(GATE_TTL_MS, ms_left(GATE_TTL_MS + 5), "寿命从这一次起算，不接上一次的尾巴");
    }
}
