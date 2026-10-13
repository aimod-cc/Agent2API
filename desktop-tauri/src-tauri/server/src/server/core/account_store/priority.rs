//! 账号优先级号段规则（对照 workbuddy-account-store.mjs 的优先级部分）。
//!
//! 优先级是**严格主备序号**：必须唯一，允许并列会让「同级」语义失效
//! （退化成永远只取加入最早的那个，其余账号形同虚设）。因此：
//!   - 写入侧（新增/修改）遇到冲突一律拒绝（409），由用户显式选一个空闲值；
//!   - 启动时对既有数据做一次性去重迁移（`renumber_consecutively` + store 的
//!     启动迁移），保住原有选路顺序；
//!   - 校验范围含已禁用账号 —— 否则禁用期间让出号段，重新启用就会撞车。
//!
//! ── 唯一性的作用域：**全局**（所有提供商共用一条队列）─────────
//! 曾经按 provider 各排各的队（workbuddy 与 raccoon 各自维护 0–9999），
//! 现在改回**全局唯一**：四家账号混在同一条队列里，转发时按优先级从小到大
//! 逐个尝试，跳过禁用 / 不支持该模型 / 该模型限流中的账号。于是「先用哪一家」
//! 由账号优先级本身决定，不再有独立的 provider 路由优先级。
//! 本模块的函数都按**传入的那一组账号**计算，调用方应把全部账号传进来。
//!
//! 本模块只做纯数值运算，不认识磁盘与 HTTP。

use serde_json::Value;

/// 默认优先级：首个账号从这里开始编号，后续账号依次追加
pub const DEFAULT_PRIORITY: i64 = 100;
pub const MIN_PRIORITY: i64 = 0;
pub const MAX_PRIORITY: i64 = 9999;

/// 优先级归一：非法值回落到默认值，并夹在允许区间内
/// （对应 Node 版 `normalizePriority`：null/undefined/'' 与 NaN 都回落）。
pub fn normalize_priority(value: Option<&Value>, fallback: i64) -> i64 {
    let number = match value {
        None | Some(Value::Null) => return fallback,
        // 空字符串在 JS 里 `Number('') === 0`，但 Node 版显式把它归到 fallback 分支
        Some(Value::String(text)) if text.trim().is_empty() => return fallback,
        Some(Value::Number(number)) => match number.as_f64() {
            Some(value) if value.is_finite() => value,
            _ => return fallback,
        },
        Some(Value::String(text)) => match text.trim().parse::<f64>() {
            Ok(value) if value.is_finite() => value,
            _ => return fallback,
        },
        // 对象/数组/布尔在 JS 里 Number() 多为 NaN → 回落
        _ => return fallback,
    };
    let rounded = number.round();
    // Math.min(MAX, Math.max(MIN, round(num)))
    (rounded as i64).clamp(MIN_PRIORITY, MAX_PRIORITY)
}

/// 直接从强类型记录上归一（避免每处都包一层 Value）
pub fn normalize_priority_value(value: i64) -> i64 {
    value.clamp(MIN_PRIORITY, MAX_PRIORITY)
}

/// 选路顺序：优先级升序，同优先级按加入时间（与 Node 版 `byPriorityOrder` 一致）。
/// 唯一性由写入侧保证，这里的次级排序键只是手工编辑文件时的兜底。
pub fn by_priority_order(a: (i64, i64), b: (i64, i64)) -> std::cmp::Ordering {
    let (a_priority, a_added) = a;
    let (b_priority, b_added) = b;
    normalize_priority_value(a_priority)
        .cmp(&normalize_priority_value(b_priority))
        .then(a_added.cmp(&b_added))
}

/// 下一个可用优先级：排在现有账号之后（新增账号不抢占已有转发顺序）。
///
/// `used` 是全部账号已占用的号段。这里收的是**裸数值**而不是记录列表：
/// 号段计算是纯数值运算，改造后调用方直接在投影列 `priority` 上查一次
/// （`sql::priorities_except`）就能填进来，不必为了数 20 个数把全部记录的 JSON
/// 解析一遍。
///
/// 号段用满时从默认值起找第一个空位；再找不到就回落默认值
/// （与 Node 版完全相同的兜底路径）。
pub fn next_free_priority(used: &[i64]) -> i64 {
    let max = used
        .iter()
        .map(|value| normalize_priority_value(*value))
        .max()
        .unwrap_or(DEFAULT_PRIORITY - 1);
    let mut candidate = max + 1;
    while used.contains(&candidate) && candidate <= MAX_PRIORITY {
        candidate += 1;
    }
    if candidate <= MAX_PRIORITY {
        return candidate;
    }
    for priority in MIN_PRIORITY..=MAX_PRIORITY {
        if !used.contains(&priority) {
            return priority;
        }
    }
    DEFAULT_PRIORITY
}

