//! 账号选路 —— 严格优先级排队（对照 Node 版 src/workbuddy-routing.mjs 全量移植）。
//!
//! 优先级是「主备序号」：优先级必须唯一（Agent2API 改造后作用域收窄为
//! **同 provider 内**，由 account-store 在写入侧保证，见其模块头），
//! 不允许多个账号并列 —— 并列会让「同级」语义失效。本模块只做**纯判定**，
//! 数据来源是账号存储的公开形态（`store.list_accounts()` 的 `accounts` 数组），
//! 字段就是 UI 上看到的那几个：`id` / `name` / `priority` / `addedAt` /
//! `enabled` / `rateLimits`。
//!
//! ── 为什么用 `serde_json::Value` 而不是强类型结构 ──────────────
//! Node 版的判据全部作用在「公开形态对象」上，而公开形态是容错的
//! （字段缺失/类型不对都不报错，按 JS 语义回落）。这里保持同一数据源与同一
//! 语义，避免为了「类型好看」把手工编辑出的脏数据在解析期整条丢掉 ——
//! 那会让选路结果与 Node 版分叉（例如某个账号因为 `priority: "abc"` 而消失）。
//!
//! ── 规则（照抄 Node 版头部注释）──────────────────────────────
//!   1. 候选 = 启用中（`enabled !== false`）且未对「该模型」处于限额冷却期的账号；
//!   2. 取候选里优先级数值最小的那个（数值小的先用）；
//!   3. 本次已尝试过的账号（429 降级）从候选中排除，避免回环；
//!   4. 全部候选都不可用时，由调用方决定是降级重试还是透传错误。
//!
//! ── 规则 1 里的「该模型」指**上游真名**，不是请求名 ─────────────
//! `rateLimits` 的键是上游实际收到并据此记额度的那个名字。映射
//! （`modelRules.mappings`）会在发送前把请求名改写成上游真名，所以判定与写入
//! 都必须用真名 —— 键怎么来见 [`cooldown_key`]，用请求名会有什么后果见那里。
//! 传进本模块各函数的 `model` 参数因此一律是**请求名**，由函数自己按账号所属
//! 的家解析成真名（[`CooldownKeys`] 负责让这次解析每家只做一次）。
//!
//! 「当前账号」不是独立的手动选择，而是本模块选路结果在「不限模型」下的那个账号
//! （account-store 的 `get_current_entry` 按同样的判据派生）。因此界面上的「当前」
//! 与转发默认使用谁始终一致；仅当账号对某具体模型限额时，该模型的请求才会临时
//! 降级到下一个候选 —— 那是按模型的一次性决策，不改写「当前账号」。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::server::core::account_store::priority::{by_priority_order, normalize_priority};
use crate::server::core::account_store::store_util::js_truthy;

/// 「请求名 → 各家上游真名」的解析缓存：限额冷却键的解析器。
///
/// ── 为什么冷却键必须是真名而不是请求名（本模块最要紧的一条）─────
/// `rateLimits` 的键是**上游按它记额度的那个名字**。映射（`modelRules.mappings`）
/// 会在发送前把请求名改写成上游真名（`providers::catalog::wire_target_for_provider`），
/// 上游因此只认识真名：它对 `deepseek-v4.1-flash` 记额度，而 `gpt-5.6-luna`
/// 这种纯对外名上游根本没见过。拿请求名当冷却键会同时坏掉三件事：
///
///   1. **写入分裂**：同一份额度被记成 `deepseek-v4.1-flash` 与 `gpt-5.6-luna`
///      两条互不相干的冷却。2026-09 实测：同一账号上 `deepseek-v4.1-flash` /
///      `gpt-5.6-luna` / `gpt-6-astra` 三个键的 `resetAt` 完全相同（它们本就
///      是同一份额度，因为后两个都是映射到第一个的对外名），账号页却显示成
///      「3 个模型限流中」。
///   2. **判定漏命中**（最实质的伤害）：用别名请求撞限额后，冷却写在别名键上；
///      接着用真名请求时读的是真名键 —— 读不到那条冷却，于是照样选中这个已经
///      限额的账号，白撞一次 429。换号链因此反复挑到「名字不同但同一个额度」
///      的账号，看起来像「映射没生效、请求还是打到原模型」。
///   3. **清除失效**：成功时按请求名清（`provider_loop::cap_cleared`），真名键下
///      那条旧记录一直留到重置时间，账号页常驻一条点不掉的限流记录。
///
/// ── 为什么按 provider 缓存（而不是每次现算）──────────────────
/// 真名是**按家**解析的：同一个 `gpt-6-astra` 在 workbuddy 家映射到
/// `deepseek-v4.1-flash`，在 catpaw 家可能映射到别的 target。而选路要对
/// **账号列表**逐个判定，同一家往往有多个账号 —— 不缓存的话，一家有几个账号
/// 就把 `wire_target_for_provider`（内含一次 `model_rules::current()` 克隆与
/// 一轮清单扫描）跑几遍。缓存键是 provider id，规模恒等于候选家数（≤6）。
///
/// 用 `Mutex` 而不是 `RefCell`：本结构会被**异步**的选路路径持有（`select_target_account`
/// 是 async），而 `RefCell` 不是 `Sync` —— 它会让整个 handler 的 future 失去
/// `Send`，编译期直接失败（这正好也说明它跨 await 存在）。锁的临界区只有
/// 几次哈希查找，且与 `wire_target_for_provider` 的解析不重叠（解析在锁外做，
/// 结果才写回），不存在锁竞争问题。
pub struct CooldownKeys<'a> {
    /// 客户端请求名（原始形态，未解析）
    requested: &'a str,
    /// provider id → 该家收到的上游真名
    resolved: Mutex<HashMap<String, String>>,
}

impl<'a> CooldownKeys<'a> {
    pub fn new(requested: &'a str) -> Self {
        Self { requested, resolved: Mutex::new(HashMap::new()) }
    }

