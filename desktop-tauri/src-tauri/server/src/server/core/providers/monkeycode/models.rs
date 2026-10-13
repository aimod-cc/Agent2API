//! MonkeyCode 模型目录：`GET /api/v1/users/models` + 持久化缓存（按地区分格）。
//!
//! ── 上游形态（`mvp/models.py::list_models` + `docs/03-llm/01-model-management-api.md`）──
//! `{"code":0,"data":{"models":[{…}]}}`（也有 `{"models":[…]}` 的兼容形态）。
//! 每个条目的关键字段（参考 `domain/model.go` 的 Model 实体）：
//!   - `id`           模型 UUID（**创建任务时用的 `model_id`**，`task.rs` 反查它）；
//!   - `provider`     提供商名（`siliconflow` / `openai` / …）；
//!   - `model`        上游模型名（`gpt-4o` / `Qwen/Qwen3.5-Plus`）；
//!   - `display_name` 展示名（可能为空）；
//!   - `interface_type` `openai_chat` / `openai_responses` / `anthropic`；
//!   - `context_limit` / `output_limit` 上下文与输出上限（可能为 0）；
//!   - `is_free` / `is_default` / `access_level` 免费 / 默认 / 订阅档。
//!
//! 参考实现（`proxy/src/models.ts`）给下游的模型 id 是
//! `monkeycode/{provider}/{model}` —— 本模块的目录 `id` 采用同一形态
//! （`{provider}/{model}`，provider 为空时退化成 `model`），并把 UUID 与
//! `interface_type` 归一到转发链路要用的字段名（`upstreamId` / `cliName`），
//! 聚合层（`models::list_item`）会忽略不认识的键。
//!
//! ── 目录的 id 与「发什么给上游」是两回事 ────────────────────────
//! 客户端请求的 `id` 是本目录的 `{provider}/{model}`；转发时要用
//! `upstreamId`（UUID）+ `cliName`（由 `interface_type` 映射）去建任务 ——
//! 因此这两个键必须保留在目录条目里，不能只留展示字段。
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 与各家同款：内存缓存 + `providers::catalog_cache` 落盘（进程重启后读回）。
//! **两个地区各占一格**（`catalog_cache::SCOPE_MONKEYCODE_CN` /
//! `SCOPE_MONKEYCODE_INTL`）：两站的模型清单是两份独立数据，合成一格会互相
//! 覆盖。自动路径 10 分钟 TTL；用户手动刷新时 `force = true` 绕过。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use serde_json::{Map, Value};

use crate::server::core::providers::catalog_cache;
use crate::server::errors::GatewayError;

use super::client;
use super::endpoints;
use super::region::Region;

/// 自动路径的缓存 TTL（与客户端自己的节奏同档）
const TTL_MS: i64 = 10 * 60 * 1000;

/// 本家在 `catalog_cache` 里的 scope 名（按地区分格）
pub fn scope_for(region: Region) -> &'static str {
    match region {
        Region::Cn => catalog_cache::SCOPE_MONKEYCODE_CN,
        Region::Intl => catalog_cache::SCOPE_MONKEYCODE_INTL,
    }
}

/// 进程内缓存（每个地区一格；首次访问时从落盘缓存读回）
struct CatalogState {
    models: Vec<Value>,
    fetched_at: i64,
}

/// 两个地区的缓存槽（顺序与 [`Region::ALL`] 一致：国内版在前）
static SLOTS: [OnceLock<Mutex<Option<CatalogState>>>; 2] = [OnceLock::new(), OnceLock::new()];

/// 取某个地区的状态句柄（毒锁按「继续用」处理：这份缓存是可重建的展示数据，
/// 不值得让进程挂掉）
fn slot(region: Region) -> &'static Mutex<Option<CatalogState>> {
    let index = match region {
        Region::Cn => 0,
        Region::Intl => 1,
    };
    SLOTS[index].get_or_init(|| Mutex::new(None))
}

/// 在锁内执行一段取值，并在首次访问时把落盘缓存装回内存
fn with_state<R>(region: Region, f: impl FnOnce(&mut Option<CatalogState>) -> R) -> R {
    let mutex = slot(region);
    let mut guard = mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        if let Some(cached) = catalog_cache::load(scope_for(region)) {
            *guard = Some(CatalogState {
                models: cached.models,
                fetched_at: cached.fetched_at,
            });
        }
    }
    f(&mut guard)
}

