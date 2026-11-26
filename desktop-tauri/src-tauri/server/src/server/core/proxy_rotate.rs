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

fn candidate_ids_from(config: &Value, items: &[Value]) -> Vec<String> {
    if !is_all(config) {
        return configured_ids(config);
    }
    items
        .iter()
        .filter(|item| is_enabled(item) && entry_ready(item).is_ok())
        .filter_map(|item| {
            let id = text_of(item, "id");
            if id.is_empty() { None } else { Some(id) }
        })
        .collect()
}

/// 轮询候选的条目 id（按配置取，见模块头）。
///
/// `all: true` → 池里**全部启用且就绪**条目（禁用或失联未就绪的条目不参与：
/// 失联的 EasyClash 代理没有可用端口，硬选中只会每次都解析失败再顺延，
/// 且会在代理页被误标成被全部轮询账号引用）。顺序即池的存储顺序（新加的在前）。
pub fn candidate_ids(config: &Value) -> Vec<String> {
    if !is_all(config) {
        return configured_ids(config);
    }
    let items = crate::server::core::proxy_pool::raw_items();
    candidate_ids_from(config, &items)
}

/// 条目的**廉价**可用性（不发网络）：用于展示摘要里的
/// 「可用 N 条」与全部轮询候选过滤。真正的取用判定在解析那一步（`proxy_pool::resolve_reference`）。
pub(crate) fn entry_ready(item: &Value) -> Result<(), String> {
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

fn empty_all_error(items: &[Value]) -> String {
    let mut has_enabled = false;
    let mut first_reason: Option<String> = None;
    for item in items.iter().filter(|item| is_enabled(item)) {
        has_enabled = true;
        if first_reason.is_none() {
            if let Err(reason) = entry_ready(item) {
                first_reason = Some(reason);
            }
        }
    }
    if !has_enabled {
        return "轮询没有可用出口：「网络代理」页还没有启用的代理".to_string();
    }
    match first_reason {
        Some(reason) => format!("轮询没有可用出口：已启用的代理当前均不可用（{reason}）"),
        None => "轮询没有可用出口：已启用的代理当前均不可用（请检查「网络代理」页）".to_string(),
    }
}

fn resolve_rotate(config: &Value, key: Option<&str>) -> Result<ResolvedProxy, String> {
    let all = is_all(config);
    let ids = if all {
        let items = crate::server::core::proxy_pool::raw_items();
        let ids = candidate_ids_from(config, &items);
        if ids.is_empty() {
            return Err(empty_all_error(&items));
        }
        ids
    } else {
        configured_ids(config)
    };
    let start = match key {
        Some(k) if !ids.is_empty() && !k.is_empty() => take_cursor(k, ids.len()),
        _ => 0,
    };
    pick_at(&ids, start, all)
}

/// 轮询取下一个出口（游标按 `key` 前进；`key` = 账号 id）。
///
/// `key` 为空串时退化为「第一条可用」：没有可归属的账号身份就谈不上轮询进度，
/// 此时确定性优先（同一个调用方每次拿到同一条，便于复现与排障）。
pub fn next(config: &Value, key: &str) -> Result<ResolvedProxy, String> {
    resolve_rotate(config, Some(key))
}

/// 不前进游标的静态解析：给**展示**与**非转发**链路用（描述账号代理、
/// 余额查询这类「一次调用不代表一次转发」的地方）。
///
/// 取第一条可用条目 —— 它回答的是「这条配置现在能不能用、大概走哪儿」，
/// 而不是「这次该走哪儿」。
pub fn first_usable(config: &Value) -> Result<ResolvedProxy, String> {
    resolve_rotate(config, None)
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
    let items = crate::server::core::proxy_pool::raw_items();
    summary_from(config, &items)
}

fn summary_from(config: &Value, items: &[Value]) -> Summary {
    let all = is_all(config);
    let mut total = 0usize;
    let mut usable = 0usize;
    let mut has_enabled = false;
    if all {
        for item in items.iter().filter(|item| is_enabled(item)) {
            has_enabled = true;
            if entry_ready(item).is_ok() && !text_of(item, "id").is_empty() {
                total += 1;
                usable += 1;
            }
        }
    } else {
        for id in configured_ids(config) {
            if let Some(item) = items
                .iter()
                .find(|item| text_of(item, "id") == id)
            {
                total += 1;
                if is_enabled(item) && entry_ready(item).is_ok() {
                    usable += 1;
                }
            }
        }
    }
    let error = if total == 0 {
        Some(if all {
            if has_enabled {
                "轮询里的代理都不可用（已禁用或未就绪）".to_string()
            } else {
                "「网络代理」页还没有启用的代理".to_string()
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manual_item(id: &str, enabled: bool) -> Value {
        json!({
            "id": id,
            "name": id,
            "source": "manual",
            "protocol": "http",
            "host": "127.0.0.1",
            "port": 7890,
            "enabled": enabled,
        })
    }

    fn broken_clash_item(id: &str) -> Value {
        json!({
            "id": id,
            "name": "Broken Clash",
            "source": "clash",
            "listenerUid": "__missing_listener_for_rotate_test__",
            "enabled": true,
        })
    }

    #[test]
    fn rotate_all_excludes_unready_entries_while_explicit_keeps_them() {
        let items = vec![
            manual_item("px_ok", true),
            manual_item("px_off", false),
            broken_clash_item("px_broken_clash"),
        ];

        let all_cfg = json!({ "source": "rotate", "all": true });
        assert_eq!(candidate_ids_from(&all_cfg, &items), vec!["px_ok"]);
        let all_summary = summary_from(&all_cfg, &items);
        assert_eq!(all_summary.total, 1);
        assert_eq!(all_summary.usable, 1);
        assert_eq!(all_summary.error, None);

        let only_broken = vec![broken_clash_item("px_broken_clash")];
        assert!(candidate_ids_from(&all_cfg, &only_broken).is_empty());
        let broken_summary = summary_from(&all_cfg, &only_broken);
        assert_eq!(broken_summary.total, 0);
        assert_eq!(broken_summary.usable, 0);
        assert_eq!(
            broken_summary.error.as_deref(),
            Some("轮询里的代理都不可用（已禁用或未就绪）")
        );
        assert!(empty_all_error(&only_broken).contains("已启用的代理当前均不可用"));

        let empty_summary = summary_from(&all_cfg, &[]);
        assert_eq!(
            empty_summary.error.as_deref(),
            Some("「网络代理」页还没有启用的代理")
        );
        assert!(empty_all_error(&[]).contains("还没有启用的代理"));

        let explicit_cfg = json!({
            "source": "rotate",
            "all": false,
            "proxyIds": ["px_broken_clash"],
        });
        assert_eq!(candidate_ids_from(&explicit_cfg, &items), vec!["px_broken_clash"]);
        let explicit_summary = summary_from(&explicit_cfg, &items);
        assert_eq!(explicit_summary.total, 1);
        assert_eq!(explicit_summary.usable, 0);
        assert!(explicit_summary.error.is_some());
    }
}