    /// 请求名原文（轮转游标按它分键：不同模型的候选集合不同，各轮各的）
    pub fn request_model(&self) -> &'a str {
        self.requested
    }

    /// 该家实际收到的上游模型名 —— 也就是它的冷却键。
    ///
    /// 解析不出（请求名为空 / 该家既不原生承载、也没有映射指向它）时原样返回
    /// 请求名：与转发侧 `wire_target_for_provider` 的 ④ 兜底同一口径 ——
    /// 那时发出去的就是请求名，键也该是它。
    ///
    /// 锁中毒（别的线程 panic 过）时**不做缓存**、直接现算 —— 缓存只是加速，
    /// 选路结果不该因为一个内部加速器而失败。
    pub fn for_provider(&self, provider_id: &str) -> String {
        if self.requested.is_empty() || provider_id.is_empty() {
            return self.requested.to_string();
        }
        if let Ok(cache) = self.resolved.lock() {
            if let Some(cached) = cache.get(provider_id) {
                return cached.clone();
            }
        }
        // 传 `None` 账号：`wire_target_for_provider` 当前不读它（同家多条映射
        // 由 target 的通道前缀判定，见那里的说明），而选路时账号还没被选中 ——
        // 「按账号解析」在判定阶段根本无从谈起。
        //
        // ── 自定义提供商走自己的解析（第二阶段）───────────────────
        // modelRules 的映射表不认识 custom id（添加侧按注册表校验），
        // `wire_target_for_provider` 对它只会原样返回请求名 —— 而自定义家的
        // 真名解析有自己的数据源（提供商记录上的 `mappings`，见
        // `custom_providers::wire_model_for`）。跳过这一步会让 alias 请求的
        // 冷却写在请求名上：正是本模块头描述的那类「判定漏命中、已限额账号
        // 被反复选中」的事故形态，所以这里必须按家分派。
        let wire =
            if crate::server::core::custom_providers::is_custom_provider_id(provider_id) {
                crate::server::core::providers::custom::forward::cooldown_model(
                    provider_id,
                    self.requested,
                )
            } else {
                crate::server::core::providers::catalog::wire_target_for_provider(
                    self.requested,
                    provider_id,
                    None,
                )
                .model
            };
        if let Ok(mut cache) = self.resolved.lock() {
            cache.insert(provider_id.to_string(), wire.clone());
        }
        wire
    }

    /// 某条账号记录的冷却键：按它所属的家解析（每条账号记录只属于一家）。
    pub fn for_account(&self, account: &Value) -> String {
        self.for_provider(provider_of(account))
    }
}

/// 账号对某模型是否处于限额冷却期。
///
/// 对应 Node 版 `isRateLimited(account, model, now)`：只认 `rateLimits[model].resetAt`
/// 是**未来**时间戳的情况；记录存在但已过期 = 未限额（冷却自然结束，不必清理）。
///
/// 这里的 `keys` 把请求名解析成**上游真名**再查（理由见 [`CooldownKeys`]）。
pub fn is_rate_limited(account: &Value, keys: &CooldownKeys<'_>, now: i64) -> bool {
    rate_limit_reset_at(account, keys, now) > 0
}

/// 限额恢复时间（未限额返回 0）。
///
/// Node 版口径：`Number(limit.resetAt) || 0`，只有大于 now 才返回，
/// 否则返回 0 —— 这个 0 在选路里表示「当前可用」，不是「立刻恢复」。
///
/// 冷却键按 [`CooldownKeys`] 解析（真名，不是请求名）。
pub fn rate_limit_reset_at(account: &Value, keys: &CooldownKeys<'_>, now: i64) -> i64 {
    let key = keys.for_account(account);
    let Some(limit) = account
        .get("rateLimits")
        .and_then(|limits| limits.get(key.as_str()))
    else {
        return 0;
    };
    let reset_at = limit
        .get("resetAt")
        .and_then(js_number)
        .unwrap_or(0.0);
    if reset_at > now as f64 {
        reset_at as i64
    } else {
        0
    }
}

/// 账号是否可用于转发：启用 + 未被该模型限额。
/// `reason` ∈ `Some("disabled")` | `Some("rate-limited")` | `None`（可用）。
pub fn account_usability(account: &Value, keys: &CooldownKeys<'_>, now: i64) -> AccountUsability {
    // Node: `account?.enabled === false` —— 只有显式 false 才算禁用，
    // 缺失/字符串 "false"/0 都视为启用（与 account-store 的 enabled() 同口径）
    if matches!(account.get("enabled"), Some(Value::Bool(false))) {
        return AccountUsability { usable: false, reason: Some("disabled") };
    }
    if is_rate_limited(account, keys, now) {
        return AccountUsability { usable: false, reason: Some("rate-limited") };
    }
    AccountUsability { usable: true, reason: None }
}

/// `account_usability` 的结果
#[derive(Clone, Copy, Debug)]
pub struct AccountUsability {
    pub usable: bool,
    pub reason: Option<&'static str>,
}

