//! WorkBuddy 国际版日活保活的**模型链配置**。
//!
//! ── 为什么它是一份独立配置 ──────────────────────────────────
//! 保活走免费模型链（见 `providers::workbuddy::keepalive::DEFAULT_FREE_MODELS`），
//! 但免费模型是上游运营态的事实：模型下线、改名、新增，都可能让写死的那条链
//! 整条失效 —— 保活失败一天，国际版的活跃奖励就断一天。所以链路开放给用户
//! 自定义：签到中心「自动签到」卡里直接编辑，落 config.json 的
//! `checkinKeepalive.models`。
//!
//! ── 读口径 ─────────────────────────────────────────────────
//! 两种落盘形态都认：对象 `{"models": […]}`（写入侧落的就是这一种）与裸数组
//! `[…]`（旧形态 / 手改配置）。缺失 / 非上述形态 / 全是空串都回落缺省链 ——
//! 与 `auto_checkin::normalize_providers` 同一条兜底哲学：旧 config.json 里没有
//! 这个字段，读出来必须是合法默认行为。归一化逐项 trim、丢空串、去重（保序）：
//! 用户在输入框里写的多半是「逗号/空格分隔的一行」，api 层拆好数组再进来，
//! 这里做最后一道净化。

use serde_json::{Value, json};

use crate::server::config;
use crate::server::logging;

/// config.json 里承载保活模型链的键
const CONFIG_KEY: &str = "checkinKeepalive";

/// 缺省模型链（与 `providers::workbuddy::keepalive::DEFAULT_FREE_MODELS` 同源）
pub fn default_models() -> Vec<String> {
    crate::server::core::providers::workbuddy::keepalive::DEFAULT_FREE_MODELS
        .iter()
        .map(|model| (*model).to_string())
        .collect()
}

/// 归一化一份模型清单：逐项 trim、丢空串、去重（保序）。
fn normalize(list: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::with_capacity(list.len());
    for item in list {
        let model = item.trim();
        if model.is_empty() || seen.iter().any(|existing| existing == model) {
            continue;
        }
        seen.push(model.to_string());
    }
    seen
}

/// 当前生效的保活模型链（配置缺失 / 非法时回落缺省链）。
///
/// ── 两种落盘形态都认（修复 #183）────────────────────────────
/// 写入侧（[`set_models`]）落的是**对象** `{"models": […]}`，而这里早先只按
/// **数组**解析 —— 存进去的是对象、读回来按数组匹配，`Value::Array` 永远不命中，
/// 于是每次读都回落缺省链：界面表现为「保存成功、日志也打成功，但输入框和重启后
/// 都还是原链路」，落库实际上是对的。现在对象优先取 `models` 键，数组（旧形态 /
/// 手改配置）照收，两者之外一律回落缺省。
pub fn models() -> Vec<String> {
    // 先绑定快照再取键：`config::current()` 返回的临时值必须活到借用结束
    let snapshot = config::current();
    let Some(raw) = snapshot.raw().get(CONFIG_KEY) else {
        return default_models();
    };
    let items = match raw {
        Value::Array(items) => items.clone(),
        Value::Object(_) => raw
            .get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => return default_models(),
    };
    let picked = normalize(
        &items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>(),
    );
    if picked.is_empty() {
        default_models()
    } else {
        picked
    }
}

/// 保存保活模型链。**空清单 = 恢复缺省**：用户把输入框清空的意思是
/// 「我不确定该填什么」，替他回到出厂链比把保活关掉更接近本意
/// （保活链为空等于国际版日活永远只剩保活失败，那不该是一句配置就能造成的）。
/// 返回归一化后的清单（写盘失败返回 Err，调用方把原因吐给界面）。
pub fn set_models(list: &[String]) -> Result<Vec<String>, String> {
    let picked = normalize(list);
    let value: Vec<String> = if picked.is_empty() { default_models() } else { picked };
    config::update_raw_field(
        CONFIG_KEY,
        json!({ "models": value }),
    );
    logging::log(
        "[Checkin]",
        &format!("WorkBuddy 国际版保活模型链已改为 {}", value.join(" → ")),
    );
    Ok(value)
}

/// 对外状态（签到中心快照的 `keepalive` 段与保存响应共用这一份）：
/// `models` 是当前生效链，`defaultModels` 给界面提示"清空即恢复这份"。
pub fn state() -> Value {
    json!({
        "models": models(),
        "defaultModels": default_models(),
    })
}
