//! 账号出网代理轮询，候选来自手动地址与 EasyClash 端点。
//! 游标按账号存于进程内；展示只读取候选，不推进游标。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::server::core::proxies::ResolvedProxy;
// 条目的「有效启用」（来源侧启用 ∧ 非本地禁用）直接复用池的判定，不再另写一份
// —— 只读 `enabled` 的判据会让「在「网络代理」页关掉的节点照样被轮询选中」：
// 用户按下开关却看不出任何效果，且每次选中都解析失败再顺延。
use crate::server::core::proxy_pool::is_enabled;

/// 指定轮询时**配置里最多留多少条 id**：与 `core::proxies` 的归一上限同源
/// （两处都写 64 会被改歪一处，所以只在这里定义，归一那边引用它）。
pub const MAX_IDS: usize = 64;

/// 轮询游标表的大小上限（超出时整表清空）。
///
/// 键是账号 id，正常规模在几十个以内 —— 这个上限只防「账号被反复重建」这类
/// 病态增长（每条记录一个新 id）。清空是安全的：游标只是一段进度，丢了从头转。
const MAX_CURSORS: usize = 512;

/// 账号 id → 下一次从第几个候选开始（进程级，见模块头）
static CURSORS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

fn lock_cursors() -> std::sync::MutexGuard<'static, Option<HashMap<String, u64>>> {
    match CURSORS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 取号：返回本次从候选列表的第几位开始，并把游标推到下一位。
///
/// `len` 必须非 0（调用方已挡）；条目数变化时取模自然收敛到新列表长度。
fn take_cursor(key: &str, len: usize) -> usize {
    let mut guard = lock_cursors();
    let cursors = guard.get_or_insert_with(HashMap::new);
    if cursors.len() >= MAX_CURSORS && !cursors.contains_key(key) {
        cursors.clear();
    }
    let entry = cursors.entry(key.to_string()).or_insert(0);
    let start = (*entry as usize) % len;
    *entry = entry.wrapping_add(1);
    start
}

// ─── 配置读取 ───────────────────────────────────────────────

/// `all` 的读法：缺键 / null 当**真**。
///
/// 与写入侧（归一恒写显式布尔）配起来，「缺键」只可能来自手工改库；那时按
/// 「全部」解释比按「一条都不轮」解释安全 —— 前者是这条配置的默认语义，
/// 后者会让转发直接回退直连。
fn is_all(config: &Value) -> bool {
    match config.get("all") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(flag)) => *flag,
        // 脏值（字符串 / 数字）按「不是 false 即为真」的 JS 口径：只认显式 false
        Some(_) => true,
    }
}

/// 配置里显式列出的 id（`all: false` 那一支；去空、去重、保序、截断到 [`MAX_IDS`]）
fn configured_ids(config: &Value) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let Some(items) = config.get("proxyIds").and_then(Value::as_array) else {
        return ids;
    };
    for item in items {
        let Some(text) = item.as_str() else { continue };
        let id = text.trim();
        if id.is_empty() || ids.iter().any(|existing| existing == id) {
            continue;
        }
        ids.push(id.to_string());
        if ids.len() >= MAX_IDS {
            break;
        }
    }
    ids
}