/// 账号记录上的并发上限（单账号**同时在途**的下游请求数）。
///
/// 「一次下游请求 = 一条连接」的计数在 `upstream::connections`（账号页「连接数」
/// 列的数据源），口径正是并发上限要限制的东西，所以这里直接读账号记录上的
/// `maxConcurrent` 与那份计数比对。记录里没有该键、或值不是数字 = 0 = **不限**
/// —— 与写入侧（`store_crud::apply_patch` 的 0）和公开形态
/// （`store_util::max_concurrent_public` 的缺省 0）三条口径一致。
pub fn max_concurrent_of(account: &Value) -> u64 {
    account
        .get("maxConcurrent")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// 按优先级挑选本次请求使用的账号。
///
/// `accounts` 为 store.listAccounts().accounts 的公开形态；
/// `exclude_ids` 是本次请求已尝试过的账号 id（429 降级用）。
/// 返回账号对象，或 None（没有可用账号）。
///
/// `keys` 是请求名到各家真名的解析器（冷却键，见 [`CooldownKeys`]）。
///
/// `counts` 是各账号**当前在途请求数**（账号 id → 数，`upstream::connections`
/// 的快照）：`maxConcurrent > 0` 且在途数已达上限的账号被跳过，请求转给
/// 其他账号。没有运行时计数可拿的调用方传**空表** = 不做并发过滤（见
/// `pick_for_model` / `describe_route_decision` 的说明）。
///
/// ── 并发上限是**软上限**（选路与 rebind 之间的微小窗口）────────
/// 两个并发请求可能在同一瞬间选中同一个账号、然后各自 +1 —— 期间谁都还
/// 读不到对方的计数。为此引入额外加锁不值得：超出的那 1-2 个请求只意味着
/// 短暂超载（请求结束计数立刻回落），而选路必须是「读一次快照、立刻决策」
/// 的廉价操作。把这个口径写明：**允许短暂超 1-2 个，不做强一致**。
///
/// 排序取首位即为唯一答案（优先级唯一由写入侧保证）；并列属手工编辑出来的
/// 异常数据，按加入时间兜底，结果依旧稳定。
pub fn pick_account_by_priority(
    accounts: &[Value],
    keys: &CooldownKeys<'_>,
    counts: &HashMap<String, usize>,
    exclude_ids: &[String],
    now: i64,
) -> Option<Value> {
    let mut candidates: Vec<Value> = accounts
        .iter()
        .filter(|account| {
            let Some(id) = account_id(account) else {
                return false;
            };
            if exclude_ids.iter().any(|excluded| excluded == id) {
                return false;
            }
            account_usability(account, keys, now).usable
                // 并发过滤放在 usability 之后：先答「这个账号让不让你用」，
                // 再答「它忙不忙」—— 禁用 / 限流的原因不变，这里只追加一条。
                && !at_concurrency_limit(account, id, counts)
        })
        .cloned()
        .collect();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(compare_by_priority);
    candidates.into_iter().next()
}

/// 该账号的在途请求数是否已达并发上限（`maxConcurrent == 0` = 不限，恒 false）。
///
/// 计数缺失按 0（空闲）算：快照里只留非零项（见 `Connections::snapshot`），
/// 「缺键」与「0 个在途」是同一件事。
fn at_concurrency_limit(account: &Value, id: &str, counts: &HashMap<String, usize>) -> bool {
    let limit = max_concurrent_of(account);
    limit > 0 && counts.get(id).copied().unwrap_or(0) >= limit as usize
}

/// 按**指定模型**派生队首：候选先收窄到「清单里有这个模型的家」，再走
/// [`pick_account_by_priority`] 的常规判据（启用 + 该模型未限流 + 优先级序）。
///
/// ── 为什么要单独有这个函数（`pick_current` 不够用）──────────────
/// 账号库里的「当前账号」是**不限模型**的队首（只判启用 + 凭证），转发层却还要
/// 剔除「对该模型限流中」的账号。两者在「队首正被限流」时给出不同答案：界面标着
/// ★ 的那个账号，请求根本不会走它。账号页要回答的是「下一个请求会先用谁」，
/// 所以必须按模型派生，见 `/api/session` 的 `routedAccountId`。
///
/// ── 为什么要传 `providers` 而不是只给 model ──────────────────
/// 全局队列里四家混排，但一家只能承接**它自己清单里有的**模型（见
/// `providers::router::route_for_forward`）。少了这道收窄，一个「优先级更小、
/// 也对该模型未限流、但根本不提供该模型」的账号会被误判成队首。
/// `providers` 为空（未知模型）时不过滤 —— 宁可退回全局队首，也不给空答案。
///
/// `hasCredentials` 也算进判据：转发挑出候选后还要取到会话才用，无凭证的账号
/// 必然被跳过，前端按同一口径推算时才不会把这种账号标成 ★。
///
/// 冷却键按各家真名解析（[`CooldownKeys`]）—— 界面标的 ★ 因此与转发实际会先试
/// 的那个账号同判据：别名请求撞限额后，★ 不会再指向那个额度已耗尽的账号。
///
/// `counts` 的口径同样要对齐真实选路（并发过滤，见
/// [`pick_account_by_priority`]）：★ 推算拿得到运行时计数就传（`/api/session`
/// 从 `UpstreamService::connections()` 取），拿不到就传空表 = 并发不过滤 ——
/// 宁可 ★ 少剔一个忙账号，也不要让整条推算在缺数据时静默失效。
pub fn pick_for_model(
    accounts: &[Value],
    model: &str,
    providers: &[&str],
    counts: &HashMap<String, usize>,
    now: i64,
) -> Option<Value> {
    let candidates: Vec<Value> = accounts
        .iter()
        .filter(|account| {
            if !providers.is_empty() && !providers.iter().any(|known| *known == provider_of(account))
            {
                return false;
            }
            !matches!(account.get("hasCredentials"), Some(Value::Bool(false)))
        })
        .cloned()
        .collect();
    let keys = CooldownKeys::new(model);
    pick_account_by_priority(&candidates, &keys, counts, &[], now)
}

/// 账号记录上的 provider id（缺失按默认 provider 兜底，与 store 的
/// `StoredAccount::provider` 同口径）。
pub fn provider_of(account: &Value) -> &str {
    account
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID)
}

/// 选路决策的完整快照（供日志展示与排障）：
///   `{ picked, priority, candidateCount, blocked: [{ id, name, reason }], total }`
///
/// Node 版 workbuddy-routing.mjs 同名导出 `describeRouteDecision` 的对等物：
/// 生产路径用不到（Node 同样只导出未使用，仅 smoke test 覆盖），
/// 但排障时能直接拿来比对「为什么选了 B 而不是 A」，故保留 —— 字段与 Node 逐一对齐。
#[allow(dead_code)]
pub fn describe_route_decision(
    accounts: &[Value],
    model: &str,
    exclude_ids: &[String],
    now: i64,
) -> Value {
    let keys = CooldownKeys::new(model);
    let mut blocked: Vec<Value> = Vec::new();
    let mut candidate_count = 0usize;
    for account in accounts {
        let Some(id) = account_id(account) else {
            continue;
        };
        if exclude_ids.iter().any(|excluded| excluded == id) {
            blocked.push(json!({
                "id": id,
                "name": account_name(account, id),
                "reason": "tried",
            }));
            continue;
        }
        let usability = account_usability(account, &keys, now);
        if usability.usable {
            candidate_count += 1;
        } else {
            blocked.push(json!({
                "id": id,
                "name": account_name(account, id),
                "reason": usability.reason.unwrap_or(""),
            }));
        }
    }
    // 排障快照，拿不到运行时的在途计数（它住在 UpstreamService 里），
    // 传空表 = 并发过滤不生效 —— 这里只回答「按启用/限流该选谁」。
    let picked = pick_account_by_priority(accounts, &keys, &HashMap::new(), exclude_ids, now);
    let priority = picked
        .as_ref()
        .map(|account| normalize_priority_value_of(account))
        .unwrap_or(Value::Null);
    json!({
        "picked": picked.unwrap_or(Value::Null),
        "priority": priority,
        "candidateCount": candidate_count,
        "blocked": blocked,
        "total": accounts.len(),
    })
}

/// 账号 id（非空字符串才算；对应 Node 的 `account?.id` 真值判定）
pub fn account_id(account: &Value) -> Option<&str> {
    account
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

/// 账号展示名：`displayName`（统一展示名，见 `store_view::display_name_of`）
/// → `name` → id（快照缺 `displayName` 时按旧口径回落）
pub fn account_name(account: &Value, id: &str) -> String {
    account
        .get("displayName")
        .filter(|value| js_truthy(value))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            account.get("name").filter(|value| js_truthy(value)).and_then(|value| match value {
                Value::String(text) => Some(text.to_string()),
                other => Some(other.to_string()),
            })
        })
        .unwrap_or_else(|| id.to_string())
}

