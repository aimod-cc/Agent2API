//! 账号调度：三种模式下的**候选挑选**（严格优先级 / 全局轮询 / 同家轮询）。
//!
//! ── 与 `routing` 的分工 ─────────────────────────────────────
//! `routing.rs` 回答「一个账号此刻可不可用」（纯判定，无状态）；
//! 本模块回答「在可用的账号里这次挑谁」（有状态 —— 轮询指针）。
//! 判定的唯一实现在 `routing::is_usable`，本模块只做挑选顺序，不重复
//! 写一遍可用性判据（理由见那里的说明）。
//!
//! ── 三种模式（语义见 `config::ScheduleMode`）──────────────────
//! ```text
//! priority   可用账号按 (优先级, 加入时间) 排序，取第一个 —— 改造前唯一行为
//! global     可用账号按 (优先级, 加入时间) 排序成环，从上次选中的账号之后
//!            找第一个可用的（环绕）；不分家，账号多的家天然占更多环位
//! provider   先按 (优先级, 加入时间) 找到优先级最小的账号，**只用它所属
//!            那一家的账号**成环，从该家上次选中的账号之后取下一个
//! ```
//!
//! ── 指针为什么按「上次选中的账号 id」而不是下标 ─────────────────
//! 下标在账号增删 / 排序变化后会指向另一个账号（把指针悄悄挪到别人身上，
//! 表现为「轮询跳号」）。存 id 之后，指针失效（账号被删 / 被禁用而掉出
//! 候选）时退化成「从环头开始」，行为可预期且不需要任何维护动作。
//!
//! ── 指针粒度：全局一个 / 每供应商一个，**不分模型** ──────────────
//! 「所有模型共用一条环」是有意的简化：按模型各存一条环会让「同一家多个
//! 模型各轮各的」变成多份互不相干的指针，用户看到的轮询节奏就散了。
//! 需要更细的粒度是另一个需求，不在本次范围内。
//!
//! ── 推进时机：**确认选中即推进**（不是成功才推进）────────────────
//! 挑选（[`peek`]）与推进（[`note_pick`]）是两个动作：挑只是算「下一个是谁」，
//! 中间还隔着几道可能失败的关卡（无凭证的账号会被 `get_session_by_id` 挡下、
//! 换号路径的预览只用于写日志）。确认要用某个账号发请求时推进一次 ——
//! 失败换号（429 降级、401 重试）同样走这条路，指针因此自然跟着走，
//! 不需要在重试路径上再写一遍推进逻辑。代价是「选中后立刻失败」也会消耗
//! 一次轮转位 —— 那正是轮询要的效果（这次不行就下次轮到别人）。
//!
//! 反过来说，**一次请求只消耗一格**这条不变量由「推进入口只有一个」保证：
//! 挑选侧全是只读的，谁都不能在「只是看一眼」的时候把环拨动。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use crate::server::config::ScheduleMode;
use crate::server::core::routing::{self, CooldownKeys};

/// 轮询指针表：`""`（全局模式）或 provider id → 上次选中的账号 id。
///
/// 与 `session_sticky` 同一取舍：一把 `Mutex` 包一张小表，临界区只是几次
/// 哈希操作。锁中毒不致命（指针丢失的后果只是「从环头重新开始」）。
fn cursors() -> &'static Mutex<HashMap<String, String>> {
    static CURSORS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CURSORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 按当前调度模式挑一个账号 —— **只读预览，不推进指针**。
