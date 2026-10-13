//! Antigravity 模型目录：`POST {base}:fetchAvailableModels` + 内置兜底清单。
//!
//! ── 上游形态（规格 §5.1）──────────────────────────────────────
//! ```text
//! POST {base}/v1internal:fetchAvailableModels
//! body   {"project": "<pid>"}（没有 pid 时 `{}`；上游忽略这个字段）
//! header Authorization: Bearer <token>、User-Agent: vscode/1.X.X (Antigravity/{ver})
//! resp   {"models": { "<model_id>": { quotaInfo: {remainingFraction, resetTime},
//!                                      displayName, supportsImages, supportsThinking,
//!                                      thinkingBudget, recommended, maxTokens,
//!                                      maxOutputTokens, supportedMimeTypes } },
//!          "deprecatedModelIds": { "<old>": {"newModelId": "<new>"} } }
//! ```
//! 模型是**以 id 为键的对象**（不是数组）—— 解析时要按 Map 遍历，别按数组读。
//!
//! ── 本步只列 Gemini（任务范围）─────────────────────────────────
//! 同一个端点也会下发 `claude-sonnet-4-6` / `claude-opus-4-6-*` /
//! `gpt-oss-120b-medium`（规格 §5.3），但本家本步**只接 Gemini**：
//!   - 动态目录按 **id 前缀 `gemini-`** 过滤（`claude-*` / `gpt-oss-*` 因此
//!     既不进广告清单、也不进缓存 —— 客户端点不到，也就不会走一条未实现的通道）；
//!   - 内置兜底清单同口径，只列规格 §5.2 的 Gemini id（逐字）。
//! 前缀白名单而不是「黑名单排除 claude/gpt-oss」是刻意的：将来上游再下发
//! 别的家族（veo 之类）时，默认仍是「不接」而不是「误广告一个没实现的通道」。
//!
//! ── 兜底清单的 id 与「发给上游的名字」是两回事（本步已落地映射表）──
//! 规格 §5.2 的 id 是**本家对外的名字**：`gemini-3.6-flash` 发出去时上游要的是
//! `gemini-3.6-flash-tiered`，`gemini-3.1-pro-high` 对应上游真名
//! `gemini-pro-agent`（`gemini-3-pro-high` 同理）。映射表见
//! [`upstream_model_id`]（出处：Manager `CLAUDE_TO_GEMINI` 的 Gemini 部分 +
//! `is_bare_gemini_v36_or_above_flash` 的 tiered 规则；9router registry 的 id
//! 集合交叉验证）。**对不上的 id 原样透传** —— 远程
//! `fetchAvailableModels` 下发的 id 本身就是上游真名（规格 §5.2 末句 /
//! §8.11），不该被这张表改写。
//! 图片模型的组合后缀（`-2k/-4k` × 六个比例）在规格里只给了后缀集合、
//! 没给拼接形态，**不猜**：兜底清单只放 `gemini-3-pro-image` 与
//! `gemini-3.1-flash-image` 两条基础 id（TODO 见报告）。
//!
//! ── 三个环境轮流打（规格 §3.1 / §7.15）────────────────────────
//! `sandbox → daily → prod`：429 / 5xx / 传输失败就换下一个；4xx（403/404 之类）
//! 不换 —— 那是「这次请求本身不对」，换域名只会白撞。403 且带
//! `x-goog-user-project` 时按规格坑 #3 去掉该头再试一次。
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 与各家同款：内存缓存 + `providers::catalog_cache` 落盘（重启后读回）。
//! 本家**没有地区之分**（规格 §6），因此单格（`SCOPE_ANTIGRAVITY`）。
//! 自动路径 10 分钟 TTL；用户手动「刷新模型清单」时 `force = true` 绕过。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::server::core::egress;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::providers::catalog_cache;

use super::endpoints;

/// 本家在 `catalog_cache` 里的 scope 名（单一环境/地区，一格即可）
pub const SCOPE: &str = catalog_cache::SCOPE_ANTIGRAVITY;

/// 自动路径的缓存 TTL（与别家的 5~10 分钟同档；目录不是热点数据）
const TTL_MS: i64 = 10 * 60 * 1000;

