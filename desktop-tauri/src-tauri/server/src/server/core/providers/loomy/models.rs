//! Loomy 模型目录：`GET {集成网关}/api/v1/models`（OpenAI 格式）+ 持久化缓存。
//!
//! ── 上游形态（客户端 `electron/model-service.js` 的 `fetchModelsFromProvider`）──
//! 标准 OpenAI 形态 `{data:[{id, ...}]}`；鉴权是**双头**：`token: <session>` 与
//! `Authorization: Bearer <session>`（客户端 `authMode: 'token'` 时两个都发）。
//! 拉取失败（500/404）时客户端会按 baseURL 用**内置兜底清单** —— 但那条兜底
//! 只服务用户自配的 `spark-api-open.xf-yun.com`（`getSparkFallbackModels`），
//! Loomy 官方 imodel 网关没有内置清单（`BUILTIN_FALLBACK_MODELS` 为空表）。
//! 因此这里如实报失败、保留旧清单，不编造模型名。
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 与各家同款：内存缓存 + `providers::catalog_cache` 落盘（进程重启后读回）。
//! 自动路径 10 分钟 TTL；用户手动「刷新模型清单」时 `force = true` 绕过。
//!
//! ── 能力位 ──────────────────────────────────────────────────
//! 思考 / 工具 / 图片 / 视频位都读上游条目的 `capabilities`（`reasoning` /
//! `function_calling` / `vision` / `input_modalities`，实测 2026-10 每个条目都
//! 带这四个键，见 `map_entry`）。旧实现只按模型 id 判思考位（spark-x）、硬编码
//! 工具位 —— 那会丢掉 GLM-5.3-Flash 等模型的 vision 声明，目录里全部显示成
//! 不支持图片。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::server::core::providers::catalog_cache;
use crate::server::errors::GatewayError;

use super::client;

/// 本家在 `catalog_cache` 里的 scope 名
pub const SCOPE: &str = catalog_cache::SCOPE_LOOMY;

/// 自动路径的缓存 TTL（与客户端自己的轮询周期同档）
const TTL_MS: i64 = 10 * 60 * 1000;

/// 模型列表路径（相对集成网关）
pub const MODELS_PATH: &str = "/api/v1/models";

/// 进程内缓存（首次访问时从落盘缓存读回）
struct CatalogState {
    models: Vec<Value>,
    fetched_at: i64,
}

static STATE: OnceLock<Mutex<Option<CatalogState>>> = OnceLock::new();

/// 取状态句柄（毒锁按「上一个持有者 panic 过、数据可能半更新」处理：
/// 本项目的取值是「继续用」，因为这份缓存是可重建的展示数据，不值得让进程挂掉）
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
            });
        }
    }
    f(&mut guard)
}

/// 该模型 id 是不是思考模型（客户端只对 spark-x 挂 reasoning 能力位）
fn is_reasoning_model(id: &str) -> bool {
    id.eq_ignore_ascii_case("spark-x") || id.to_ascii_lowercase().contains("spark-x")
}