///
/// `accounts` 是候选池（已按 provider 收窄，见 `upstream::rotate`），
/// 过滤与排序判据全部来自 `routing`；本函数只决定**挑选顺序**。
///
/// 返回 `None` = 候选里一个可用的都没有（调用方走第二级兜底）。
///
/// ── 为什么挑与推进是分开的 ──────────────────────────────────
/// 「挑出来」与「真的用它发」是两件事，中间隔着几道可能失败的关卡
/// （无凭证被 `get_session_by_id` 挡下、换号路径只是写日志的预告）。
/// 挑选侧一律只读，推进由确认选中的那一方调 [`note_pick`] —— 这样
/// 「一次请求消耗几格环位」只有一个答案，不会出现「两边都以为对方推了」
/// 的空档，也不会让打开账号页（★ 推算走 `api::session`）把环拨动。
pub fn peek(
    mode: ScheduleMode,
    accounts: &[Value],
    keys: &CooldownKeys<'_>,
    counts: &HashMap<String, usize>,
    exclude_ids: &[String],
    now: i64,
) -> Option<Value> {
    match mode {
        // 严格优先级：与改造前逐字相同的一条路（`pick_account_by_priority`）。
        // 没有指针，advance 对它无意义
        ScheduleMode::Priority => {
            routing::pick_account_by_priority(accounts, keys, counts, exclude_ids, now)
        }
        // 全局轮询：全部可用账号一条环，从上次选中的账号之后取下一个（环绕）
        ScheduleMode::Global => {
            let ring = usable_ring(accounts, keys, counts, exclude_ids, now);
            let cursor = read_cursor(GLOBAL_SCOPE);
            let picked = ring_after(&ring, cursor.as_deref())?;
            find_by_id(accounts, &picked)
        }
        // 同家轮询：先定「优先级最高的那一家」，再在该家的可用账号里环取
        ScheduleMode::Provider => {
            let (head, provider, ring) =
                provider_ring(accounts, keys, counts, exclude_ids, now)?;
            if ring.is_empty() {
                // 理论上不可达（head 就是该家的可用账号），兜底回队首
                return Some(head);
            }
            let scope = format!("provider:{provider}");
            let cursor = read_cursor(&scope);
            let picked = ring_after(&ring, cursor.as_deref())?;
            find_by_id(accounts, &picked)
        }
    }
}

/// 同家轮询的两个前置产物：队首账号、它所属的家、该家的可用账号环。
///
/// ── 家是怎么定的（本模式的语义核心）──────────────────────────
/// 用**全局优先级序里的第一个可用账号所属的家** —— 与严格优先级模式
/// 「这次会用谁」的家完全一致。于是「家与家之间仍严格按优先级」这条
/// 语义不需要任何额外配置：一家可用就不会用到别家；该家全忙 / 全限流时，
/// 全局序里的下一个可用账号自然落在别家，环也跟着换到那一家。
///
/// 环的推进**按家存**（`provider:<id>` → 账号 id）：换到另一家再换回来时，
/// 每家的环位各自保留，不会被对方搅乱。
///
/// 环为空（家被整体排除，理论上不可达）时返回空环，由调用方兜底回队首。
fn provider_ring(
    accounts: &[Value],
    keys: &CooldownKeys<'_>,
    counts: &HashMap<String, usize>,
    exclude_ids: &[String],
    now: i64,
) -> Option<(Value, String, Vec<String>)> {
    // 全局优先级序里的第一个可用账号 → 它的家
    let head = routing::pick_account_by_priority(accounts, keys, counts, exclude_ids, now)?;
    let provider = routing::provider_of(&head).to_string();
    // 该家的可用账号成环（顺序仍是优先级序）
    let same_provider: Vec<Value> = accounts
        .iter()
        .filter(|account| routing::provider_of(account) == provider)
        .cloned()
        .collect();
    let ring = usable_ring(&same_provider, keys, counts, exclude_ids, now);
    Some((head, provider, ring))
}

/// 推进轮询指针 —— **唯一的推进入口**，由「确认选中」的那一方调用。
///
/// [`peek`] 只挑不推；挑选之后、真正要用这个账号发请求时调本函数。
/// 粘性命中的账号同样走这里：它不是按模式挑出来的（被直接钉在首位），
/// 不推进的话粘性会话结束后轮询会从粘性之前的位置继续，把「刚用过的
/// 账号」立刻又轮一遍。AxonHub 同款语义（`TrackSelection(stickyCandidate)`：
/// 粘性渠道单独计入轮询计数）。
///
/// `priority` 模式没有指针，直接返回（与 `peek` 的分派一致）。
pub fn note_pick(mode: ScheduleMode, account_id: &str, provider_id: &str) {
    match mode {
        ScheduleMode::Priority => {}
        ScheduleMode::Global => write_cursor(GLOBAL_SCOPE, account_id),
        ScheduleMode::Provider => {
            write_cursor(&format!("provider:{provider_id}"), account_id)
        }
    }
}

/// 全局模式的指针作用域（与 `provider:<id>` 区分开）
const GLOBAL_SCOPE: &str = "";