/// 单次目录请求超时
const REQUEST_TIMEOUT_MS: u64 = 20_000;

/// 本家对外 id → 上游模型 id 的**精确**别名表（规格 §5.2 括号里的真名；
/// Manager `CLAUDE_TO_GEMINI` 的 Gemini 部分逐条）：
///
/// ```text
///   gemini-3.6/3.7/3.8-flash  → <同 id>-tiered    （无后缀 Flash 走自适应档）
///   gemini-3.1-pro-high       → gemini-pro-agent  （Manager 表逐字）
///   gemini-3-pro-high         → gemini-pro-agent  （同上）
///   gemini-2.5-flash-lite     → gemini-2.5-flash  （Manager 表逐字；9router
///                               registry 不含这条 id，属两个参考的差异）
/// ```
///
/// 表里没有的一律原样透传（见模块头：远程目录下发的 id 就是上游真名）。
const UPSTREAM_ALIASES: [(&str, &str); 6] = [
    ("gemini-3.6-flash", "gemini-3.6-flash-tiered"),
    ("gemini-3.7-flash", "gemini-3.7-flash-tiered"),
    ("gemini-3.8-flash", "gemini-3.8-flash-tiered"),
    ("gemini-3.1-pro-high", "gemini-pro-agent"),
    ("gemini-3-pro-high", "gemini-pro-agent"),
    ("gemini-2.5-flash-lite", "gemini-2.5-flash"),
];

/// 对外 id（或上游真名）→ 真正发给 v1internal 的模型 id。
///
/// 顺序：精确别名表 → 「无后缀 `gemini-<x.y>-flash`（x.y ≥ 3.6）加 `-tiered`」
/// 的动态规则（Manager `is_bare_gemini_v36_or_above_flash` 的语义）→
/// 原样返回。大小写不敏感匹配（客户端常发小写），但**返回值保留调用方给的
/// 原始大小写**（与 Manager `format!("{}-tiered", original_model)` 同口径）。
pub fn upstream_model_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lowered = trimmed.to_ascii_lowercase();
    for (alias, target) in UPSTREAM_ALIASES {
        if lowered == alias {
            return target.to_string();
        }
    }
    if let Some(tiered) = tiered_flash_id(trimmed, &lowered) {
        return tiered;
    }
    trimmed.to_string()
}

/// 「无后缀、版本 ≥ 3.6 的 `gemini-<x.y>-flash`」→ 追加 `-tiered`；
/// 其余（已有档位/变体后缀、版本不够、不是 flash）返回 None。
///
/// 逐条对齐 Manager `is_bare_gemini_v36_or_above_flash`：先排除一切已知后缀
/// （`-high` / `-medium` / `-low` / `-extra-low` / `-tiered` / `-preview` /
/// `-agent` / `-thinking` / `-image`），再解析 `gemini-` 之后的版本段
/// （取到第一个非数字非点号的字符为止）要求 ≥ 3.6。
fn tiered_flash_id(original: &str, lowered: &str) -> Option<String> {
    for suffix in [
        "-high", "-medium", "-low", "-extra-low", "-tiered", "-preview", "-agent", "-thinking",
        "-image",
    ] {
        if lowered.contains(suffix) {
            return None;
        }
    }
    if !lowered.contains("flash") {
        return None;
    }
    let rest = lowered.strip_prefix("gemini-")?;
    let version: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    let version = version.trim_end_matches('.').parse::<f64>().ok()?;
    if version < 3.6 {
        return None;
    }
    Some(format!("{original}-tiered"))
}

/// 按 id 取**目录条目**（远程清单优先，其次内置兜底）。
///
/// 取值链：给定 id 直查 → 它映射出的上游真名再查一次。两个方向都需要：
/// 调用方可能拿着**对外 id**（内置兜底清单的键），也可能拿着**上游真名**
/// （远程目录的键，规格 §8.11 说明线上下发的可能是真名）。
pub fn entry(id: &str) -> Option<Value> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return None;
    }
    let models = list();
    for candidate in [trimmed.to_string(), upstream_model_id(trimmed)] {
        if candidate.is_empty() {
            continue;
        }
        if let Some(item) = models
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(candidate.as_str()))
        {
            return Some(item.clone());
        }
    }
    None
}