/// 从 store 快照 `{ currentAccountId, accounts: [...] }` 取账号数组。
/// 形状不对（手工编辑坏数据）时返回空列表 —— 与 Node 的
/// `Array.isArray(accounts) ? accounts : []` 同义。
pub fn accounts_of(snapshot: &Value) -> Vec<Value> {
    snapshot
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// 选路排序：优先级升序，同优先级按加入时间（复用 account-store 的规则实现，
/// 保证「界面排序」与「转发顺序」永远同一套判据）。
fn compare_by_priority(a: &Value, b: &Value) -> std::cmp::Ordering {
    let key = |account: &Value| {
        (
            normalize_priority(account.get("priority"), crate::server::core::account_store::priority::DEFAULT_PRIORITY),
            js_number(account.get("addedAt").unwrap_or(&Value::Null)).unwrap_or(0.0),
        )
    };
    let (a_priority, a_added) = key(a);
    let (b_priority, b_added) = key(b);
    by_priority_order((a_priority, a_added as i64), (b_priority, b_added as i64))
}

/// 公开形态里的 priority（归一后的数值）——对应 Node 在日志里打印的 `priority`。
/// 这里沿用 store 归一后的值（Rust 的公开形态一定带这个字段）。
fn normalize_priority_value_of(account: &Value) -> Value {
    Value::from(normalize_priority(
        account.get("priority"),
        crate::server::core::account_store::priority::DEFAULT_PRIORITY,
    ))
}

/// JS `Number(x)`：解析不出（NaN）时返回 None，由调用方按 `|| 0` 兜底。
fn js_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                // JS: Number('') === 0（与 Number('abc') 的 NaN 不同）
                Some(0.0)
            } else {
                trimmed.parse::<f64>().ok().filter(|value| value.is_finite())
            }
        }
        // Boolean / null / 对象 / 数组：Number(true)=1、Number(null)=0、
        // 其余 NaN。这些取值在真实数据里不会出现，但按 JS 语义实现不费事
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        Value::Null => Some(0.0),
        _ => None,
    }
}

// ─── 策略路由（plan_route）────────────────────────────────────
//
// 两种策略（顺序优先 / 负载均衡）与三种排序修饰
// （垫底 / 休息 / 熔断）都落在这里：输入一张**快照**，输出一份**带原因的
// 排序清单**。转发选路（rotate.rs）按清单从头往后找人；队列端点
// （api::routing）把整张清单原样画给用户。两条路调的是同一个函数，
// 「界面标的顺序」与「转发实际走的顺序」因此不可能分叉。

use crate::server::config::{RouteStrategy, StickyMode};

/// 一条候选在计划里的样子（转发用 `account`，界面用其余字段）。
#[derive(Clone, Debug)]
pub struct PlanEntry {
    pub account: Value,
    pub id: String,
    pub name: String,
    pub provider: String,
    /// 状态码：`usable` / `tried` / `disabled` / `cooldown` / `concurrency` /
    /// `limiter` / `no-credentials`
    pub state: &'static str,
    /// 一句「为什么排这 / 为什么跳过」（队列面板与请求详情直接展示）
    pub reason: String,
    /// 限额恢复时刻（毫秒；0 = 未限流）
    pub reset_at: i64,
}

/// 一份选路计划：可用候选（排好序）+ 被跳过的候选（各带原因）。
pub struct RoutePlan {
    pub ordered: Vec<PlanEntry>,
    pub blocked: Vec<PlanEntry>,
    /// 粘性命中的账号是否真的参与了排序（它可用且未被排除）
    pub sticky_hit: bool,
}

/// 策略排序要读的**全部外部事实**（一次快照，纯函数不写任何状态）。
pub struct PlanMeta<'a> {
    pub strategy: RouteStrategy,
    /// 沉底账号 → 沉底时刻（限流垫底）
    pub sunk: &'a HashMap<String, i64>,
    /// 休息中的账号 → 休息截止时刻（失败退避）
    pub resting: &'a HashMap<String, i64>,
    /// 熔断打开的账号
    pub open: &'a std::collections::HashSet<String>,
    /// 被限制器软跳过的账号（余额不足 / Token 限额，调用方判好传入）
    pub limiter_blocked: &'a std::collections::HashSet<String>,
    /// 轮转**计数**（`None` = 不轮转 —— 顺序优先策略）。取模由 `base_order` 做，
    /// 用的是过滤后的长度（理由见 `RouteState::rotate_take`）。
    pub cursor: Option<usize>,
    /// 家内轮询（`Provider`）下**每家各自的**轮转计数（provider id → 计数），
    /// 同样由 `provider_order` 按各家过滤后的成员数取模。
    /// 其余策略不看这个字段；`None` = 不轮转（预演 / 本轮已推进过）
    pub provider_cursors: Option<&'a HashMap<String, usize>>,
    /// 粘性命中的账号 id（`None` = 无命中）
    pub sticky_account: Option<&'a str>,
}

/// 时间戳 → 「HH:MM」短文案（原因句里用，按**操作者本机时区** —— 与请求
/// 统计的切日同一口径：用户看的是「我这里几点恢复」）。跨天时补「明天」前缀。
fn clock_text(reset_at: i64, now: i64) -> String {
    use chrono::{TimeZone, Timelike};
    let Some(secs) = reset_at.checked_div(1000) else {
        return String::new();
    };
    let Some(time) = chrono::Local.timestamp_opt(secs, 0).single() else {
        return String::new();
    };
    let days_ahead = (reset_at - now).max(0) / (24 * 3600 * 1000);
    let prefix = match days_ahead {
        0 => String::new(),
        1 => "明天 ".to_string(),
        _ => format!("{days_ahead} 天后 "),
    };
    format!("{prefix}{:02}:{:02}", time.hour(), time.minute())
}

/// 把可用候选按策略排出先后（不处理垫底 / 休息 / 熔断那三层修饰）。
fn base_order(candidates: &[Value], strategy: RouteStrategy, meta: &PlanMeta<'_>) -> Vec<Value> {
    let mut sorted: Vec<Value> = candidates.to_vec();
    sorted.sort_by(compare_by_priority);
    match strategy {
        RouteStrategy::Order => sorted,
        RouteStrategy::Rotate => {
            let len = sorted.len();
            if len <= 1 {
                return sorted;
            }
            // `cursor` 是单调计数，取模只在这里做一次 —— 用的是**过滤后**的长度
            // （`sorted` 已排除禁用 / 冷却 / 并发满 / 无凭证），与游标推进时那把
            // 尺子无关（见 `RouteState::rotate_take`）。
            let start = meta.cursor.unwrap_or(0) % len;
            (0..len)
                .map(|offset| sorted[(start + offset) % len].clone())
                .collect()
        }
        RouteStrategy::Provider => provider_order(sorted, meta),
    }
}