/// 可用账号按 (优先级, 加入时间) 排序后的 id 环。
fn usable_ring(
    accounts: &[Value],
    keys: &CooldownKeys<'_>,
    counts: &HashMap<String, usize>,
    exclude_ids: &[String],
    now: i64,
) -> Vec<String> {
    let mut usable: Vec<&Value> = accounts
        .iter()
        .filter(|account| routing::is_usable(account, keys, counts, exclude_ids, now))
        .collect();
    usable.sort_by(|a, b| routing::compare_by_priority(a, b));
    usable
        .into_iter()
        .filter_map(|account| routing::account_id(account).map(str::to_string))
        .collect()
}

/// 从环上取「`cursor` 之后的下一个」，环绕。
///
/// `cursor` 为空 / 不在环上（账号被删、被禁用而掉出候选）→ 取环头 ——
/// 退化成「从优先级最高的开始」，可预期且无需维护动作。
fn ring_after(ring: &[String], cursor: Option<&str>) -> Option<String> {
    if ring.is_empty() {
        return None;
    }
    match cursor.and_then(|id| ring.iter().position(|item| item == id)) {
        // 指针在环上：取它之后的下一个（末尾环绕回第一个）
        Some(index) => ring.get((index + 1) % ring.len()).cloned(),
        // 指针为空 / 失效：从环头开始
        None => ring.first().cloned(),
    }
}

/// 从账号数组里按 id 取回完整记录（指针只存 id，返回值要带全部字段）。
fn find_by_id(accounts: &[Value], id: &str) -> Option<Value> {
    accounts
        .iter()
        .find(|account| routing::account_id(account) == Some(id))
        .cloned()
}

fn read_cursor(scope: &str) -> Option<String> {
    let guard = cursors().lock().ok()?;
    guard.get(scope).cloned()
}

fn write_cursor(scope: &str, account_id: &str) {
    if let Ok(mut guard) = cursors().lock() {
        guard.insert(scope.to_string(), account_id.to_string());
    }
}

/// 指针表当前内容（排障 / 测试用）。
#[cfg(test)]
pub fn cursors_snapshot() -> HashMap<String, String> {
    cursors().lock().map(|guard| guard.clone()).unwrap_or_default()
}