/// 目录条目里的一个数值键（远程清单的 `thinkingBudget` / `maxOutputTokens`；
/// 内置兜底没有这些键 → None）
pub fn entry_number(id: &str, key: &str) -> Option<i64> {
    entry(id).and_then(|item| item.get(key).and_then(Value::as_i64))
}

/// 目录条目里的一个布尔键（`supportsThinking` / `supportsImages`）
pub fn entry_bool(id: &str, key: &str) -> Option<bool> {
    entry(id).and_then(|item| item.get(key).and_then(Value::as_bool))
}

/// 内置兜底清单（规格 §5.2 的 Gemini id **逐字**，只保留 `gemini-` 家族）。
///
/// 键是**对外的模型 id**（下一步转发时要映射成上游真名，见模块头）；
/// 值是展示名（上游 `displayName` 拉不到时的界面文案，不影响转发）。
const FALLBACK_MODELS: [(&str, &str); 25] = [
    ("gemini-3-flash", "Gemini 3 Flash"),
    ("gemini-3.5-flash", "Gemini 3.5 Flash"),
    ("gemini-3.5-flash-high", "Gemini 3.5 Flash High"),
    ("gemini-3.5-flash-low", "Gemini 3.5 Flash Low"),
    ("gemini-3.5-flash-extra-low", "Gemini 3.5 Flash Extra Low"),
    ("gemini-3.6-flash", "Gemini 3.6 Flash"),
    ("gemini-3.7-flash", "Gemini 3.7 Flash"),
    ("gemini-3.8-flash", "Gemini 3.8 Flash"),
    ("gemini-3.7-flash-low", "Gemini 3.7 Flash Low"),
    ("gemini-3.7-flash-medium", "Gemini 3.7 Flash Medium"),
    ("gemini-3.7-flash-high", "Gemini 3.7 Flash High"),
    ("gemini-3.1-pro-low", "Gemini 3.1 Pro Low"),
    ("gemini-3.1-pro-high", "Gemini 3.1 Pro High"),
    ("gemini-pro-agent", "Gemini Pro Agent"),
    ("gemini-3.1-pro-preview", "Gemini 3.1 Pro Preview"),
    ("gemini-3-pro-low", "Gemini 3 Pro Low"),
    ("gemini-3-pro-high", "Gemini 3 Pro High"),
    ("gemini-3-pro-preview", "Gemini 3 Pro Preview"),
    ("gemini-2.5-flash", "Gemini 2.5 Flash"),
    ("gemini-2.5-flash-lite", "Gemini 2.5 Flash Lite"),
    ("gemini-2.5-flash-thinking", "Gemini 2.5 Flash Thinking"),
    ("gemini-2.0-flash", "Gemini 2.0 Flash"),
    ("gemini-2.0-flash-exp", "Gemini 2.0 Flash Exp"),
    ("gemini-3-pro-image", "Gemini 3 Pro Image"),
    ("gemini-3.1-flash-image", "Gemini 3.1 Flash Image"),
];

/// 进程内缓存（首次访问时从落盘缓存读回）
struct CatalogState {
    models: Vec<Value>,
    fetched_at: i64,
    /// 这份清单是不是**远程拉到**的（落盘缓存装回来的也算 —— 它当初就是远程的）
    remote: bool,
}

static STATE: OnceLock<Mutex<Option<CatalogState>>> = OnceLock::new();

/// 取状态句柄（毒锁按「继续用」处理：这份缓存是可重建的展示数据，
/// 不值得让进程挂掉）
fn state() -> &'static Mutex<Option<CatalogState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

/// 在锁内执行一段取值，并在首次访问时把落盘缓存装回内存
fn with_state<R>(f: impl FnOnce(&mut Option<CatalogState>) -> R) -> R {
    let mutex = state();
    let mut guard = mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        if let Some(cached) = catalog_cache::load(SCOPE) {
            *guard = Some(CatalogState {
                models: cached.models,
                fetched_at: cached.fetched_at,
                remote: true,
            });
        }
    }
    f(&mut guard)
}