/// 家内轮询的排序：候选先按**家**分段，段之间按「段内最优账号的优先级」升序
/// （优先级最高的家排最前），段内按各自的游标环绕。
///
/// ── 为什么不直接在排好的线上轮转 ─────────────────────────────
/// `sorted` 已按优先级升序，是一条**跨家的线**；在这条线上环绕就是 `Rotate`
/// （轮一转会跳到别的家）。家内轮询的要点恰恰是「家的先后固定、只有家内换人」，
/// 所以必须先把这条线按家切成段、各段自己转，再按段拼回去。
///
/// ── 段序为什么取「首次出现的位置」────────────────────────────
/// 正常路径下 `sorted` 有序，某家第一次出现的那个账号就是这家**优先级最高**
/// 的账号 —— 按首次出现的位置编排各段，段序正是「各家最优账号之间的优先级
/// 顺序」，不必另算一遍。
///
/// ⚠️ 有一条例外，是有意为之：`plan_route` 会把**粘性命中的账号**提到传进来的
/// 列表最前（见那里「粘性命中的可用账号提到最前」一段）。那时它的家整段排到
/// 第一位，段序随之让位于粘性、不再按优先级 —— 粘性的语义就是「这个会话继续
/// 用同一个账号」，压过家的优先级才是对的。粘性只在本会话内生效，别的会话
/// 照常按优先级排。
fn provider_order(sorted: Vec<Value>, meta: &PlanMeta<'_>) -> Vec<Value> {
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    for account in sorted {
        let provider = provider_of(&account).to_string();
        match groups.iter_mut().find(|(name, _)| *name == provider) {
            Some((_, members)) => members.push(account),
            None => groups.push((provider, vec![account])),
        }
    }
    let mut arranged = Vec::new();
    for (provider, members) in groups {
        let len = members.len();
        if len <= 1 {
            arranged.extend(members);
            continue;
        }
        // 没有游标时从第 0 位起 —— 与 `Rotate` 分支对 `cursor: None` 的处理同一口径。
        // 游标是单调计数（`RouteState::rotate_take_per_provider`），取模只在这里做
        // 一次、只对**这一家过滤后的成员数**取 —— 家内有人被冷却时环会变短，位次
        // 序列仍然均匀。
        let start = meta
            .provider_cursors
            .and_then(|cursors| cursors.get(&provider))
            .copied()
            .unwrap_or(0)
            % len;
        for offset in 0..len {
            arranged.push(members[(start + offset) % len].clone());
        }
    }
    arranged
}

/// 统计一组账号里各家的账号数（`provider id → 个数`，按首次出现排）。
///
/// 家内轮询用它列出「有哪几家」，为每家建一条游标键（`rotate_take_per_provider`）。
/// 个数本身不参与取模 —— 取模在 `provider_order` 里按各家**过滤后**的成员数做
/// （见 `RouteState::rotate_take` 那段）。
pub fn provider_sizes(accounts: &[Value]) -> Vec<(String, usize)> {
    let mut sizes: Vec<(String, usize)> = Vec::new();
    for account in accounts {
        let provider = provider_of(account).to_string();
        match sizes.iter_mut().find(|(name, _)| *name == provider) {
            Some((_, count)) => *count += 1,
            None => sizes.push((provider, 1)),
        }
    }
    sizes
}