/// 从上游展示名里拆出**积分倍率**后缀（`(x3.0)` / `(x0.8)` / `（x12）`）。
///
/// ── 为什么倍率要从名字里拆（有实测依据，不是猜）────────────────
/// 上游 `/models` **没有**结构化的倍率字段，它把倍率拼在展示名尾部 ——
/// 实测（2026-10，真实账号拉下来的 11 条目录）：
/// `DeepSeek V4 Flash 0731 (x3.0)`、`GLM 5.3 Flash(x0.8)`、`Spark X2.5 (x0.1)`。
/// 而本网关的「倍率」列读的是 `credits`（`core/models/shape.rs::list_item`），
/// 不拆的话那一列永远显示 `—`（就是用户报的那个现象）。
///
/// ── 括号口径与客户端同源 ────────────────────────────────────
/// 客户端自己也这么拆（`src/lib/model-metadata.js` 的 `splitModelDisplayName`：
/// 末尾括号内容作为展示标签，ASCII 与全角括号都认），这里跟随同一口径。与客户端
/// 唯一的差别：**只把 `x<数字>` 形态的括号算倍率** —— 其余标签（如「限时免费」）
/// 本网关没有承载字段，原样留在名字里，不静默丢弃信息。
///
/// 拆出来的形态用 `x3.0`（前端 `formatCredits` 认 `x\d` 并显示成 `3.0x`），
/// 与 AutoClaw（`低/中/高`）和 CodeArts（`0.7x`）两家「上游给什么就透什么」
/// 的口径一致 —— 这里只是把上游已经给的信息换个字段承载，没有换算、没有编造。
///
/// 同时把后缀从展示名里去掉（只在这一种形态下）：不去掉的话同一个数会出现两次
/// （名称子行 + 倍率列）。去掉后名字为空（整名就是倍率）时保留原名，不返回空串。
fn split_rate_suffix(name: &str) -> (String, Option<String>) {
    let trimmed = name.trim_end();
    // 两种括号都认（口径见上）；只有「末尾括号 + 内容非空」才继续
    let (body, inner) = if let Some(body) = trimmed.strip_suffix(')') {
        match body.rfind('(') {
            Some(open) => (&body[..open], body[open + 1..].trim()),
            None => return (name.to_string(), None),
        }
    } else if let Some(body) = trimmed.strip_suffix('）') {
        match body.rfind('（') {
            Some(open) => (&body[..open], body[open + '（'.len_utf8()..].trim()),
            None => return (name.to_string(), None),
        }
    } else {
        return (name.to_string(), None);
    };
    let Some(number) = inner.strip_prefix('x').or_else(|| inner.strip_prefix('X')) else {
        return (name.to_string(), None);
    };
    let number = number.trim();
    if number.is_empty()
        || !number.chars().any(|ch| ch.is_ascii_digit())
        || !number.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
    {
        return (name.to_string(), None);
    }
    let cleaned = body.trim_end();
    if cleaned.is_empty() {
        return (name.to_string(), Some(format!("x{number}")));
    }
    (cleaned.to_string(), Some(format!("x{number}")))
}

/// 从上游 `capabilities` 对象里取布尔位（缺失 / 非布尔一律 false）
fn caps_bool(caps: Option<&serde_json::Map<String, Value>>, key: &str) -> bool {
    caps.and_then(|caps| caps.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// `capabilities.input_modalities` 数组里有没有指定模态（大小写不敏感）
fn caps_modal(caps: Option<&serde_json::Map<String, Value>>, modal: &str) -> bool {
    caps.and_then(|caps| caps.get("input_modalities"))
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|value| {
                value.as_str().is_some_and(|text| text.eq_ignore_ascii_case(modal))
            })
        })
}

/// 把一个 OpenAI 目录条目映射成聚合层认的形态
/// （键名对照 `core/models/shape.rs::list_item`：`id` / `name` /
/// `maxInputTokens` / `maxOutputTokens` / `supportsImages` / `supportsVideo` /
/// `supportsReasoning` / `supportsToolCall`）。
///
/// 能力位以上游 `capabilities` 为准（实测 2026-10：条目都带
/// `{reasoning, vision, function_calling, input_modalities, ...}`，见模块头的
/// 上游形态说明）。`vision` / `input_modalities` 双读判定图片位，防一端缺失时
/// 静默丢能力 —— 生图模型（Hy image / qwen image）的 `vision:true` 也由此落入
/// `supportsImages`，与目录展示口径一致。缺失时保守回落：工具调用按 OpenAI
/// 兼容网关的既有能力给 true（照 `autoclaw::models` 的处置），思考位保留
/// 按 id 的旧判断兜底。倍率从展示名后缀拆出（见 [`split_rate_suffix`]）。
fn map_entry(row: &Value) -> Option<Value> {
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let raw_name = row
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(id);
    let (name, rate) = split_rate_suffix(raw_name);
    let caps = row.get("capabilities").and_then(Value::as_object);
    let tool_call = caps_bool(caps, "function_calling") || caps.is_none();
    let reasoning = caps_bool(caps, "reasoning") || is_reasoning_model(id);
    let mut item = json!({
        "id": id,
        "name": name,
        "supportsToolCall": tool_call,
        "supportsReasoning": reasoning,
    });
    if caps_bool(caps, "vision") || caps_modal(caps, "image") {
        if let Some(object) = item.as_object_mut() {
            object.insert("supportsImages".to_string(), Value::Bool(true));
        }
    }
    if caps_modal(caps, "video") {
        if let Some(object) = item.as_object_mut() {
            object.insert("supportsVideo".to_string(), Value::Bool(true));
        }
    }
    if let Some(rate) = rate {
        if let Some(object) = item.as_object_mut() {
            object.insert("credits".to_string(), Value::String(rate));
        }
    }
    // 上游给了上下文 / 输出上限就透传。键名取自**客户端自己的规范化器**
    // （`src/lib/model-metadata.js::normalizeFetchedModelMetadata` 读的就是
    // `context_length` / `input_token_limit` / `max_input_tokens` /
    // `max_output_tokens` / `max_completion_tokens` —— 这是上游字段名的一手依据，
    // 不是我们猜的）。
    if let Some(object) = item.as_object_mut() {
        let pick_number = |keys: &[&str]| -> Option<i64> {
            for key in keys {
                if let Some(value) = row.get(*key).and_then(Value::as_i64) {
                    if value > 0 {
                        return Some(value);
                    }
                }
            }
            None
        };
        if let Some(max_input) = pick_number(&[
            "context_length",
            "context_window",
            "input_token_limit",
            "max_input_tokens",
            "maxInputTokens",
        ]) {
            object.insert("maxInputTokens".to_string(), Value::from(max_input));
        }
        if let Some(max_output) = pick_number(&[
            "max_output_tokens",
            "max_completion_tokens",
            "max_tokens",
            "maxOutputTokens",
        ]) {
            object.insert("maxOutputTokens".to_string(), Value::from(max_output));
        }
    }
    Some(item)
}

