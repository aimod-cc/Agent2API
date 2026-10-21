//! AtomCode 的模型目录：`coding-plan/models-v2` 是**唯一权威**来源。
//!
//! ── 三层结构（照官方与本地 Go 参考实现）─────────────────────────
//! ```text
//!   1. GET {codingplan}/coding-plan/status-v2
//!        -> 套餐名（"CodingPlan Lite-体验版" / "… Pro" / "… Max"）
//!           + 滚动额度窗口（额度展示用，见 usage.rs）
//!   2. 由套餐名推断档位（Max / Pro / Lite）——models-v2 的 `plan_available`
//!      是**相对被问的那个档位**算的，问错档位就会把一个能用的模型过滤掉
//!      （官方 `types.rs:53-70` 的映射是「从最具体到最不具体」的子串匹配）
//!   3. GET {codingplan}/coding-plan/models-v2?plan_type=<档位>
//!        -> 条目数组；只注册 `plan_available == true` 的
//! ```
//!
//! ── 条目里哪个字段才是「发给网关的模型名」（最容易搞错的一处）────
//! `id` 是**数字**（AtomGit 内部模型 id），**不是**模型名；网关认的是
//! `display_model_name`（形如 `glm5.3-flash`）。本地 Go 参考在
//! `models.go:399-401` 专门为这件事留了一句注释：
//!
//! > id (the `glm5.3-flash` spelling it answers to), while `display` is the …
//!
//! 拿数字 id 去发请求不会报错 —— 网关回 `HTTP 200 + 正文 参数错误`
//! （见 `errors.rs` 的模块头）。
//!
//! ── 名义目录 ≠ 可达目录（实测，所以不做兜底清单）───────────────
//! 2026-10-01 同一个 `CodingPlan Lite-体验版` 账号上逐个 `POST /chat/completions`：
//!
//! ```text
//!   deepseek-flash                200 真回答   （**不在** models-v2 名单里）
//!   Qwen/Qwen3-32B                200 真回答   （不在名单里）
//!   Qwen/Qwen2-VL-72B             200 真回答   （不在名单里）
//!   Qwen/Qwen3-4B-Instruct-2507   200 正文是 `三方请求失败: 502 <html>…`
//!   no-such-model-xyz             200 正文是 `参数错误`
//! ```
//!
//! 所以「可达」不等于「被广告」、更不等于「能用」。本模块**只广告
//! `plan_available` 的条目**，不内置任何兜底清单 —— 把上面那几个不可达的写死
//! 进来，用户点一下就会看到「参数错误」或一页 HTML（对比：Loomy 也是
//! 「拉不到就如实报错，不编模型名」）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use serde_json::{Map, Value, json};

use crate::server::core::auth_http::send_raw;
use crate::server::core::providers::adapter::ModelRefreshOutcome;
use crate::server::core::providers::catalog_cache;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::credentials::AtomcodeCredentials;
use super::endpoints::{
    CATALOG_TIMEOUT_MS, CODINGPLAN_CLAIM_PATH, REQUEST_TIMEOUT_MS, codingplan_base_url,
    models_url, status_url,
};

/// 本家在 `catalog_cache` 里的 scope 名
pub const SCOPE: &str = catalog_cache::SCOPE_ATOMCODE;

/// 目录缓存时长（与官方 CLI 的 2 小时同档）
const CATALOG_TTL_MS: i64 = 2 * 60 * 60 * 1000;

/// 目录里的一条模型
#[derive(Clone, Debug)]
pub struct ModelEntry {
    /// 发给网关的模型名（`display_model_name`，如 `glm5.3-flash`）
    pub gateway_id: String,
    /// 展示名（本网关给用户看的名字）
    pub display_name: String,
    /// 上下文窗口（上游没给 = 0，不猜）
    pub context_window: i64,
    /// 是否收图片（上游没给 = None，**不猜**）
    pub supports_vision: Option<bool>,
    /// 上游声明的思考档位（空 = 上游没说支持思考）
    pub reasoning_levels: Vec<String>,
}

/// 进程内目录状态
#[derive(Clone, Debug, Default)]
struct CatalogState {
    entries: Vec<ModelEntry>,
    fetched_at: i64,
}

static STATE: OnceLock<Mutex<Option<CatalogState>>> = OnceLock::new();

