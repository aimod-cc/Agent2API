//! 通知去重：同类事件在窗口内只发一次。
//!
//! ── 为什么必须有这一层 ──────────────────────────────────────
//! 一个坏账号（登录态被上游作废）会让**每一轮**凭证维护都失败一次；
//! 定时任务的间隔可以是几分钟 —— 没有去重，用户会在几小时内收到上百条
//! 一模一样的「账号凭证失效」。通知一旦变成刷屏就失去了价值：人会关掉它，
//! 于是**真正**重要的那条也看不到了。
//!
//! ── 为什么是「窗口」而不是「状态变化时发」 ──────────────────
//! 状态变化时发（只在「好→坏」的沿上发）更精确，但要求本模块维护一份
//! 「上次是什么状态」的持久化台账，且要正确处理「进程重启后状态未知」
//! 「中途恢复又坏」这些情况。窗口去重只需要一张内存表，行为也更好解释：
//! **同一件事，6 小时内只说一次**。超出窗口后如果还没好，再提醒一次是对的
//! ——那说明用户确实还没处理。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::event::NotifyEvent;

/// 去重窗口：同一 (kind, key) 在这段时间内只发一次。
const WINDOW_MS: i64 = 6 * 60 * 60 * 1000;

/// 台账上限：正常使用远达不到；纯粹是防「key 每次都不同」时无限增长
/// （那种情况一般说明调用点把易变值放进了 key，见 `event` 的说明）。
const MAX_ENTRIES: usize = 512;

fn table() -> &'static Mutex<HashMap<String, i64>> {
    static TABLE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 去重键：(kind, key)。测试事件不参与去重（用户连点该收到多条）。
fn dedupe_key(event: &NotifyEvent) -> Option<String> {
    if event.kind == super::event::KIND_TEST {
        return None;
    }
    Some(format!("{}\u{1}{}", event.kind, event.key))
}

/// 这条事件是否**该被丢掉**（true = 窗口内已发过，跳过）。
///
/// 副作用：未命中时写入当前时刻 —— 所以调用一次即「占用」了这个窗口，
/// 应在真正决定要发之前调用（`notify` 就是这么用的）。
pub fn is_duplicate(event: &NotifyEvent) -> bool {
    let Some(key) = dedupe_key(event) else { return false };
    let now = crate::server::logging::now_ms();
    let mut guard = match table().lock() {
        Ok(guard) => guard,
        // 锁中毒：去重表本身没有不变式，取回内部表继续用比 panic 合理
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(last) = guard.get(&key) {
        if now.saturating_sub(*last) < WINDOW_MS {
            return true;
        }
    }
    // 超上限时清掉过期的（不整表清空：那会让所有事件立刻又能发一遍，正是要防的）
    if guard.len() >= MAX_ENTRIES {
        guard.retain(|_, at| now.saturating_sub(*at) < WINDOW_MS);
    }
    guard.insert(key, now);
    false
}

/// 清空台账（测试用；也可在「设置改了渠道」时调用，让新配的渠道立刻能收到
/// 之前被去重掉的事件 —— 那正是用户改配置的意图）。
pub fn reset() {
    let mut guard = match table().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.clear();
}