/// 这个 id 是不是本家对外提供的模型：**只认 `gemini-` 家族**
/// （`claude-*` / `gpt-oss-*` 因此被排除，见模块头）。
pub fn is_gemini_model(id: &str) -> bool {
    id.trim().to_ascii_lowercase().starts_with("gemini-")
}

/// 该 id 是否支持思考（兜底清单的判据；远程清单用上游自己的 `supportsThinking`）。
///
/// Gemini 3 系全体 + 名字里带 `thinking` 的（2.5 那一支）给 true，
/// 其余（2.0、2.5 普通档）保守 false。
fn fallback_thinking_supported(id: &str) -> bool {
    let lowered = id.to_ascii_lowercase();
    lowered.starts_with("gemini-3") || lowered.contains("pro-agent") || lowered.contains("thinking")
}

/// 兜底清单里的「是否推荐为默认」：规格 §5.2 把 `gemini-3-flash` 标为推荐入口
fn fallback_is_default(id: &str) -> bool {
    id == "gemini-3-flash"
}

/// 把一个上游目录条目映射成聚合层认的形态。
///
/// 键名对照 `core/models/shape.rs::list_item`：`id` / `name` /
/// `maxInputTokens` / `maxOutputTokens` / `supportsImages` / `supportsVideo` /
/// `supportsReasoning` / `supportsToolCall` / `isDefault`。
/// 另有三个**非聚合层**的键（聚合层原样忽略未知键）：`quota`（规格 §5 的
/// `remainingFraction` / `resetTime`）、`supportsThinking`、`thinkingBudget` ——
/// 留给下一步做思考档映射与界面配额显示。
fn map_entry(id: &str, info: &Value) -> Option<Value> {
    if !is_gemini_model(id) {
        return None;
    }
    let display_name = info
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(id);
    let mut item = Map::new();
    item.insert("id".to_string(), Value::String(id.to_string()));
    item.insert("name".to_string(), Value::String(display_name.to_string()));
    // 工具调用是本家 wire 协议的一等公民（`request.tools`，规格 §3.3）→ true
    item.insert("supportsToolCall".to_string(), Value::Bool(true));
    // 输入图片：目录有 `supportsImages` 就照它；没有（内置兜底 / 字段缺失）按
    // Gemini 家族的多模态能力给 true（规格 §4.3 的 `inlineData` 就是它）。
    // 视频位**没有依据**，一律 false（报错方向只能选「少报」）。
    item.insert(
        "supportsImages".to_string(),
        Value::Bool(
            info.get("supportsImages")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        ),
    );
    item.insert("supportsVideo".to_string(), Value::Bool(false));
    let thinking = info
        .get("supportsThinking")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| fallback_thinking_supported(id));
    item.insert("supportsReasoning".to_string(), Value::Bool(thinking));
    item.insert("supportsThinking".to_string(), Value::Bool(thinking));
    item.insert(
        "isDefault".to_string(),
        Value::Bool(
            info.get("recommended")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| fallback_is_default(id)),
        ),
    );
    for (source, target) in [("maxTokens", "maxInputTokens"), ("maxOutputTokens", "maxOutputTokens")] {
        if let Some(value) = info.get(source).and_then(Value::as_i64) {
            if value > 0 {
                item.insert(target.to_string(), Value::from(value));
            }
        }
    }
    if let Some(budget) = info.get("thinkingBudget").and_then(Value::as_i64) {
        if budget > 0 {
            item.insert("thinkingBudget".to_string(), Value::from(budget));
        }
    }
    // 配额（规格 §5 / §5.4）：原样搬运 `remainingFraction` 与 `resetTime`
    if let Some(quota) = info.get("quotaInfo") {
        if quota.is_object() {
            item.insert("quota".to_string(), quota.clone());
        }
    }
    Some(Value::Object(item))
}

