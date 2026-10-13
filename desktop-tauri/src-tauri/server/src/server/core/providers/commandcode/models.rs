//! Command Code 模型目录：`GET /provider/v1/models` + 5 分钟缓存 + 内置兜底清单。
//!
//! ── 上游形态（规格 §6）──────────────────────────────────────────
//! ```text
//!   { "data": [ { "id": "deepseek/deepseek-v4-flash", "object": "model", … } ] }
//! ```
//! 参考实现只依赖 `data[].id`（`fetchModels`），并且**失败时回落到内置清单**
//! （26 项）—— 本模块照做：远程拉取成功就用远程的，失败 / 拉不到就用手写表，
//! 绝不把目录清空（清了会让「这家有账号但模型列表是空的」，见 `catalog.rs`
//! 的可用性判据：清单为空的家不进 `/v1/models`）。
//!
//! ── 模型 id 的命名规律（规格 §6）────────────────────────────────
//! Anthropic / OpenAI 系**无前缀**（`claude-sonnet-4-6` / `gpt-5.5`），
//! 开源系**带厂商前缀**（`deepseek/...` / `moonshotai/...` / `zai-org/...`）。
//! id 原样透传：本家不做任何名称重写（转发侧 `params.model` 就是这个 id）。
//!
//! ── 能力位的来源（为什么只有图片位是「点名给」的）─────────────────
//! 上游目录**不返回**结构化能力位。工具调用是本家 wire 协议的一等公民
//! （`params.tools` 总是下发，见 `plan.rs`）→ true；思考同样是一等字段
//! （`params.reasoning_effort`，规格 §5.2）→ true。图片位**没有目录依据**，
//! 只有规格 §6 点名的几支（`xiaomi/mimo-v2.5*`、`moonshotai/Kimi-K2.5`、
//! 含 `vision` 的实验模型）给 true，其余 conservative false ——
//! 报错方向只能选「少报」（把不支持图片的模型标成支持，会让客户端发出
//! 上游 400 的请求）。
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 与各家同款：内存缓存 + `providers::catalog_cache` 落盘（进程重启后读回，
//! scope 是 `SCOPE_COMMANDCODE`；本家**没有地区之分**，单格即可）。
//! 自动路径 5 分钟 TTL（参考实现的 `modelRefreshIntervalMs` 口径）；
//! 用户手动「刷新模型清单」时 `force = true` 绕过。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::server::core::auth_http::send_raw;
use crate::server::core::providers::catalog_cache;

use super::endpoints;

/// 本家在 `catalog_cache` 里的 scope 名（单一地区，一格即可）
pub const SCOPE: &str = catalog_cache::SCOPE_COMMANDCODE;

/// 自动路径的缓存 TTL（对齐参考实现的 `modelRefreshIntervalMs`）
const TTL_MS: i64 = 5 * 60 * 1000;

/// 目录查询超时（参考实现给 10 秒）
const REQUEST_TIMEOUT_MS: u64 = 10_000;