fn text_of(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// 轮询候选的条目 id（按配置取，见模块头）。
///
/// `all: true` → 池里**全部启用**条目（禁用项不参与：它在池里就是「先别用它」
/// 的意思，而轮询是逐请求选路，没有「这次先跳过」的余地 —— 硬选中它只会每次
/// 都解析失败再顺延，多绕一圈）。顺序即池的存储顺序（新加的在前）。
pub fn candidate_ids(config: &Value) -> Vec<String> {
    if !is_all(config) {
        return configured_ids(config);
    }
    crate::server::core::proxy_pool::raw_items()
        .iter()
        .filter(|item| is_enabled(item))
        .filter_map(|item| {
            let id = text_of(item, "id");
            if id.is_empty() { None } else { Some(id) }
        })
        .collect()
}

/// 条目的**廉价**可用性（不发网络）：用于展示摘要里的
/// 「可用 N 条」。真正的取用判定在解析那一步（`proxy_pool::resolve_reference`）。
fn entry_ready(item: &Value) -> Result<(), String> {
    if item.get("source").and_then(Value::as_str) == Some("clash") {
        return match crate::server::core::proxy_pool::resolve_item(item) {
            Some(crate::server::core::proxies::ProxyResolution::Resolved(_)) => Ok(()),
            Some(crate::server::core::proxies::ProxyResolution::Failed(reason)) => Err(reason),
            None => Err("条目地址不完整".to_string()),
        };
    }
    let host = text_of(item, "host");
    let port = item.get("port").and_then(Value::as_u64).unwrap_or(0);
    if host.is_empty() || !(1..=65535).contains(&port) {
        return Err("条目地址不完整".to_string());
    }
    Ok(())
}

// ─── 取用 ───────────────────────────────────────────────────

/// 从 `start` 起绕一圈找第一条能解析的候选；一条都解析不出来时给出**第一条的
/// 原因**（它最有代表性：坏在哪，用户第一条就该看到那句话）+ 一句总起。
///
/// `ids` 由调用方算好传进来（候选要读一次池，一条请求只该读一次）。
fn pick_at(ids: &[String], start: usize, all: bool) -> Result<ResolvedProxy, String> {
    if ids.is_empty() {
        return Err(if all {
            "轮询没有可用出口：「网络代理」页还没有启用的代理".to_string()
        } else {
            "轮询没有可用出口：账号设置里选中的代理都不在了（可能已被删除）".to_string()
        });
    }
    let mut first_error: Option<String> = None;
    for offset in 0..ids.len() {
        let id = &ids[(start + offset) % ids.len()];
        match crate::server::core::proxy_pool::resolve_reference(id) {
            Ok(proxy) => return Ok(proxy),
            Err(reason) => {
                if first_error.is_none() {
                    first_error = Some(reason);
                }
            }
        }
    }
    Err(format!(
        "轮询的出口都不可用：{}",
        first_error.unwrap_or_else(|| "没有可用的代理".to_string())
    ))
}

/// 轮询取下一个出口（游标按 `key` 前进；`key` = 账号 id）。
///
/// `key` 为空串时退化为「第一条可用」：没有可归属的账号身份就谈不上轮询进度，
/// 此时确定性优先（同一个调用方每次拿到同一条，便于复现与排障）。
pub fn next(config: &Value, key: &str) -> Result<ResolvedProxy, String> {
    let ids = candidate_ids(config);
    // 空候选时不必取号（`take_cursor` 要求 len 非 0）——直接由 pick_at 报空
    let start = if ids.is_empty() || key.is_empty() {
        0
    } else {
        take_cursor(key, ids.len())
    };
    pick_at(&ids, start, is_all(config))
}

/// 不前进游标的静态解析：给**展示**与**非转发**链路用（描述账号代理、
/// 余额查询这类「一次调用不代表一次转发」的地方）。
///
/// 取第一条可用条目 —— 它回答的是「这条配置现在能不能用、大概走哪儿」，
/// 而不是「这次该走哪儿」。
pub fn first_usable(config: &Value) -> Result<ResolvedProxy, String> {
    let ids = candidate_ids(config);
    pick_at(&ids, 0, is_all(config))
}

// ─── 展示摘要（账号列表 / 日志文案用，不动游标）───────────────

/// 摘要读数：候选条数与其中可用的条数（判据见 [`entry_ready`]）
pub struct Summary {
    /// 是否「全部出口」档
    pub all: bool,
    /// 候选条数（all=false 时只数**还存在**的条目）
    pub total: usize,
    /// 其中看起来可用的条数（廉价判据，见 [`entry_ready`]）
    pub usable: usize,
    /// 一条候选都不可用时的原因（可用时为 None）
    pub error: Option<String>,
}

/// 算一份摘要（读一次池，不做网络请求）。
pub fn summary(config: &Value) -> Summary {
    let all = is_all(config);
    let items = crate::server::core::proxy_pool::raw_items();
    let mut total = 0usize;
    let mut usable = 0usize;
    let mut consider = |item: &Value| {
        total += 1;
        if is_enabled(item) && entry_ready(item).is_ok() {
            usable += 1;
        }
    };
    if all {
        for item in items.iter().filter(|item| is_enabled(item)) {
            consider(item);
        }
    } else {
        for id in configured_ids(config) {
            if let Some(item) = items
                .iter()
                .find(|item| text_of(item, "id") == id)
            {
                consider(item);
            }
        }
    }
    let error = if total == 0 {
        Some(if all {
            "「网络代理」页还没有启用的代理".to_string()
        } else {
            "轮询里选中的代理都不在了（可能已被删除）".to_string()
        })
    } else if usable == 0 {
        Some("轮询里的代理都不可用（已禁用或未就绪）".to_string())
    } else {
        None
    };
    Summary { all, total, usable, error }
}