/// 解析 `fetchAvailableModels` 的响应：`{models: {"<id>": {…}}}`。
///
/// 也接受 `models` 是**数组**的形态（防御性：字段形态若变，至少不整份丢），
/// 数组里的条目要从自己的 `id` 字段取名字。
pub fn parse_models(payload: &Value) -> Vec<Value> {
    let Some(models) = payload.get("models") else {
        return Vec::new();
    };
    if let Some(rows) = models.as_array() {
        return rows
            .iter()
            .filter_map(|row| {
                let id = row
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())?;
                map_entry(id, row)
            })
            .collect();
    }
    let Some(object) = models.as_object() else {
        return Vec::new();
    };
    object
        .iter()
        .filter_map(|(id, info)| map_entry(id, info))
        .collect()
}

/// 内置兜底清单的聚合层形态（永远非空）。
///
/// 条目本体走 [`map_entry`]（能力位的默认值与兜底判据只有一份），展示名用
/// [`FALLBACK_MODELS`] 里那份人读的名字（远程拉不到时这是界面上唯一的文案）。
fn fallback_models() -> Vec<Value> {
    FALLBACK_MODELS
        .iter()
        .filter_map(|(id, name)| {
            let mut item = map_entry(id, &Value::Null)?;
            if let Some(object) = item.as_object_mut() {
                object.insert("name".to_string(), Value::String((*name).to_string()));
            }
            Some(item)
        })
        .collect()
}

/// 当前模型清单（内存 → 落盘缓存 → 内置兜底；永远非空）
pub fn list() -> Vec<Value> {
    with_state(|slot| {
        slot.as_ref()
            .map(|state| state.models.clone())
            .filter(|models| !models.is_empty())
            .unwrap_or_else(fallback_models)
    })
}

/// 这份清单是否来自**远程拉取**（`catalog::refresh_meta` 的「来源」列判据）
pub fn remote_refreshed() -> bool {
    with_state(|slot| slot.as_ref().map(|state| state.remote).unwrap_or(false))
}

/// 最近一次成功刷新的时刻（毫秒；没刷过为 0）—— 界面「更新日期」列读它
pub fn last_refreshed_at() -> i64 {
    with_state(|slot| slot.as_ref().map(|state| state.fetched_at).unwrap_or(0))
}

/// 一次目录请求的中间结果（供环境轮询判断要不要换下一条基址）
enum Attempt {
    /// 拿到了可用清单
    Models(Vec<Value>),
    /// 这次不成（原因），可以换下一条基址
    Retry(String),
    /// 这次不成，且换域名也没用（4xx 之外的确定失败）
    Fatal(String),
}

/// 打一次目录请求（单条基址）。
async fn fetch_once(
    base: &str,
    access_token: &str,
    project_id: &str,
    proxy: Option<&ResolvedProxy>,
) -> Attempt {
    let url = endpoints::fetch_available_models_url(base);
    let body = if project_id.trim().is_empty() {
        json!({})
    } else {
        json!({ "project": project_id.trim() })
    };
    let response = match send_json(&url, access_token, project_id, &body, proxy).await {
        Ok(response) => response,
        Err(reason) => return Attempt::Retry(reason),
    };
    if response.status == 403 && !project_id.trim().is_empty() {
        // 规格坑 #3：403 + 带了 `x-goog-user-project` → 去掉该头重试一次
        // （content 类请求本就不带它；这是非 content 方法上的降级路径）
        match send_json(&url, access_token, "", &body, proxy).await {
            Ok(retried) if (200..300).contains(&retried.status) => {
                return parse_payload(&retried.payload)
            }
            Ok(retried) => {
                return Attempt::Fatal(format!(
                    "目录接口返回 HTTP {}（去掉 x-goog-user-project 重试后仍失败）",
                    retried.status
                ))
            }
            Err(reason) => return Attempt::Retry(reason),
        }
    }
    if (200..300).contains(&response.status) {
        return parse_payload(&response.payload);
    }
    let detail = upstream_message(&response.payload);
    let summary = if detail.is_empty() {
        format!("目录接口返回 HTTP {}", response.status)
    } else {
        format!("目录接口返回 HTTP {}：{detail}", response.status)
    };
    // 429 / 5xx 换下一条基址；其余 4xx 是请求本身的问题，换域名白撞
    if response.status == 429 || response.status >= 500 {
        Attempt::Retry(summary)
    } else {
        Attempt::Fatal(summary)
    }
}