/// 内置兜底清单（参考实现 `MODELS` 的逐项复刻：id + 展示名）。
///
/// 上游拉不到时的保底：它同时是「没有账号时点什么」的那张表。
/// id 是转发时直接下发的名字，改一个字母就等于换模型，逐字保留。
const FALLBACK_MODELS: [(&str, &str); 26] = [
    ("claude-sonnet-4-6", "Claude Sonnet 4.6"),
    ("claude-opus-4-8", "Claude Opus 4.8"),
    ("claude-opus-4-7", "Claude Opus 4.7"),
    ("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
    ("gpt-5.5", "GPT-5.5"),
    ("gpt-5.4", "GPT-5.4"),
    ("gpt-5.4-mini", "GPT-5.4 Mini"),
    ("gpt-5.3-codex", "GPT-5.3 Codex"),
    ("deepseek/deepseek-v4-pro", "DeepSeek V4 Pro"),
    ("deepseek/deepseek-v4-flash", "DeepSeek V4 Flash"),
    ("moonshotai/Kimi-K2.6", "Kimi K2.6"),
    ("moonshotai/Kimi-K2.5", "Kimi K2.5"),
    ("zai-org/GLM-5.1", "GLM 5.1"),
    ("zai-org/GLM-5", "GLM 5"),
    ("MiniMaxAI/MiniMax-M3", "MiniMax M3"),
    ("MiniMaxAI/MiniMax-M2.7", "MiniMax M2.7"),
    ("MiniMaxAI/MiniMax-M2.5", "MiniMax M2.5"),
    ("Qwen/Qwen3.6-Max-Preview", "Qwen 3.6 Max Preview"),
    ("Qwen/Qwen3.6-Plus", "Qwen 3.6 Plus"),
    ("Qwen/Qwen3.7-Max", "Qwen 3.7 Max"),
    ("stepfun/Step-3.7-Flash", "Step 3.7 Flash"),
    ("stepfun/Step-3.5-Flash", "Step 3.5 Flash"),
    ("xiaomi/mimo-v2.5-pro", "MiMo V2.5 Pro"),
    ("xiaomi/mimo-v2.5", "MiMo V2.5"),
    ("google/gemini-3.5-flash", "Gemini 3.5 Flash"),
    ("google/gemini-3.1-flash-lite", "Gemini 3.1 Flash Lite"),
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
    let mut guard = mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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

/// 这个模型 id 是否声明支持图片输入（**只有规格点名的几支**，见模块头）
fn supports_images(id: &str) -> bool {
    let lowered = id.to_lowercase();
    lowered.contains("vision") || lowered.contains("mimo-v2.5") || lowered.contains("kimi-k2.5")
}

/// 把一个上游目录条目映射成聚合层认的形态
/// （键名对照 `core/models/shape.rs::list_item`）。
fn map_entry(row: &Value) -> Option<Value> {
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let name = row
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(id);
    Some(json!({
        "id": id,
        "name": name,
        "supportsToolCall": true,
        "supportsReasoning": true,
        "supportsImages": supports_images(id),
    }))
}

/// 内置兜底清单的聚合层形态
fn fallback_models() -> Vec<Value> {
    FALLBACK_MODELS
        .iter()
        .map(|(id, name)| {
            json!({
                "id": id,
                "name": name,
                "supportsToolCall": true,
                "supportsReasoning": true,
                "supportsImages": supports_images(id),
            })
        })
        .collect()
}

/// 解析 `/provider/v1/models` 的响应（`{data:[…]}` 或裸数组）
pub fn parse_models(payload: &Value) -> Vec<Value> {
    let rows = payload
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| payload.as_array())
        .cloned()
        .unwrap_or_default();
    rows.iter().filter_map(map_entry).collect()
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

/// 拉一次模型目录并落地（内存 + 持久化缓存）。
///
/// `force = false`（自动路径）时 5 分钟 TTL 内早退为 `unchanged()`；
/// `force = true`（用户手动刷新）真打上游。
///
/// 失败时的处置：**保留现有清单**（已拉到的远程清单优先，其次内置兜底），
/// 返回 `failed(原因)` 让手动路径如实汇报 —— 自动路径不看返回值，只打日志。
pub async fn refresh(api_key: &str, force: bool) -> ModelRefreshOutcome {
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
    let mut headers = endpoints::cli_headers();
    headers.extend(endpoints::auth_headers(api_key));
    let response = match send_raw(
        "GET",
        &endpoints::url(endpoints::MODELS_PATH),
        None,
        &headers,
        None,
        Some(REQUEST_TIMEOUT_MS),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            return ModelRefreshOutcome::failed(format!(
                "Command Code 模型目录查询失败：{error}（保留现有清单）"
            ))
        }
    };
    if !response.ok {
        return ModelRefreshOutcome::failed(format!(
            "Command Code 模型目录返回 HTTP {}（保留现有清单）",
            response.status
        ));
    }
    let payload = response.payload.unwrap_or(Value::Null);
    let models = parse_models(&payload);
    if models.is_empty() {
        return ModelRefreshOutcome::failed(
            "Command Code 模型目录为空或格式不认识（保留现有清单）",
        );
    }
    let count = models.len();
    with_state(|slot| {
        *slot = Some(CatalogState {
            models: models.clone(),
            fetched_at: now,
            remote: true,
        });
    });
    catalog_cache::save(SCOPE, &models, now);
    ModelRefreshOutcome::refreshed(count)
}

/// 模型目录刷新结果（用于 `ProviderAdapter::refresh_models` 的返回）
pub use crate::server::core::providers::adapter::ModelRefreshOutcome;
