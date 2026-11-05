//! WorkBuddy 国际版日活保活的**模型链配置**。
//!
//! ── 为什么它是一份独立配置 ──────────────────────────────────
//! 保活走免费模型链，而免费模型是上游运营态的事实：模型下线、改名、新增，都可能
//! 让那条链整条失效 —— 保活失败一天，国际版的活跃奖励就断一天。所以链路开放给
//! 用户自定义：签到中心「自动签到」卡里直接编辑，落 config.json 的
//! `checkinKeepalive.models`。
//!
//! ── 缺省链在插件里（2026-10 插件化）─────────────────────────
//! 「上游此刻有哪些免费模型」是上游知识，随实现搬到提供商插件了。因此这里
//! 的缺省值是**空**：用户没配时网关不下发 `models`，由插件用它自己的缺省链 ——
//! 网关不再持有那份会过时的名单。用户配了就用用户的（那是纯配置）。
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

/// 缺省模型链：**空** —— 由插件用自己的那份（见模块头）。
///
/// 保留这个函数是为了让 `models()` 的「回落缺省」分支有具名出口：网关手里
/// 没有「哪些模型免费」的事实，空数组就是它该说的话。
pub fn default_models() -> Vec<String> {
    Vec::new()
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
///
/// ── API Key 的缺省：取第一把可用 Key ────────────────────────
/// 改造前不选 Key 就是 `None`，而 `authorized_models()` 对 `None` 直接报
/// 「请先选择 API Key」—— 用户必须在签到中心手动挑一次才能让保活跑起来，
/// 而界面上那句提示又只在保存后才出现。现在缺省落到 [`first_key_id`]：
/// 有可用 Key 就直接用它，用户之后想换再换。显式传 `null` 仍表示「不选」，
/// 那是用户清空下拉的动作，必须尊重。
pub fn set_models(list: &[String], api_key_id: Option<Option<String>>) -> Result<Vec<String>, String> {
    let selected = api_key_id.unwrap_or_else(|| binding_id().or_else(first_key_id));
    if let Some(id) = selected.as_deref() {
        access_for_id(id).map_err(|error| error.message)?;
    }
    let picked = normalize(list);
    let value: Vec<String> = if picked.is_empty() { default_models() } else { picked };
    config::update_raw_field(
        CONFIG_KEY,
        json!({ "models": value, "apiKeyId": selected }),
    );
    logging::log(
        "[Checkin]",
        &format!("WorkBuddy 国际版保活模型链已改为 {}", value.join(" → ")),
    );
    Ok(value)
}

/// 第一把**可用**的网关 Key（缺省绑定用）。
///
/// 顺序与「网关 Key」页一致（`api_keys::list()` 的落盘顺序 = 创建顺序），
/// 跳过停用 / 解密失败的记录。环境变量 Key 排在最后：它没有记录、也没有
/// 白名单，用户建了真 Key 时优先用真的那把。
fn first_key_id() -> Option<String> {
    use crate::server::core::api_keys;
    if let Some(entry) = api_keys::list().into_iter().find(api_keys::ApiKeyEntry::is_usable) {
        return Some(entry.id);
    }
    crate::server::config::parse::env_api_key_value().map(|_| "env".to_string())
}

pub fn binding_id() -> Option<String> {
    config::current().raw().get(CONFIG_KEY).and_then(|value| value.get("apiKeyId"))
        .and_then(Value::as_str).map(str::trim).filter(|id| !id.is_empty()).map(str::to_string)
}

/// 实际生效的 Key：显式绑定的那把；没绑过就是第一把可用 Key。
///
/// 读侧（`state` / `authorized_models`）一律走这里，写侧（`set_models`）在用户
/// 没指定时也落到它 —— 于是「界面显示哪把」与「保活实际用哪把」永远同一个答案。
pub fn effective_key_id() -> Option<String> {
    binding_id().or_else(first_key_id)
}

fn access_for_id(id: &str) -> Result<(crate::server::core::key_scope::KeyScope, crate::server::request_stats::ApiKeyIdentity), crate::server::errors::GatewayError> {
    use crate::server::core::{api_keys, key_scope::KeyScope};
    use crate::server::errors::GatewayError;
    use crate::server::request_stats::ApiKeyIdentity;
    if id == "env" && config::parse::env_api_key_value().is_some() {
        return Ok((KeyScope::default(), ApiKeyIdentity { source: "env".to_string(), ..Default::default() }));
    }
    let entry = api_keys::list().into_iter().find(|entry| entry.id == id && entry.is_usable())
        .ok_or_else(|| GatewayError::with_status(401, "保活 API Key 已停用、删除或不可用，请重新选择"))?;
    Ok((KeyScope::from_entry(&entry), ApiKeyIdentity { id: entry.id, name: entry.name, source: "key".to_string() }))
}

pub fn authorized_models() -> Result<(Vec<String>, crate::server::request_stats::ApiKeyIdentity), crate::server::errors::GatewayError> {
    use crate::server::errors::GatewayError;
    let id = effective_key_id().ok_or_else(|| GatewayError::with_status(401, "日活保活会调用模型，请先在「网关 Key」页创建一把 Key（或设置环境变量 Key）"))?;
    let (scope, identity) = access_for_id(&id)?;
    if !scope.allows_provider("workbuddy-intl") {
        return Err(GatewayError::with_status(403, "保活 API Key 不允许 WorkBuddy 国际版"));
    }
    let models = models();
    if models.is_empty() && scope.restricts_models() {
        return Err(GatewayError::with_status(403, "受限 API Key 的保活模型链不能为空，请明确填写获准使用的模型"));
    }
    if models.iter().any(|model| !scope.allows_model(model)) {
        return Err(GatewayError::with_status(403, "保活模型链包含所选 API Key 未授权的模型"));
    }
    Ok((models, identity))
}

static REQUEST_STATS: std::sync::OnceLock<std::sync::Arc<crate::server::request_stats::RequestStats>> = std::sync::OnceLock::new();

pub fn install_stats(stats: std::sync::Arc<crate::server::request_stats::RequestStats>) {
    let _ = REQUEST_STATS.set(stats);
}

pub fn record_activity(
    account_id: &str,
    started_at: i64,
    payload: &Value,
    result: &Result<Value, crate::server::errors::GatewayError>,
    identity: crate::server::request_stats::ApiKeyIdentity,
) {
    use crate::server::request_stats::{NewRequestEntry, RequestSource};
    let Some(stats) = REQUEST_STATS.get() else { return };
    let body = result.as_ref().ok();
    let model = body.and_then(|value| value.pointer("/activity/pokeModel")).and_then(Value::as_str)
        .or_else(|| body.and_then(|value| value.pointer("/activity/webConversation/model")).and_then(Value::as_str)).unwrap_or("");
    let success = result.as_ref().ok().is_some_and(|value| {
        value.pointer("/activity/pokeSucceeded").and_then(Value::as_bool) == Some(true)
            || value.pointer("/activity/webConversation/succeeded").and_then(Value::as_bool) == Some(true)
    });
    let mut entry = NewRequestEntry::new(model, if success { 200 } else { 502 });
    entry.ts = Some(started_at);
    entry.id = format!("keepalive-{account_id}-{started_at}");
    entry.account_id = account_id.to_string();
    entry.provider = Some("workbuddy-intl".to_string());
    entry.client_model = payload.get("models").and_then(Value::as_array)
        .map(|models| models.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("、")).unwrap_or_default();
    entry.upstream_model = model.to_string();
    entry.duration_ms = logging::now_ms() - started_at;
    entry.is_test = true;
    entry.source = RequestSource { client: "checkin_keepalive".to_string(), api_key: identity, ..Default::default() };
    if !success {
        entry.status = result.as_ref().err().map(|error| i64::from(error.http_status().as_u16())).unwrap_or(502);
        entry.error = Some(result.as_ref().err().map(|error| error.message.clone()).unwrap_or_else(|| "日活模型保活未成功，详情见签到记录".to_string()));
    }
    stats.record(entry);
}

/// 对外状态（签到中心快照的 `keepalive` 段与保存响应共用这一份）：
/// `models` 是当前生效链，`defaultModels` 给界面提示"清空即恢复这份"。
pub fn state() -> Value {
    json!({
        "models": models(),
        "defaultModels": default_models(),
        // 生效的 Key（显式绑定 > 第一把可用 Key）：界面显示的那把就是保活真用的
        "apiKeyId": effective_key_id(),
        // 是否只是缺省选中（界面可以提示「用的是默认 Key」）
        "apiKeyDefaulted": binding_id().is_none() && effective_key_id().is_some(),
        "authError": authorized_models().err().map(|error| error.message),
    })
}