/// 取状态句柄（毒锁按「上一个持有者 panic 过、数据可能半更新」处理：
/// 这份缓存是可重建的展示数据，不值得让进程挂掉 —— 与本仓其它目录模块同口径）
fn state() -> &'static Mutex<Option<CatalogState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

/// 在锁内执行一段取值，并在首次访问时把落盘缓存装回内存
fn with_state<R>(f: impl FnOnce(&mut Option<CatalogState>) -> R) -> R {
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_none() {
        if let Some(cached) = catalog_cache::load(SCOPE) {
            let entries: Vec<ModelEntry> = cached
                .models
                .iter()
                .filter_map(entry_from_cached)
                .collect();
            if !entries.is_empty() {
                *guard = Some(CatalogState {
                    entries,
                    fetched_at: cached.fetched_at,
                });
            }
        }
    }
    f(&mut guard)
}

/// 从落盘缓存条目还原（缓存里存的是 [`to_cached_value`] 写下的形态）
fn entry_from_cached(value: &Value) -> Option<ModelEntry> {
    let gateway_id = value.get("model").and_then(Value::as_str)?.trim().to_string();
    if gateway_id.is_empty() {
        return None;
    }
    Some(ModelEntry {
        gateway_id,
        display_name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        context_window: value.get("contextWindow").and_then(Value::as_i64).unwrap_or(0),
        supports_vision: value.get("supportsImages").and_then(Value::as_bool),
        reasoning_levels: value
            .get("reasoningLevels")
            .and_then(Value::as_array)
            .map(|levels| {
                levels
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// 把目录条目落盘时的形态（key 用本仓惯用的驼峰，便于人工看）
fn to_cached_value(entry: &ModelEntry) -> Value {
    let mut object = Map::new();
    object.insert("model".to_string(), Value::String(entry.gateway_id.clone()));
    object.insert("name".to_string(), Value::String(entry.display_name.clone()));
    if entry.context_window > 0 {
        object.insert("contextWindow".to_string(), Value::from(entry.context_window));
    }
    if let Some(vision) = entry.supports_vision {
        object.insert("supportsImages".to_string(), Value::Bool(vision));
    }
    if !entry.reasoning_levels.is_empty() {
        object.insert(
            "reasoningLevels".to_string(),
            Value::Array(
                entry
                    .reasoning_levels
                    .iter()
                    .map(|level| Value::String(level.clone()))
                    .collect(),
            ),
        );
    }
    Value::Object(object)
}

/// 网关名 → 展示名。只覆盖**上游名字对用户不够清楚**的那几个：
/// 官方客户端自己也承认 `glm5.3-flash` 就是 z.ai 的 GLM-5.3-Flash
/// （本地 Go 参考 `models.go:262-263` 记了 2026-09-25 的实测）。
///
/// 表里没有的名字**原样展示**，不猜、不套模板（`deepseek-flash` 这类
/// 上游自己给的形态已经很清楚了）。
pub fn display_name_for(gateway_id: &str) -> String {
    match gateway_id {
        "glm5.3-flash" => "GLM-5.3-Flash".to_string(),
        "qwen3.8-27b" => "Qwen3.8-27B".to_string(),
        "deepseek-flash" => "DeepSeek-V4.1-Flash".to_string(),
        other => other.to_string(),
    }
}

/// 把一条 `models-v2` 条目映射成本模块的形态。
///
/// 只注册 `plan_available == true` 的（服务端自己的权益判定）；
/// 缺 `display_model_name` 的条目直接丢掉 —— 没有它就发不出请求。
fn entry_from_upstream(value: &Value) -> Option<ModelEntry> {
    if !value.get("plan_available").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let gateway_id = value
        .get("display_model_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if gateway_id.is_empty() {
        return None;
    }
    let context_window = value
        .get("context_window")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(0);
    let supports_vision = value.get("supports_vision").and_then(Value::as_bool);
    let reasoning_levels = value
        .get("reasoning_effort_levels")
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(ModelEntry {
        display_name: display_name_for(&gateway_id),
        gateway_id,
        context_window,
        supports_vision,
        reasoning_levels,
    })
}

/// 聚合层认的条目形态（键名对照 `core/models/shape.rs::list_item`）。
///
/// 这里直接给出**聚合层的输入键**（`id` / `name` / `maxInputTokens` / …），
/// 由 `list_item` 再折成 `/v1/models` 的 wire 形态 —— 与 loomy 等家同一条路。
fn to_manifest_value(entry: &ModelEntry) -> Value {
    let mut object = Map::new();
    object.insert("id".to_string(), Value::String(entry.gateway_id.clone()));
    if !entry.display_name.is_empty() {
        object.insert("name".to_string(), Value::String(entry.display_name.clone()));
    }
    if entry.context_window > 0 {
        object.insert("maxInputTokens".to_string(), Value::from(entry.context_window));
    }
    // 模态位：上游**没给**时按不支持处理（`list_item` 对缺失键按 false 折叠），
    // 但没有证据就不声称支持 —— 与本仓「广告能力要有实测依据」的口径一致。
    object.insert(
        "supportsImages".to_string(),
        Value::Bool(entry.supports_vision.unwrap_or(false)),
    );
    object.insert("supportsVideo".to_string(), Value::Bool(false));
    object.insert(
        "supportsReasoning".to_string(),
        Value::Bool(!entry.reasoning_levels.is_empty()),
    );
    // 网关是 OpenAI Chat Completions，tools 与 telemetry 都是原样透传 ⇒ 支持工具调用
    object.insert("supportsToolCall".to_string(), Value::Bool(true));
    Value::Object(object)
}

/// 当前目录的聚合层清单
pub fn list() -> Vec<Value> {
    with_state(|guard| {
        guard
            .as_ref()
            .map(|state| state.entries.iter().map(to_manifest_value).collect())
            .unwrap_or_default()
    })
}

/// 取一条缓存的模型（按客户端给的名字找），返回它的**入口形态**。
///
/// 客户端手里的清单来自 `/v1/models`，那里的 `id` 与 `name` 都可能被用来发请求
/// （有的客户端认 id、有的认 name），所以两个都要能命中。
fn find_entry(needle: &str) -> Option<ModelEntry> {
    let target = needle.trim();
    if target.is_empty() {
        return None;
    }
    with_state(|guard| {
        let state = guard.as_ref()?;
        state
            .entries
            .iter()
            .find(|entry| entry.gateway_id == target || entry.display_name == target)
            .cloned()
    })
}

/// 客户端请求里的模型名 → 发给网关的模型名。
///
/// 认不出时返回 `None`，由调用方按「原样发」处理（`chat.rs` 就是这么用的）：
/// 猜一个更接近的名字比原样发更糟 —— 那会把一个「名字没映射」的问题
/// 变成一次静默的错误重试。
pub fn upstream_id_for(requested: &str) -> Option<String> {
    find_entry(requested).map(|entry| entry.gateway_id)
}

/// 目录是否为空（界面「刷新目录」按钮的提示用）
pub fn is_empty() -> bool {
    with_state(|guard| {
        guard
            .as_ref()
            .map(|state| state.entries.is_empty())
            .unwrap_or(true)
    })
}

/// 目录是否来自**远程拉取**（聚合层用它区分「远程来源」与「内置兜底」）。
///
/// 本家**没有内置兜底清单**（有意为之：名单里写死的模型点一下就可能是
/// 「参数错误」，见模块头），所以「有目录」等价于「远程拉到过」。
pub fn remote_refreshed() -> bool {
    with_state(|guard| {
        guard
            .as_ref()
            .map(|state| state.fetched_at > 0 && !state.entries.is_empty())
            .unwrap_or(false)
    })
}

/// 目录最后一次落地时刻（毫秒；0 = 没有目录）
pub fn last_refreshed_at() -> i64 {
    with_state(|guard| guard.as_ref().map(|state| state.fetched_at).unwrap_or(0))
}

/// 当前目录的年龄（毫秒；0 = 没有目录）
pub fn age_ms() -> i64 {
    with_state(|guard| {
        guard
            .as_ref()
            .map(|state| logging::now_ms() - state.fetched_at)
            .unwrap_or(0)
    })
}

/// 套餐档位推断：从 `status-v2` 的套餐名里取 Max / Pro / Lite。
///
/// 上游套餐名形如 `CodingPlan Lite-体验版` / `CodingPlan Pro` / `CodingPlan Max`，
/// 匹配**从最具体到最不具体**（与官方 `types.rs:53-70` 同一顺序）。
pub fn plan_tier_from_name(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    if lower.contains("max") {
        return Some(PLAN_MAX);
    }
    if lower.contains("pro") {
        return Some(PLAN_PRO);
    }
    if lower.contains("lite") {
        return Some(PLAN_LITE);
    }
    None
}

/// 档位常量（`models-v2?plan_type=` 的取值）
pub const PLAN_MAX: &str = "Max";
/// 见 [`PLAN_MAX`]
pub const PLAN_PRO: &str = "Pro";
/// 见 [`PLAN_MAX`]
pub const PLAN_LITE: &str = "Lite";

/// 从 `status-v2` 响应体里取档位
pub fn plan_tier_from_status(payload: &Value) -> Option<&'static str> {
    let name = payload
        .get("codingplan_free")
        .and_then(|plan| plan.get("plan_name"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("plan_type").and_then(Value::as_str))
        .unwrap_or("");
    plan_tier_from_name(name)
}

/// 带鉴权取一个 JSON（目录与状态两条路共用）。
async fn fetch_json(
    url: &str,
    credentials: &AtomcodeCredentials,
    timeout_ms: u64,
) -> Result<Value, GatewayError> {
    let headers: Vec<(String, String)> = vec![
        (
            "authorization".to_string(),
            format!("Bearer {}", credentials.access_token),
        ),
        ("accept".to_string(), "application/json".to_string()),
        (
            "user-agent".to_string(),
            super::endpoints::ADAPTER_USER_AGENT.to_string(),
        ),
    ];
    let response = send_raw("GET", url, None, &headers, None, Some(timeout_ms))
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GatewayError::with_status(504, "AtomCode 目录接口超时")
            } else {
                GatewayError::with_status(502, format!("AtomCode 目录接口请求失败: {error}"))
            }
        })?;
    let payload = response.payload.ok_or_else(|| {
        GatewayError::with_status(502, "AtomCode 目录接口响应不是 JSON")
    })?;
    if !response.ok {
        return Err(GatewayError::with_status(
            response.status as i32,
            format!(
                "AtomCode 目录接口返回 HTTP {}：{}",
                response.status,
                payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("（无文案）")
            ),
        ));
    }
    Ok(payload)
}

/// 取套餐状态（`status-v2`）：档位推断与额度读数（`usage.rs`）共用这一条路。
pub async fn fetch_status(
    credentials: &AtomcodeCredentials,
    timeout_ms: u64,
) -> Result<Value, GatewayError> {
    fetch_json(&status_url(), credentials, timeout_ms).await
}

/// 拉一次 `models-v2`（给定档位）
async fn fetch_catalog(
    credentials: &AtomcodeCredentials,
    plan_type: &str,
) -> Result<Vec<ModelEntry>, GatewayError> {
    let payload = fetch_json(&models_url(plan_type), credentials, CATALOG_TIMEOUT_MS).await?;
    let items = payload
        .get("models")
        .and_then(Value::as_array)
        .or_else(|| payload.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(items.iter().filter_map(entry_from_upstream).collect())
}

/// 刷新目录。
///
/// 档位推断不出来时**依次试 Lite → Pro → Max**（各一次，取第一个非空的）。
/// 试三次而不是直接失败，是因为免费档最常见的形态是 Lite，而档位名上游改过
/// 一次口径（`Lite-体验版`）——「推断失败」不该等于「目录空」。
pub async fn refresh(
    credentials: &AtomcodeCredentials,
    force: bool,
) -> ModelRefreshOutcome {
    if !force && !is_empty() && age_ms() < CATALOG_TTL_MS {
        return ModelRefreshOutcome::unchanged();
    }

    let plan_type = match fetch_json(&status_url(), credentials, REQUEST_TIMEOUT_MS).await {
        Ok(status) => plan_tier_from_status(&status),
        Err(error) => {
            // 状态接口失败不致命：档位退化成「逐个试」，目录仍可能拿到
            logging::verbose(
                "[AtomCode]",
                &format!("套餐状态读取失败，改为逐个档位试目录：{}", error.message),
            );
            None
        }
    };
    let candidates: Vec<&str> = match plan_type {
        Some(tier) => vec![tier],
        None => vec![PLAN_LITE, PLAN_PRO, PLAN_MAX],
    };

    let mut last_error: Option<String> = None;
    for tier in candidates {
        match fetch_catalog(credentials, tier).await {
            Ok(entries) if !entries.is_empty() => {
                let fetched_at = logging::now_ms();
                let cached: Vec<Value> = entries.iter().map(to_cached_value).collect();
                catalog_cache::save(SCOPE, &cached, fetched_at);
                with_state(|guard| {
                    *guard = Some(CatalogState {
                        entries: entries.clone(),
                        fetched_at,
                    });
                });
                logging::log(
                    "[AtomCode]",
                    &format!("目录刷新完成：{} 个模型（档位 {tier}）", entries.len()),
                );
                return ModelRefreshOutcome::refreshed(entries.len());
            }
            Ok(_) => {
                last_error = Some(format!("档位 {tier} 下目录为空"));
            }
            Err(error) => {
                last_error = Some(error.message.clone());
            }
        }
    }
    ModelRefreshOutcome::failed(
        last_error.unwrap_or_else(|| "AtomCode 目录刷新失败：上游没有返回可用模型".to_string()),
    )
}

/// 领取套餐：`POST /coding-plan/claim-v2`，按 Max → Pro → Lite 级联、命中即停。
///
/// ⚠️ 这个端点**没有幂等标记**：重复领取同样返回成功（本地 Go 参考实测），
/// 所以调用方可以多次调用，但**不能**把「成功」当成「刚发放」。
pub async fn claim_plan(
    credentials: &AtomcodeCredentials,
    plan_type: &str,
) -> Result<Value, GatewayError> {
    let url = format!("{}{}", codingplan_base_url(), CODINGPLAN_CLAIM_PATH);
    let headers: Vec<(String, String)> = vec![
        (
            "authorization".to_string(),
            format!("Bearer {}", credentials.access_token),
        ),
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        (
            "user-agent".to_string(),
            super::endpoints::ADAPTER_USER_AGENT.to_string(),
        ),
    ];
    let body = json!({ "plan_type": plan_type });
    let response = send_raw("POST", &url, Some(&body), &headers, None, Some(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GatewayError::with_status(504, "AtomCode 套餐领取超时")
            } else {
                GatewayError::with_status(502, format!("AtomCode 套餐领取失败: {error}"))
            }
        })?;
    let payload = response.payload.unwrap_or(Value::Null);
    if !response.ok {
        return Err(GatewayError::with_status(
            response.status as i32,
            format!("AtomCode 套餐领取返回 HTTP {}", response.status),
        ));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 档位推断：从最具体到最不具体（`max` 先于 `pro`、`pro` 先于 `lite`）。
    #[test]
    fn plan_tier_inference_prefers_the_most_specific_name() {
        assert_eq!(plan_tier_from_name("CodingPlan Lite-体验版"), Some(PLAN_LITE));
        assert_eq!(plan_tier_from_name("CodingPlan Pro"), Some(PLAN_PRO));
        assert_eq!(plan_tier_from_name("CodingPlan Max"), Some(PLAN_MAX));
        // 名字里同时含 pro 与 max 时取 max（官方同一顺序）
        assert_eq!(plan_tier_from_name("CodingPlan Pro Max"), Some(PLAN_MAX));
        assert_eq!(plan_tier_from_name("免费套餐"), None);
    }

    /// 从 status-v2 响应里取档位：`codingplan_free.plan_name` 优先，`plan_type` 兜底。
    #[test]
    fn plan_tier_reads_from_status_envelope() {
        assert_eq!(
            plan_tier_from_status(&json!({"codingplan_free": {"plan_name": "CodingPlan Lite-体验版"}})),
            Some(PLAN_LITE)
        );
        assert_eq!(
            plan_tier_from_status(&json!({"plan_type": "Pro"})),
            Some(PLAN_PRO)
        );
        assert_eq!(plan_tier_from_status(&json!({})), None);
    }

    /// **关键**：`id` 是数字、`display_model_name` 才是模型名 —— 别取错字段。
    #[test]
    fn entry_uses_display_model_name_not_the_numeric_id() {
        let upstream = json!({
            "id": 1980884839691821059_i64,
            "display_model_name": "glm5.3-flash",
            "plan_available": true,
            "context_window": 512000,
            "supports_vision": true,
            "reasoning_effort_levels": ["low", "high"]
        });
        let entry = entry_from_upstream(&upstream).expect("应映射成功");
        assert_eq!(entry.gateway_id, "glm5.3-flash");
        assert_eq!(entry.context_window, 512_000);
        assert_eq!(entry.supports_vision, Some(true));
        assert_eq!(entry.reasoning_levels, vec!["low", "high"]);
        // 展示名走规范化表
        assert_eq!(entry.display_name, "GLM-5.3-Flash");
    }

    /// 不广告 `plan_available == false` 的条目（服务端自己的权益判定）。
    #[test]
    fn entries_without_entitlement_are_skipped() {
        let upstream = json!({"display_model_name": "glm5.3-flash", "plan_available": false});
        assert!(entry_from_upstream(&upstream).is_none());
        // plan_available 缺失也算不可用（不猜「默认给」）
        let missing = json!({"display_model_name": "glm5.3-flash"});
        assert!(entry_from_upstream(&missing).is_none());
    }

    /// 缺 `display_model_name` 的条目丢掉：没有它就发不出请求。
    #[test]
    fn entries_without_a_gateway_name_are_dropped() {
        let upstream = json!({"id": 42, "plan_available": true});
        assert!(entry_from_upstream(&upstream).is_none());
        let blank = json!({"display_model_name": "   ", "plan_available": true});
        assert!(entry_from_upstream(&blank).is_none());
    }

    /// 上游没给能力位时不猜：`supports_vision` 保持 None，
    /// 清单里按不支持折叠（广告能力要有实测依据）。
    #[test]
    fn unknown_capabilities_are_not_advertised() {
        let upstream = json!({"display_model_name": "deepseek-flash", "plan_available": true});
        let entry = entry_from_upstream(&upstream).expect("应映射成功");
        assert_eq!(entry.supports_vision, None);
        let manifest = to_manifest_value(&entry);
        assert_eq!(manifest.get("supportsImages"), Some(&Value::Bool(false)));
        assert_eq!(manifest.get("supportsReasoning"), Some(&Value::Bool(false)));
        // 上下文窗口未知时不写这个键（由 `list_item` 折叠成缺失）
        assert!(manifest.get("maxInputTokens").is_none());
    }

    /// 清单键名必须与 `list_item` 读取的键一一对上。
    #[test]
    fn manifest_keys_match_the_aggregate_contract() {
        let upstream = json!({
            "display_model_name": "qwen3.8-27b", "plan_available": true,
            "context_window": 262144, "supports_vision": true,
            "reasoning_effort_levels": ["low"]
        });
        let entry = entry_from_upstream(&upstream).expect("应映射成功");
        let manifest = to_manifest_value(&entry);
        for key in ["id", "name", "maxInputTokens", "supportsImages", "supportsVideo", "supportsReasoning", "supportsToolCall"] {
            assert!(manifest.get(key).is_some(), "缺键 {key}");
        }
        assert_eq!(manifest.get("id"), Some(&Value::String("qwen3.8-27b".into())));
        assert_eq!(manifest.get("supportsToolCall"), Some(&Value::Bool(true)));
    }

    /// 落盘缓存的往返：写下的形态必须能原样读回。
    #[test]
    fn cached_entries_round_trip() {
        let entry = ModelEntry {
            gateway_id: "glm5.3-flash".into(),
            display_name: "GLM-5.3-Flash".into(),
            context_window: 512_000,
            supports_vision: Some(false),
            reasoning_levels: vec!["high".into()],
        };
        let cached = to_cached_value(&entry);
        let restored = entry_from_cached(&cached).expect("应可还原");
        assert_eq!(restored.gateway_id, "glm5.3-flash");
        assert_eq!(restored.display_name, "GLM-5.3-Flash");
        assert_eq!(restored.context_window, 512_000);
        assert_eq!(restored.supports_vision, Some(false));
        assert_eq!(restored.reasoning_levels, vec!["high"]);
    }

    /// 「认不出就 None」：调用方会原样发，而不是猜一个更接近的名字。
    #[test]
    fn unknown_names_resolve_to_none() {
        assert!(upstream_id_for("完全不存在的模型").is_none());
        assert!(upstream_id_for("   ").is_none());
    }
}
