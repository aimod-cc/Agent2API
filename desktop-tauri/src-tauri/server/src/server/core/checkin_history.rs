//! 签到历史台账 —— 签到中心「最近签到记录」的数据源。
//!
//! ── 为什么需要它 ────────────────────────────────────────────
//! 定时签到只有 `autoCheckin.lastResult` 一条（被下一轮覆盖），账号页只能看到
//! 「最后一次」的结果；签到中心的时间线要回看多轮（定时 / 手动 / 启动补签），
//! 所以把每轮批量签到的汇总追加进 config.json 的 `checkinHistory`。
//!
//! ── 只记批量轮次（id=None），单账号签到不记 ─────────────────
//! 时间线记录的是「一轮」的成败总览；单账号签到是账号页 / 明细行上的点操作，
//! 它的效果已经通过 `checkinAt` 反映在每日签到的账号状态里，再进时间线只会
//! 把「今天 09:12 签了浣熊-C」这种碎行混进轮次记录里。
//!
//! ── 为什么放 core 而不是 api ────────────────────────────────
//! 记账点是 `billing::checkin::run_checkin`（定时签到与手动批量共用那一段执行体），
//! 它在 core 里；`core::auto_checkin` 直接读写 config 已是先例（见它的模块头
//! 「配置读写」），本模块沿用同一模式：`config::update_raw_field` 合并写回，
//! config.json 里的其它字段原样保留。
//!
//! ── 容量 ────────────────────────────────────────────────────
//! 保留最近 [`MAX_ENTRIES`]（20）条：一天最多定时 1 轮 + 手动若干轮，20 条
//! 足够回看两三天；再大只会把 config.json 撑大，没有查询价值。

use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::auto_checkin;
use crate::server::logging;

/// config.json 里承载签到历史的键
const CONFIG_KEY: &str = "checkinHistory";
/// 台账保留条数（新条目在前，超出裁掉最老的）
pub const MAX_ENTRIES: usize = 20;

/// 读台账（新在前）。非数组 / 缺失都给空表 —— 与 auto_checkin 的 raw_object
/// 同一兜底口径：旧 config.json 里没有这个字段，读出来必须是合法默认值。
pub fn list() -> Vec<Value> {
    match config::current().raw().get(CONFIG_KEY) {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// 追加一轮签到汇总。`summary` 是 `run_checkin` 的返回值（含 results），
/// 这里只挑界面要展示的字段重组成一行，`results` 明细不落台账 ——
/// 失败明细的展示口径与 `autoCheckin.lastResult` 一致（`名字（错误）`，取前 5 条），
/// 两处同构意味着签到中心与定时任务卡看到的是同一份失败解释。
///
/// 失败只记日志、不向上传播：台账写不进去不影响这次签到本身的结果
/// （积分已经在上游领到了）。
pub fn record(summary: &Value, reason: &str) {
    let number = |key: &str| summary.get(key).and_then(Value::as_u64).unwrap_or(0);
    let failures: Vec<String> = summary
        .get("results")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let error = item.get("error").and_then(Value::as_str)?;
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .or_else(|| item.get("id").and_then(Value::as_str))
                        .unwrap_or("未知账号");
                    Some(format!("{name}（{error}）"))
                })
                .collect()
        })
        .unwrap_or_default();
    let now_ms = logging::now_ms();
    let entry = json!({
        "at": now_ms,
        // 日期键按北京时间（上游自然日口径，见 core::beijing）：台账的
        // 「今天」与签到的「今天」必须是同一天，否则海外部署下时间线会错位
        "date": auto_checkin::today_key(),
        "reason": reason,
        "succeeded": number("succeeded"),
        // 活跃保活数（WorkBuddy 国际版）：与 succeeded 分开统计 —— 保活的
        // 成功不落 checkinAt，台账里也要单独一列，时间线才读得出这一轮
        // 「国际版那边发生了什么」
        "active": number("active"),
        "total": number("total"),
        "skipped": number("skipped"),
        "failed": failures.iter().take(5).cloned().collect::<Vec<_>>(),
        "failedCount": failures.len(),
    });
    let mut entries = list();
    entries.insert(0, entry);
    entries.truncate(MAX_ENTRIES);
    write(entries);
}

/// 合并写回 `checkinHistory`（与 auto_checkin::write_state 同一模式：
/// `update_raw_field` 按键直写，config.json 里的其它字段原样保留）
fn write(entries: Vec<Value>) {
    config::update_raw_field(CONFIG_KEY, Value::Array(entries));
}