/// 从上游条目里取字符串（trim 后非空才算有值）
fn text_of(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 把一个上游模型条目映射成聚合层认的形态。
///
/// 键名对照 `core/models/shape.rs::list_item`：`id` / `name` /
/// `maxInputTokens` / `maxOutputTokens` / `supportsImages` / `supportsVideo` /
/// `supportsReasoning` / `supportsToolCall` / `isDefault`。另有三个**非聚合层**
/// 的键是转发要用的：`upstreamId`（UUID）、`cliName`、`interfaceType`
/// （聚合层原样忽略未知键）。
///
/// 没有 `model` 字段的条目直接跳过（没有上游模型名就无法建任务）；
/// `display_name` 空时用 `model` 兜底展示名。
fn map_entry(row: &Value) -> Option<Value> {
    let model_name = text_of(row, "model");
    if model_name.is_empty() {
        return None;
    }
    let provider = text_of(row, "provider");
    // 目录 id：`{provider}/{model}`（provider 为空时退化成 `model`）—— 与参考
    // 实现 `toOpenAIModelId` 同形态（去掉 `monkeycode/` 前缀，因为本网关
    // 用 `owned_by`/provider 区分归属）
    let id = if provider.is_empty() {
        model_name.clone()
    } else {
        format!("{provider}/{model_name}")
    };
    let display_name = text_of(row, "display_name");
    let name = if display_name.is_empty() {
        model_name.clone()
    } else {
        display_name
    };
    let interface_type = text_of(row, "interface_type");
    let mut item = Map::new();
    item.insert("id".to_string(), Value::String(id));
    item.insert("name".to_string(), Value::String(name));
    item.insert("provider".to_string(), Value::String(provider));
    item.insert("model".to_string(), Value::String(model_name));
    item.insert(
        "upstreamId".to_string(),
        Value::String(text_of(row, "id")),
    );
    item.insert(
        "interfaceType".to_string(),
        Value::String(interface_type.clone()),
    );
    // interface_type → coding agent（CLI）名，建任务时用
    item.insert(
        "cliName".to_string(),
        Value::String(endpoints::cli_name_for(&interface_type).to_string()),
    );
    // 能力位：本家目录没有 capabilities 对象。工具调用是**上游 coding agent
    // 网关**的固有能力（opencode / codex / claude 都走工具循环），给 true；
    // 思考位如实取 `thinking_enabled`（创建模型时的字段，目录里可能缺失）；
    // 图片位参考没给依据，保守 false。
    item.insert("supportsToolCall".to_string(), Value::Bool(true));
    item.insert("supportsImages".to_string(), Value::Bool(false));
    item.insert("supportsVideo".to_string(), Value::Bool(false));
    item.insert(
        "supportsReasoning".to_string(),
        Value::Bool(
            row.get("thinking_enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
    );
    item.insert(
        "isDefault".to_string(),
        Value::Bool(row.get("is_default").and_then(Value::as_bool).unwrap_or(false)),
    );
    // 免费 / 订阅档：非聚合层字段，但账号页与排障会看（保留原样）
    if let Some(is_free) = row.get("is_free").and_then(Value::as_bool) {
        item.insert("isFree".to_string(), Value::Bool(is_free));
    }
    let access_level = text_of(row, "access_level");
    if !access_level.is_empty() {
        item.insert("accessLevel".to_string(), Value::String(access_level));
    }
    for (key, target) in [("context_limit", "maxInputTokens"), ("output_limit", "maxOutputTokens")] {
        if let Some(value) = row.get(key).and_then(Value::as_i64) {
            if value > 0 {
                item.insert(target.to_string(), Value::from(value));
            }
        }
    }
    Some(Value::Object(item))
}

/// 解析 `/api/v1/users/models` 响应（`{code,data:{models:[]}}` 或 `{models:[]}`）
pub fn parse_models(payload: &Value) -> Vec<Value> {
    let data = payload.get("data").unwrap_or(payload);
    let rows = data
        .get("models")
        .and_then(Value::as_array)
        .or_else(|| data.as_array())
        .cloned()
        .unwrap_or_default();
    rows.iter().filter_map(map_entry).collect()
}

/// 建任务要用的目录字段（客户端请求的模型名 → 上游 UUID / Agent 类型）。
///
/// `model_id` 是建任务的 `model_id`（上游模型 UUID），`cli_name` 决定 VM 里
/// 装哪个 coding agent（opencode / codex / claude）—— 两者都**不在**展示字段
/// 里，只能从目录条目反查，因此本结构是转发链路的必需输入（`task.rs`）。
pub struct TaskTarget {
    /// 上游模型 UUID（建任务的 `model_id`）
    pub model_id: String,
    /// Agent 类型（建任务的 `cli_name`）
    pub cli_name: String,
    /// 上游模型名（日志与报错文案用）
    pub model: String,
}

/// 按客户端请求的名字反查建任务参数。
///
/// ── 判据链（都是精确匹配，不做模糊 / 默认回落）──────────────────
///   1. 目录 id（`{provider}/{model}`，客户端点名的就是这个）—— 主判据；
///   2. 上游真名 `model`（客户端可能直接写上游名）；
///   3. 展示名 `name`；
///   4. `upstreamId`（UUID，排障时可能直接点）。
/// 全部大小写不敏感。取到的条目必须带**非空** `upstreamId`（没有 UUID 就建不出
/// 任务），否则视为查不到 —— 返回 `None` 让调用方报可读错误，而不是拿空串去撞
/// 上游的 400。
///
/// ── 为什么不做参考实现那样的「模糊匹配 + 默认模型回落」────────────
/// 参考的 `resolveModel` 在找不到时回落到 `is_default` / `models[0]`。网关侧
/// 不能这么做：入口校验已按**广告视图**解析过模型名（`api::pipeline`），走到
/// 转发这一步时名字必然是清单里的某一个；把「找不到」静默换成另一个模型，
/// 就是「选了 A、用的是 B」—— 本仓对这类静默错误一律报错（见
/// `account_store::session_for_account` 的同一条纪律）。
pub fn resolve_task_model(region: Region, requested: &str) -> Option<TaskTarget> {
    let requested = requested.trim();
    if requested.is_empty() {
        return None;
    }
    let models = list(region);
    let matched = models
        .iter()
        .find(|item| text_of(item, "id").eq_ignore_ascii_case(requested))
        .or_else(|| {
            models
                .iter()
                .find(|item| text_of(item, "model").eq_ignore_ascii_case(requested))
        })
        .or_else(|| {
            models
                .iter()
                .find(|item| text_of(item, "name").eq_ignore_ascii_case(requested))
        })
        .or_else(|| {
            models
                .iter()
                .find(|item| text_of(item, "upstreamId").eq_ignore_ascii_case(requested))
        })?;
    let model_id = text_of(matched, "upstreamId");
    if model_id.is_empty() {
        return None;
    }
    let cli_name = {
        let stored = text_of(matched, "cliName");
        if stored.is_empty() {
            endpoints::cli_name_for(&text_of(matched, "interfaceType")).to_string()
        } else {
            stored
        }
    };
    Some(TaskTarget {
        model_id,
        cli_name,
        model: text_of(matched, "model"),
    })
}

/// 当前模型清单（内存 → 落盘缓存；都没有时为空，由刷新路径填）
pub fn list(region: Region) -> Vec<Value> {
    with_state(region, |slot| {
        slot.as_ref().map(|state| state.models.clone()).unwrap_or_default()
    })
}

/// 这份清单是否来自**远程拉取**（`catalog::refresh_meta` 的「来源」列判据）。
/// 上游没有内置兜底清单：有内容就等于远程拉到过。
pub fn remote_refreshed(region: Region) -> bool {
    with_state(region, |slot| {
        slot.as_ref()
            .map(|state| !state.models.is_empty())
            .unwrap_or(false)
    })
}

/// 最近一次成功刷新的时刻（毫秒；没刷过为 0）—— 界面「更新日期」列读它
pub fn last_refreshed_at(region: Region) -> i64 {
    with_state(region, |slot| slot.as_ref().map(|state| state.fetched_at).unwrap_or(0))
}

/// 拉一次模型目录并落地（内存 + 持久化缓存）。
///
/// `force = false`（自动路径）时 10 分钟 TTL 内早退为 `unchanged()`；
/// `force = true`（用户手动刷新）真打上游。
pub async fn refresh(region: Region, session: &str, force: bool) -> ModelRefreshOutcome {
    let now = crate::server::logging::now_ms();
    if !force {
        let fresh = with_state(region, |slot| {
            slot.as_ref()
                .map(|state| !state.models.is_empty() && now - state.fetched_at < TTL_MS)
                .unwrap_or(false)
        });
        if fresh {
            return ModelRefreshOutcome::unchanged();
        }
    }
    let payload = match client::get_json(
        region,
        endpoints::MODELS_PATH,
        session,
        "模型目录查询",
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => return ModelRefreshOutcome::failed(error.message),
    };
    let models = parse_models(&payload);
    if models.is_empty() {
        return ModelRefreshOutcome::failed("MonkeyCode 模型目录为空或格式不认识（保留现有清单）");
    }
    let count = models.len();
    with_state(region, |slot| {
        *slot = Some(CatalogState {
            models: models.clone(),
            fetched_at: now,
        });
    });
    catalog_cache::save(scope_for(region), &models, now);
    ModelRefreshOutcome::refreshed(count)
}

/// 模型目录刷新结果（用于 `ProviderAdapter::refresh_models` 的返回）
pub use crate::server::core::providers::adapter::ModelRefreshOutcome;

/// 由 `GatewayError` 起一个「目录不可用」的占位（供目录为空时的适配器兜底）
pub fn unavailable_error() -> GatewayError {
    GatewayError::with_status(
        503,
        "MonkeyCode 模型目录尚未拉取：请先在模型管理页点「刷新模型清单」",
    )
}