/// 清空指针表（测试用：进程级状态在测试之间会互相污染）。
#[cfg(test)]
pub fn clear_cursors() {
    if let Ok(mut guard) = cursors().lock() {
        guard.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 指针表是**进程级**的，而 cargo 默认并行跑测试：不串行化的话，
    /// 一个用例的 `clear_cursors` 会把另一个用例刚推进的环位清掉。
    /// 这把锁只存在于测试里（生产路径没有它，也不需要）。
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::server::core::test_state_lock()
    }

    fn account(id: &str, provider: &str, priority: i64) -> Value {
        json!({
            "id": id,
            "provider": provider,
            "priority": priority,
            "enabled": true,
            "hasCredentials": true,
            "addedAt": 0,
        })
    }

    fn pick_ids(
        mode: ScheduleMode,
        accounts: &[Value],
        rounds: usize,
    ) -> Vec<String> {
        let keys = CooldownKeys::new("m");
        let counts = HashMap::new();
        let mut picked = Vec::new();
        for round in 0..rounds {
            let now = 1_000 + round as i64;
            // 「挑 + 推进」的组合与生产路径一致（见 `upstream::rotate`）
            let account =
                peek(mode, accounts, &keys, &counts, &[], now).expect("有可用账号");
            let id = routing::account_id(&account).expect("有 id").to_string();
            note_pick(mode, &id, routing::provider_of(&account));
            picked.push(id);
        }
        picked
    }

    #[test]
    fn priority_mode_always_uses_head() {
        let _guard = test_lock();
        clear_cursors();
        let accounts = vec![
            account("W1", "workbuddy", 100),
            account("W2", "workbuddy", 110),
            account("C1", "catpaw", 200),
        ];
        assert_eq!(
            pick_ids(ScheduleMode::Priority, &accounts, 3),
            vec!["W1", "W1", "W1"]
        );
    }

    #[test]
    fn global_mode_rotates_across_providers() {
        let _guard = test_lock();
        clear_cursors();
        let accounts = vec![
            account("W1", "workbuddy", 100),
            account("W2", "workbuddy", 110),
            account("W3", "workbuddy", 120),
            account("C1", "catpaw", 200),
            account("C2", "catpaw", 210),
        ];
        assert_eq!(
            pick_ids(ScheduleMode::Global, &accounts, 6),
            vec!["W1", "W2", "W3", "C1", "C2", "W1"]
        );
    }

    #[test]
    fn provider_mode_rotates_within_head_provider() {
        let _guard = test_lock();
        clear_cursors();
        let accounts = vec![
            account("W1", "workbuddy", 100),
            account("W2", "workbuddy", 110),
            account("W3", "workbuddy", 120),
            account("C1", "catpaw", 200),
            account("C2", "catpaw", 210),
        ];
        // W 家是队首 → 只在 W 家内轮询，C 家完全用不到
        assert_eq!(
            pick_ids(ScheduleMode::Provider, &accounts, 4),
            vec!["W1", "W2", "W3", "W1"]
        );
    }

    #[test]
    fn provider_mode_falls_to_next_provider_when_head_family_busy() {
        let _guard = test_lock();
        clear_cursors();
        // 并发上限设 1：并发过滤才生效（缺失 / 0 = 不限，账号永远可用）
        let limited = |id: &str, provider: &str, priority: i64| {
            let mut value = account(id, provider, priority);
            value["maxConcurrent"] = json!(1);
            value
        };
        let accounts = vec![
            limited("W1", "workbuddy", 100),
            limited("W2", "workbuddy", 110),
            limited("C1", "catpaw", 200),
        ];
        let keys = CooldownKeys::new("m");
        // W1 达并发上限 → 队首落到 W2（仍是 W 家）→ 环在 W 家里
        let mut counts = HashMap::new();
        counts.insert("W1".to_string(), 1usize);
        let picked = peek(ScheduleMode::Provider, &accounts, &keys, &counts, &[], 0)
            .expect("有可用账号");
        assert_eq!(routing::account_id(&picked), Some("W2"));
        // W1、W2 都满 → 队首落到 C1，环换到 C 家
        counts.insert("W2".to_string(), 1usize);
        let picked = peek(ScheduleMode::Provider, &accounts, &keys, &counts, &[], 0)
            .expect("有可用账号");
        assert_eq!(routing::account_id(&picked), Some("C1"));
    }

    #[test]
    fn cursor_falls_back_to_head_when_account_removed() {
        let _guard = test_lock();
        clear_cursors();
        let accounts = vec![
            account("W1", "workbuddy", 100),
            account("W2", "workbuddy", 110),
        ];
        assert_eq!(pick_ids(ScheduleMode::Global, &accounts, 1), vec!["W1"]);
        // W2 被删：指针指向的 W2 不在环上 → 退化成环头
        let accounts = vec![account("W1", "workbuddy", 100)];
        assert_eq!(pick_ids(ScheduleMode::Global, &accounts, 1), vec!["W1"]);
    }

    #[test]
    fn exhausted_accounts_are_skipped_in_ring() {
        let _guard = test_lock();
        clear_cursors();
        let accounts = vec![
            account("W1", "workbuddy", 100),
            account("W2", "workbuddy", 110),
            account("C1", "catpaw", 200),
        ];
        let keys = CooldownKeys::new("m");
        let counts = HashMap::new();
        // 先选中 W1（指针落到 W1）—— 之后排除 W2，环上应跳过它取到 C1
        let picked = peek(ScheduleMode::Global, &accounts, &keys, &counts, &[], 0)
            .expect("有可用账号");
        assert_eq!(routing::account_id(&picked), Some("W1"));
        note_pick(ScheduleMode::Global, "W1", "workbuddy");
        let excluded = vec!["W2".to_string()];
        let picked = peek(ScheduleMode::Global, &accounts, &keys, &counts, &excluded, 0)
            .expect("有可用账号");
        assert_eq!(routing::account_id(&picked), Some("C1"), "W2 已排除，环上跳过它");
    }

    #[test]
    fn disabled_accounts_never_picked() {
        let _guard = test_lock();
        clear_cursors();
        let mut w1 = account("W1", "workbuddy", 100);
        w1["enabled"] = json!(false);
        let accounts = vec![w1, account("W2", "workbuddy", 110)];
        assert_eq!(pick_ids(ScheduleMode::Global, &accounts, 2), vec!["W2", "W2"]);
        assert_eq!(pick_ids(ScheduleMode::Provider, &accounts, 2), vec!["W2", "W2"]);
    }
}