/// 一次 HTTP 结果（目录请求只需要状态 + 解析后的 body）
struct HttpOutcome {
    status: u16,
    payload: Value,
}

/// 发一次 JSON POST（头集合与超时统一在这里）
async fn send_json(
    url: &str,
    access_token: &str,
    project_id: &str,
    body: &Value,
    proxy: Option<&ResolvedProxy>,
) -> Result<HttpOutcome, String> {
    // 本家统一的出口口径：账号显式配了代理就用它，否则跟随系统代理
    // （理由见 `super::client_for` 的文档）
    let client = super::client_for(proxy);
    let headers = endpoints::admin_headers(access_token, project_id);
    let mut builder = client
        .post(url)
        .header("Accept", "application/json")
        .timeout(Duration::from_millis(REQUEST_TIMEOUT_MS));
    for (key, value) in &headers {
        builder = builder.header(key, value);
    }
    let response = builder
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| egress::describe_error_detail(&error))?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(HttpOutcome { status, payload })
}

/// 成功响应 → 清单。
///
/// 空清单（或一个 Gemini 都没有）算**「这次没拿到」而不是「这次拿到了空目录」**：
/// 环境基址之间可能有差异（某个环境对该账号限流 / 尚未同步），换下一条再试一次
/// 比直接把「空目录」当成结论更稳 —— 三条都空时最终仍会如实报失败并保留现有清单。
fn parse_payload(payload: &Value) -> Attempt {
    let models = parse_models(payload);
    if models.is_empty() {
        return Attempt::Retry("目录接口返回的清单为空或不含 Gemini 模型".to_string());
    }
    Attempt::Models(models)
}

/// 上游错误文案（`error.message` / `message` 兜底）
fn upstream_message(payload: &Value) -> String {
    payload
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 拉一次模型目录并落地（内存 + 持久化缓存）。
///
/// `force = false`（自动路径）时 10 分钟 TTL 内早退为 `unchanged()`；
/// `force = true`（用户手动刷新）真打上游。
///
/// 失败时**保留现有清单**（远程清单优先，其次内置兜底）并返回 `failed(原因)`：
/// 手动路径据此如实汇报，自动路径只打日志。
pub async fn refresh(
    access_token: &str,
    project_id: &str,
    proxy: Option<&ResolvedProxy>,
    force: bool,
) -> ModelRefreshOutcome {
    let now = crate::server::logging::now_ms();
    if !force {
        let fresh = with_state(|slot| {
            slot.as_ref()
                .map(|state| state.remote && now - state.fetched_at < TTL_MS)
                .unwrap_or(false)
        });
        if fresh {
            return ModelRefreshOutcome::unchanged();
        }
    }
    if access_token.trim().is_empty() {
        return ModelRefreshOutcome::failed(
            "Antigravity 账号没有可用的 access token（无法拉取模型目录）",
        );
    }
    let mut last_error = String::new();
    for base in endpoints::V1_BASE_URLS {
        match fetch_once(base, access_token, project_id, proxy).await {
            Attempt::Models(models) => {
                let count = models.len();
                with_state(|slot| {
                    *slot = Some(CatalogState {
                        models: models.clone(),
                        fetched_at: now,
                        remote: true,
                    });
                });
                catalog_cache::save(SCOPE, &models, now);
                return ModelRefreshOutcome::refreshed(count);
            }
            Attempt::Retry(reason) => {
                last_error = reason;
                crate::server::logging::verbose(
                    "[Models]",
                    &format!("Antigravity 目录换下一个环境重试（{base}）：{last_error}"),
                );
            }
            Attempt::Fatal(reason) => {
                return ModelRefreshOutcome::failed(format!("{reason}（保留现有清单）"));
            }
        }
    }
    let detail = if last_error.is_empty() {
        "所有环境均不可用".to_string()
    } else {
        last_error
    };
    ModelRefreshOutcome::failed(format!(
        "Antigravity 模型目录刷新失败：{detail}（保留现有清单）"
    ))
}

/// 模型目录刷新结果（用于 `ProviderAdapter::refresh_models` 的返回）
pub use crate::server::core::providers::adapter::ModelRefreshOutcome;