/// 生成一份完整的选路计划（转发选路与队列端点共用的**唯一**排序实现）。
///
/// `excluded` 是本轮已试过的账号（429 降级 / 失败顺延），它们进 `blocked`
/// （state = `tried`）——「这条请求不会再碰它」本来就是排障时要看的答案。
pub fn plan_route(
    accounts: &[Value],
    keys: &CooldownKeys<'_>,
    counts: &HashMap<String, usize>,
    excluded: &[String],
    now: i64,
    meta: &PlanMeta<'_>,
) -> RoutePlan {
    let mut ordered: Vec<Value> = Vec::new();
    let mut blocked: Vec<PlanEntry> = Vec::new();
    for account in accounts {
        let Some(id) = account_id(account) else { continue };
        let id = id.to_string();
        let name = account_name(account, &id);
        let provider = provider_of(account).to_string();
        let entry = |state: &'static str, reason: String| PlanEntry {
            account: account.clone(),
            id: id.clone(),
            name: name.clone(),
            provider: provider.clone(),
            state,
            reason,
            reset_at: rate_limit_reset_at(account, keys, now),
        };
        if excluded.iter().any(|tried| tried == &id) {
            blocked.push(entry("tried", "本轮已试过，不再回试".to_string()));
            continue;
        }
        if matches!(account.get("enabled"), Some(Value::Bool(false))) {
            blocked.push(entry("disabled", "已禁用，不参与转发".to_string()));
            continue;
        }
        if meta.limiter_blocked.contains(&id) {
            blocked.push(entry(
                "limiter",
                "被限制器拦下（余额不足或 Token 周期用量到顶），自动恢复后回来".to_string(),
            ));
            continue;
        }
        let reset_at = rate_limit_reset_at(account, keys, now);
        if reset_at > 0 {
            blocked.push(entry(
                "cooldown",
                format!(
                    "对该模型限流冷却中，{} 恢复（恢复后自动回来）",
                    clock_text(reset_at, now)
                ),
            ));
            continue;
        }
        if at_concurrency_limit(account, &id, counts) {
            blocked.push(entry(
                "concurrency",
                format!(
                    "并发已满（{}/上限），等一个在途回落再参与",
                    counts.get(&id).copied().unwrap_or(0)
                ),
            ));
            continue;
        }
        if matches!(account.get("hasCredentials"), Some(Value::Bool(false))) {
            blocked.push(entry("no-credentials", "没有可用凭证，登录后才能接请求".to_string()));
            continue;
        }
        ordered.push(account.clone());
    }

    // 基础序（策略）→ 稳定修饰：没沉底的在前 → 没休息的在前 → 没熔断的在前
    // 三个修饰**跨策略**生效：垫底（Sink）解决的是「每次回到第 1 位再撞一次
    // 429」，退避解决的是「反复失败反复撞」—— 它们在顺序优先下同样成立；
    // 若只对负载均衡生效，默认策略下这两个开关就是摆设。稳定排序保证
    // 同档内的先后仍由策略的基础序决定（顺序优先下 = 位次）。
    let mut arranged = base_order(&ordered, meta.strategy, meta);

    // ── 粘性命中的可用账号提到最前（它可用，请求就会走它 —— 排序必须如实反映）
    //
    // ⚠️ 这一步**必须在 `base_order` 之后**（2026-10-13 修正）：`base_order` 的
    // 三个分支都以 `sort_by(compare_by_priority)` 开头，置顶若放在它之前，会被
    // 那句排序按优先级原样冲回原位 —— 粘性只在「命中的账号本来就是队首」时
    // 才生效，命中非队首账号时请求照样回到队首（2026-10-13 在 `.3` 上实测到：
    // 同一个 trace 的相邻两条请求落在 priority=107 与 priority=100 两个账号上）。
    // 上游原实现就是放在前面的，属既有缺陷；本仓把它移到这里修掉。
    //
    // 放在这里还有一层：下面三个修饰用的是 `sort_by_key`（**稳定**排序），
    // 同键元素的相对次序原样保留，所以置顶项不会被它们打乱；而排在最前的
    // 命中账号若正处冷却 / 熔断，本就该让位 —— 命中项能走到这一步就说明它
    // 可用（不可用的在收集阶段已进 `blocked`），两者不冲突。
    let mut sticky_hit = false;
    if let Some(hit) = meta.sticky_account {
        if let Some(index) = arranged
            .iter()
            .position(|account| account_id(account) == Some(hit))
        {
            let hit_account = arranged.remove(index);
            arranged.insert(0, hit_account);
            sticky_hit = true;
        }
    }

    let rank_of = |account: &Value, table: &HashMap<String, i64>| -> bool {
        account_id(account)
            .and_then(|id| table.get(id))
            .map(|until| *until > 0)
            .unwrap_or(false)
    };
    arranged.sort_by_key(|account| rank_of(account, meta.sunk));
    arranged.sort_by_key(|account| rank_of(account, meta.resting));
    // 熔断的账号放在队尾（所有策略一致）：只有全场都打不开时才轮到它试一次
    arranged.sort_by_key(|account| {
        account_id(account)
            .map(|id| meta.open.contains(id))
            .unwrap_or(false)
    });

    // 每个位次「前面出现过的家是不是全是它自己家」—— `Provider` 策略的原因句
    // 要用：排在第 2 位的若是**同一家的下一个账号**（家 A = a1/a2、家 B = b1），
    // 前面并没有「优先级更高的家」，它是家内轮流的正常位次。此前那句一律写
    // 「等优先级更高的家轮完或失败才到它」，是一句与排序事实冲突的假话
    // （2026-10-14 review 发现）。
    let prefix_is_same_family: Vec<bool> = {
        let mut seen: Vec<String> = Vec::new();
        let mut flags = Vec::with_capacity(arranged.len());
        for account in &arranged {
            let provider = provider_of(account).to_string();
            flags.push(seen.iter().all(|name| *name == provider));
            if !seen.iter().any(|name| *name == provider) {
                seen.push(provider);
            }
        }
        flags
    };

    let ordered_entries: Vec<PlanEntry> = arranged
        .into_iter()
        .enumerate()
        .map(|(index, account)| {
            let id = account_id(&account).unwrap_or("").to_string();
            let position = index + 1;
            let mut reason = if sticky_hit && Some(id.as_str()) == meta.sticky_account {
                // 「继续用它」只在它**真的**排在队首时才说（置顶之后还有三层
                // 稳定修饰，粘住的账号若正在垫底 / 休息 / 熔断，会被推回队尾）。
                // 不说这一句会让原因句与事实冲突 —— 界面写着「本轮继续用它」，
                // 实际却排在第 10 位（2026-10-13 review 发现）。
                if position == 1 {
                    "会话粘住：这个会话上一轮是它答的，本轮继续用它".to_string()
                } else {
                    format!("会话粘住，但它当前排在第 {position} 位（正在让位于下面的状态）")
                }
            } else {
                match meta.strategy {
                    RouteStrategy::Order => {
                        if position == 1 {
                            "队列第 1 位：顺序优先下它接走所有请求，直到不能答".to_string()
                        } else {
                            format!("队列第 {position} 位：前面的都可用时轮不到它")
                        }
                    }
                    RouteStrategy::Rotate => {
                        if position == 1 {
                            "本轮轮到它：轮流均摊下每条请求换下一位".to_string()
                        } else {
                            format!("排在第 {position} 位：等前面的轮完或失败才到它")
                        }
                    }
                    RouteStrategy::Provider => {
                        if position == 1 {
                            "本轮轮到它：家之间按优先级、家内轮流，它是当前该上的那一位".to_string()
                        } else if prefix_is_same_family[index] {
                            // 同家的后一位：前面没有别的家，「等优先级更高的家」是假话
                            format!("排在第 {position} 位：同一家内轮流，轮到它才上")
                        } else {
                            format!("排在第 {position} 位：等优先级更高的家轮完或失败才到它")
                        }
                    }
                }
            };
            if meta.sunk.contains_key(&id) {
                // 文案跟着 `route_state::SINK_TTL_MS` 走：垫底现在有 30 分钟寿命，
                // 不再是「轮一圈才回来」（那句是上游原实现的说法，那时垫底永不失效）。
                reason.push_str("；曾还有额度时被 429（垫底中，30 分钟后自动回到队列）");
            }
            if let Some(until) = meta.resting.get(&id) {
                reason.push_str(&format!("；转发失败休息至 {}", clock_text(*until, now)));
            }
            if meta.open.contains(id.as_str()) {
                reason.push_str("；熔断打开中（连续失败到阈值），到点放行试一次");
            }
            PlanEntry {
                name: account_name(&account, &id),
                provider: provider_of(&account).to_string(),
                account,
                id,
                state: "usable",
                reason,
                reset_at: 0,
            }
        })
        .collect();

    RoutePlan {
        ordered: ordered_entries,
        blocked,
        sticky_hit,
    }
}