/// 解析 `/models` 响应（OpenAI `{data:[...]}` 或裸数组）
pub fn parse_models(payload: &Value) -> Vec<Value> {
    let rows = payload
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| payload.as_array())
        .cloned()
        .unwrap_or_default();
    rows.iter().filter_map(map_entry).collect()
}

/// 当前模型清单（内存 → 落盘缓存；都没有时为空，由刷新路径填）
pub fn list() -> Vec<Value> {
    with_state(|slot| slot.as_ref().map(|state| state.models.clone()).unwrap_or_default())
}

/// 这份清单是否来自**远程拉取**（`catalog::refresh_meta` 的「来源」列判据）。
/// Loomy 没有内置兜底清单：有内容就等于远程拉到过。
pub fn remote_refreshed() -> bool {
    with_state(|slot| {
        slot.as_ref()
            .map(|state| !state.models.is_empty())
            .unwrap_or(false)
    })
}

/// 最近一次成功刷新的时刻（毫秒；没刷过为 0）—— 界面「更新日期」列读它
pub fn last_refreshed_at() -> i64 {
    with_state(|slot| slot.as_ref().map(|state| state.fetched_at).unwrap_or(0))
}

/// 拉一次模型目录并落地（内存 + 持久化缓存）。
///
/// `force = false`（自动路径）时 10 分钟 TTL 内早退为 `unchanged()`；
/// `force = true`（用户手动刷新）真打上游。
pub async fn refresh(session: &str, force: bool) -> ModelRefreshOutcome {
    let now = crate::server::logging::now_ms();
    if !force {
        let fresh = with_state(|slot| {
            slot.as_ref()
                .map(|state| !state.models.is_empty() && now - state.fetched_at < TTL_MS)
                .unwrap_or(false)
        });
        if fresh {
            return ModelRefreshOutcome::unchanged();
        }
    }
    let payload = match client::token_request("GET", MODELS_PATH, session, None, "模型目录查询").await {
        Ok(payload) => payload,
        Err(error) => return ModelRefreshOutcome::failed(error.message),
    };
    let models = parse_models(&payload);
    if models.is_empty() {
        return ModelRefreshOutcome::failed("Loomy 模型目录为空或格式不认识（保留现有清单）");
    }
    let count = models.len();
    with_state(|slot| {
        *slot = Some(CatalogState {
            models: models.clone(),
            fetched_at: now,
        });
    });
    catalog_cache::save(SCOPE, &models, now);
    ModelRefreshOutcome::refreshed(count)
}

/// 模型目录刷新结果（用于 `ProviderAdapter::refresh_models` 的返回）
pub use crate::server::core::providers::adapter::ModelRefreshOutcome;

/// 由 `GatewayError` 起一个「目录不可用」的占位（供目录为空时的适配器兜底）
pub fn unavailable_error() -> GatewayError {
    GatewayError::with_status(503, "Loomy 模型目录尚未拉取：请先在模型管理页点「刷新模型清单」")
}