/// 一条优先级变更记录（返回给前端与日志展示）
#[derive(Clone, Debug)]
pub struct PriorityAssignment {
    pub id: String,
    pub name: String,
    pub from: i64,
    pub to: i64,
}

/// 队列里一条记录的**排序输入**：只有分组排序用得上的四个字段。
///
/// 本模块是纯数值模块（不认识磁盘与 HTTP），所以不吃 `StoredAccount` —— 调用方
/// 从投影列 / JSON 里把这四样取出来喂进来，排序规则就留在这里一处。
pub struct QueueEntry<'a> {
    pub id: &'a str,
    pub provider: &'a str,
    pub priority: i64,
    pub added_at: i64,
}

/// 「分组连续」的全局队列序（分组模式的**硬不变量**）。
///
/// 账号页按家分组展示，组的顺位由拖拽决定 —— 界面上「第 2 顺位的家排在第 1 顺位的家
/// 之后」这句话要成立，队列里同一家的账号就必须**连续**：中间夹进别家的账号，界面
/// 显示的先后与真实的尝试顺序就分叉了。本函数把这条件编译出来：
///   1. 先按家归堆；
///   2. 组序 = 各家**当前最靠前**的那个账号（组内最小优先级，并列比加入时间）；
///   3. 组内按优先级升序，并列按加入时间（与 [`by_priority_order`] 同口径）。
///
/// 组序不额外存一份的原因：它完全由「组里最靠前的账号」决定 —— 删空一家不会留下
/// 空档，新加一条也只影响它自己那一组，不必维护第二份顺序状态。
///
/// 只排序、不改号：编号由调用方按返回的顺序写回（0、1、2…）。返回的是**全部**传入
/// 记录的 id（一条不丢，顺序即队列序）。
pub fn grouped_queue_order(entries: &[QueueEntry<'_>]) -> Vec<String> {
    let mut groups: Vec<(&str, Vec<&QueueEntry<'_>>)> = Vec::new();
    for entry in entries {
        match groups
            .iter_mut()
            .find(|(provider, _)| *provider == entry.provider)
        {
            Some((_, list)) => list.push(entry),
            None => groups.push((entry.provider, vec![entry])),
        }
    }
    for (_, list) in groups.iter_mut() {
        list.sort_by(|a, b| {
            by_priority_order((a.priority, a.added_at), (b.priority, b.added_at))
        });
    }
    // 组序用组内第一位定：`sort_by` 是稳定排序，两个组的队首完全并列时保持
    // 传入顺序（调用方按物理行序读出来，与旧实现的数组顺序一致）
    groups.sort_by(|(_, a), (_, b)| {
        by_priority_order(
            (a[0].priority, a[0].added_at),
            (b[0].priority, b[0].added_at),
        )
    });
    groups
        .into_iter()
        .flat_map(|(_, list)| list.into_iter().map(|entry| entry.id.to_string()))
        .collect()
}

/// 把已排好序的账号重新连续编号，保持这个顺序不变。
///
/// 起点优先沿用原有最小优先级（这样只是整体平移，相对顺序与「第 N 位」都不变）；
/// 若从那里起排不下（号段顶到 MAX_PRIORITY），就整体下移到刚好放得下的位置 ——
/// 绝不能简单地对每个值取 `min(MAX, base + i)`，那样会把靠后的账号压成同一个
/// 数值，直接破坏「优先级唯一」这条不变量。
///
/// `records` 为 `(id, name, priority)` 的可变切片，按调用方排好的顺序传入。
/// 返回变更清单（未变化的项不列入）。
pub fn renumber_consecutively(records: &mut [(String, String, i64)]) -> Vec<PriorityAssignment> {
    if records.is_empty() {
        return Vec::new();
    }
    let base = records
        .iter()
        .map(|(_, _, priority)| normalize_priority_value(*priority))
        .min()
        .unwrap_or(DEFAULT_PRIORITY);
    let span = records.len() as i64 - 1;
    let start = if base + span <= MAX_PRIORITY {
        base
    } else {
        (MAX_PRIORITY - span).max(MIN_PRIORITY)
    };

    let mut assignments = Vec::new();
    for (index, (id, name, priority)) in records.iter_mut().enumerate() {
        let next = (start + index as i64).clamp(MIN_PRIORITY, MAX_PRIORITY);
        let from = normalize_priority_value(*priority);
        if from != next {
            *priority = next;
            assignments.push(PriorityAssignment {
                id: id.clone(),
                name: name.clone(),
                from,
                to: next,
            });
        }
    }
    assignments
}