/// 粘性 TTL（毫秒）：转发选路与队列端点共用的换算（StickyMode → 时长）。
pub fn sticky_ttl_ms(mode: StickyMode) -> i64 {
    mode.ttl_ms().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 家内轮询的一份最小 meta：只带各家游标，其余状态为空。
    fn provider_meta<'a>(
        cursors: &'a HashMap<String, usize>,
        empty_numbers: &'a HashMap<String, i64>,
        empty_ids: &'a std::collections::HashSet<String>,
    ) -> PlanMeta<'a> {
        PlanMeta {
            strategy: RouteStrategy::Provider,
            sunk: empty_numbers,
            resting: empty_numbers,
            open: empty_ids,
            limiter_blocked: empty_ids,
            cursor: None,
            provider_cursors: Some(cursors),
            sticky_account: None,
        }
    }

    fn account(id: &str, provider: &str, priority: i64) -> Value {
        json!({ "id": id, "provider": provider, "priority": priority })
    }

    fn ordered_ids(accounts: &[Value], meta: &PlanMeta<'_>) -> Vec<String> {
        provider_order(accounts.to_vec(), meta)
            .iter()
            .map(|account| account_id(account).unwrap_or("").to_string())
            .collect()
    }

    /// 家内轮询的核心语义：家之间保持优先级顺序，只有家内换人。
    #[test]
    fn provider_order_rotates_inside_a_family_but_keeps_families_in_priority_order() {
        let empty_numbers: HashMap<String, i64> = HashMap::new();
        let empty_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        // A 家轮到第 1 位（a2）；B 家没有游标，从第 0 位起
        let cursors: HashMap<String, usize> = HashMap::from([("A".to_string(), 1usize)]);
        let meta = provider_meta(&cursors, &empty_numbers, &empty_ids);
        // 传进来的已是「按优先级升序」的线（plan_route 的输入形态）
        let sorted = vec![
            account("a1", "A", 100),
            account("a2", "A", 110),
            account("b1", "B", 120),
        ];
        // A 家在前（它最优账号的 100 最小）且家内轮到 a2；B 家整段排在 A 家之后
        assert_eq!(ordered_ids(&sorted, &meta), vec!["a2", "a1", "b1"]);
    }

    #[test]
    fn provider_order_without_a_cursor_starts_at_each_familys_top() {
        let empty_numbers: HashMap<String, i64> = HashMap::new();
        let empty_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let cursors: HashMap<String, usize> = HashMap::new();
        let meta = provider_meta(&cursors, &empty_numbers, &empty_ids);
        let sorted = vec![
            account("a1", "A", 100),
            account("a2", "A", 110),
            account("b1", "B", 120),
        ];
        assert_eq!(ordered_ids(&sorted, &meta), vec!["a1", "a2", "b1"]);
    }

    /// 游标按**当前**候选数取模：某家账号被限流掉几个、环变短时不许越界
    #[test]
    fn provider_cursor_wraps_instead_of_going_out_of_range() {
        let empty_numbers: HashMap<String, i64> = HashMap::new();
        let empty_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let cursors: HashMap<String, usize> = HashMap::from([("A".to_string(), 7usize)]);
        let meta = provider_meta(&cursors, &empty_numbers, &empty_ids);
        let sorted = vec![account("a1", "A", 100), account("a2", "A", 110)];
        // 7 % 2 = 1 → 从 a2 起
        assert_eq!(ordered_ids(&sorted, &meta), vec!["a2", "a1"]);
    }

    #[test]
    fn provider_sizes_counts_each_family() {
        let accounts = vec![
            account("a1", "A", 100),
            account("b1", "B", 120),
            account("a2", "A", 110),
        ];
        assert_eq!(
            provider_sizes(&accounts),
            vec![("A".to_string(), 2), ("B".to_string(), 1)]
        );
    }

    #[test]
    fn provider_strategy_round_trips_through_its_config_id() {
        assert_eq!(RouteStrategy::from_id("provider"), RouteStrategy::Provider);
        assert_eq!(RouteStrategy::Provider.as_str(), "provider");
    }

    /// `Provider` 策略的原因句：**同家的下一位**不许说成「等优先级更高的家」。
    ///
    /// 家 A = a1/a2、家 B = b1 —— 第 2 位是 a2，它前面只有同家的 a1，前面
    /// **没有**任何别的家。此前这句一律写成「等优先级更高的家轮完或失败才到
    /// 它」，是一句与排序事实相冲突的假话（2026-10-14 review 发现）。原因句是
    /// 排查排序时唯一要读的东西，不能是假的。
    ///
    /// 走完整的 `plan_route`：原因句由它生成，`provider_order` 只排不写文案。
    #[test]
    fn a_sibling_in_the_same_family_is_not_explained_by_a_higher_priority_family() {
        let empty_numbers: HashMap<String, i64> = HashMap::new();
        let empty_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let counts: HashMap<String, usize> = HashMap::new();
        let cursors: HashMap<String, usize> = HashMap::new();
        let meta = provider_meta(&cursors, &empty_numbers, &empty_ids);
        let accounts = vec![
            account("a1", "A", 100),
            account("a2", "A", 110),
            account("b1", "B", 120),
        ];
        let plan = plan_route(
            &accounts,
            &CooldownKeys::new("test-model"),
            &counts,
            &[],
            1_700_000_000_000,
            &meta,
        );
        let order: Vec<&str> = plan.ordered.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(order, vec!["a1", "a2", "b1"]);
        // 第 2 位是同家的 a2
        assert!(plan.ordered[1].reason.contains("同一家内轮流"), "{}", plan.ordered[1].reason);
        assert!(!plan.ordered[1].reason.contains("优先级更高的家"), "{}", plan.ordered[1].reason);
        // 第 3 位的 b1 前面确实有 A 家，仍按「优先级更高的家」解释
        assert!(plan.ordered[2].reason.contains("优先级更高的家"), "{}", plan.ordered[2].reason);
    }

    /// 粘性命中的账号必须真的排到第 1 位 —— 即使它的优先级最低。
    ///
    /// 这条钉的是 2026-10-13 修掉的一处**上游既有缺陷**：置顶那一步原先在
    /// `base_order` 之前，而 `base_order` 三个分支都以 `sort_by(compare_by_priority)`
    /// 开头，置顶随即被冲回原位 ⇒ 粘性只在「命中账号本来就是队首」时生效。
    /// 实测证据：`.3` 上同一个 trace 的相邻两条请求落在 priority=107 与 100 上。
    ///
    /// 走完整的 `plan_route`（而不是直接调 `provider_order`）：置顶与排序的
    /// **先后关系**正是这条用例要钉的东西，只有从入口调才验得到。
    #[test]
    fn sticky_pinning_survives_the_base_order_sort() {
        let empty_numbers: HashMap<String, i64> = HashMap::new();
        let empty_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let counts: HashMap<String, usize> = HashMap::new();
        let keys = CooldownKeys::new("test-model");
        let accounts = vec![
            account("a1", "A", 100),
            account("a2", "A", 110),
            account("a3", "A", 120),
        ];
        // 粘性命中优先级最低的 a3 —— 置顶生效时它必须排第一
        let meta = PlanMeta {
            strategy: RouteStrategy::Order,
            sunk: &empty_numbers,
            resting: &empty_numbers,
            open: &empty_ids,
            limiter_blocked: &empty_ids,
            cursor: None,
            provider_cursors: None,
            sticky_account: Some("a3"),
        };
        let plan = plan_route(&accounts, &keys, &counts, &[], 1_700_000_000_000, &meta);
        let order: Vec<&str> = plan.ordered.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            order,
            vec!["a3", "a1", "a2"],
            "粘性命中的 a3 必须排在队首（粘性的全部意义就是「继续用它」）"
        );
        assert!(plan.sticky_hit);
        // 剩下两位仍按优先级（置顶只动命中那一项，不整体重排）
        assert_eq!(plan.ordered[1].id, "a1");
        assert_eq!(plan.ordered[2].id, "a2");
    }

    /// 家内轮询下也一样：命中项置顶后，家内顺序的**其余部分**照常环绕。
    #[test]
    fn sticky_pinning_survives_under_the_provider_strategy() {
        let empty_numbers: HashMap<String, i64> = HashMap::new();
        let empty_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let counts: HashMap<String, usize> = HashMap::new();
        let keys = CooldownKeys::new("test-model");
        let cursors: HashMap<String, usize> = HashMap::from([("A".to_string(), 1usize)]);
        let accounts = vec![
            account("a1", "A", 100),
            account("a2", "A", 110),
            account("b1", "B", 120),
        ];
        let meta = PlanMeta {
            strategy: RouteStrategy::Provider,
            sunk: &empty_numbers,
            resting: &empty_numbers,
            open: &empty_ids,
            limiter_blocked: &empty_ids,
            cursor: None,
            provider_cursors: Some(&cursors),
            sticky_account: Some("b1"),
        };
        let plan = plan_route(&accounts, &keys, &counts, &[], 1_700_000_000_000, &meta);
        let order: Vec<&str> = plan.ordered.iter().map(|entry| entry.id.as_str()).collect();
        // b1 置顶（家 B 优先级最低，本来排最后）⇒ 粘性压过家的优先级，这是设计
        assert_eq!(order, vec!["b1", "a2", "a1"]);
    }

    /// 粘住的账号若正在垫底，会被三层稳定修饰推回队尾 —— 那时原因句
    /// **不能**再说「本轮继续用它」（2026-10-13 review 发现：置顶在排序修饰
    /// 之前，界面会写着「继续用它」而它实际排第 2 位）。
    #[test]
    fn a_sticky_account_that_is_sunk_says_so_instead_of_claiming_it_will_be_used() {
        let empty_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let counts: HashMap<String, usize> = HashMap::new();
        let keys = CooldownKeys::new("test-model");
        let accounts = vec![account("a1", "A", 100), account("a2", "A", 110)];
        // a2 粘住，但它同时在 sink 表里
        let sunk: HashMap<String, i64> = HashMap::from([("a2".to_string(), 1_700_000_000_000i64)]);
        let empty_numbers: HashMap<String, i64> = HashMap::new();
        let meta = PlanMeta {
            strategy: RouteStrategy::Order,
            sunk: &sunk,
            resting: &empty_numbers,
            open: &empty_ids,
            limiter_blocked: &empty_ids,
            cursor: None,
            provider_cursors: None,
            sticky_account: Some("a2"),
        };
        let plan = plan_route(&accounts, &keys, &counts, &[], 1_700_000_000_000, &meta);
        let order: Vec<&str> = plan.ordered.iter().map(|entry| entry.id.as_str()).collect();
        // 垫底的 a2 被推回第 2 位（粘性置顶让位于垫底 —— 这正是要如实展示的事实）
        assert_eq!(order, vec!["a1", "a2"]);
        assert!(plan.sticky_hit, "命中过就该记着（sticky_hit 表示「查到过」）");
        // 原因句必须如实：说它在第几位，而不是「本轮继续用它」
        let a2 = &plan.ordered[1];
        assert!(
            a2.reason.contains("排在第 2 位"),
            "垫底中的粘住账号不该声称「本轮继续用它」，实际原因句：{}",
            a2.reason
        );
        assert!(!a2.reason.contains("本轮继续用它"));
        // 队首的 a1 走正常原因句（不是粘住那句）
        assert!(plan.ordered[0].reason.contains("队列第 1 位"));
    }

    /// 家内轮询**经完整 `plan_route`** 的效果：家之间按优先级、家内按游标环绕。
    ///
    /// 上面几条 `provider_order_*` 都只调那个纯函数；这一条从 `plan_route` 进，
    /// 验的是「接线正确」—— 档位传到 `base_order`、`provider_cursors` 真的被
    /// 消费到。只测纯函数的话，接线处把 `provider_cursors` 传成 `None` 也全绿。
    #[test]
    fn the_provider_strategy_keeps_families_in_priority_order_and_rotates_inside() {
        let empty_numbers: HashMap<String, i64> = HashMap::new();
        let empty_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let counts: HashMap<String, usize> = HashMap::new();
        let keys = CooldownKeys::new("test-model");
        // B 家最优账号的优先级（120）低于 A 家（100）⇒ 整段 B 排在整段 A 之后
        let accounts = vec![
            account("a1", "A", 100),
            account("b1", "B", 120),
            account("a2", "A", 110),
            account("b2", "B", 130),
        ];
        // 两家各自的游标都是 1 ⇒ 各自从第二个成员起
        let cursors: HashMap<String, usize> =
            HashMap::from([("A".to_string(), 1usize), ("B".to_string(), 1usize)]);
        let meta = PlanMeta {
            strategy: RouteStrategy::Provider,
            sunk: &empty_numbers,
            resting: &empty_numbers,
            open: &empty_ids,
            limiter_blocked: &empty_ids,
            cursor: None,
            provider_cursors: Some(&cursors),
            sticky_account: None,
        };
        let plan = plan_route(&accounts, &keys, &counts, &[], 1_700_000_000_000, &meta);
        let order: Vec<&str> = plan.ordered.iter().map(|entry| entry.id.as_str()).collect();
        // A 家整段在前（家内从 a2 起环绕），B 家整段在后（家内从 b2 起环绕）
        assert_eq!(order, vec!["a2", "a1", "b2", "b1"]);
        // 队首的原因句是家内轮询那一档（不是顺序优先 / 负载均衡的）
        assert!(
            plan.ordered[0].reason.contains("家之间按优先级"),
            "队首原因句应当是家内轮询的口径，实际：{}",
            plan.ordered[0].reason
        );
    }

    /// 家内轮询下**没有游标**（预演 / 队列端点）时，各家都从自己的队首起。
    #[test]
    fn the_provider_strategy_without_cursors_shows_each_family_from_its_top() {
        let empty_numbers: HashMap<String, i64> = HashMap::new();
        let empty_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let counts: HashMap<String, usize> = HashMap::new();
        let keys = CooldownKeys::new("test-model");
        let accounts = vec![
            account("a1", "A", 100),
            account("b1", "B", 120),
            account("a2", "A", 110),
            account("b2", "B", 130),
        ];
        let meta = PlanMeta {
            strategy: RouteStrategy::Provider,
            sunk: &empty_numbers,
            resting: &empty_numbers,
            open: &empty_ids,
            limiter_blocked: &empty_ids,
            cursor: None,
            provider_cursors: None,
            sticky_account: None,
        };
        let plan = plan_route(&accounts, &keys, &counts, &[], 1_700_000_000_000, &meta);
        let order: Vec<&str> = plan.ordered.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(order, vec!["a1", "a2", "b1", "b2"]);
    }
}


