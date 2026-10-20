//! ModelScope 模型目录：`GET {推理域}/models`（标准 OpenAI 形态）+ 持久化缓存。
//!
//! ── 上游形态（2026-10-16 真实账号实测）──────────────────────
//! ```text
//! GET https://api-inference.modelscope.cn/v1/models     Authorization: Bearer <ms-令牌>
//!   → {"object":"list","data":[{"id":"Qwen/Qwen3.8-Flash-Next",
//!                               "object":"","owned_by":"system","created":1789…}, …]}
//! ```
//! **只有 `id`**：没有 `name`、没有 `capabilities`、没有上下文长度 ——
//! 与 Loomy / MonkeyCode 那种带能力位的目录不同，这里能如实给出的只有 id 本身。
//!
//! ── 清单是**账号相关**的（这是不做静态兜底表的理由）────────────
//! 实测同一个令牌拉到的是 **36 条**，而不是魔搭全站的几千个模型 ——
//! 它给的是「这个账号在 API-Inference 上能调的模型」。
//! 换句话说清单随账号变（免费额度、实名状态、地区都可能影响它），
//! 因此**编一份静态兜底表是有害的**：用户会看到一批自己其实调不通的模型，
//! 而目录的用途正是回答「我能用什么」。拉不到就如实报失败、保留旧清单
//! —— 与 `loomy::models` 同一取舍。
//!
//! ── 清单里混着非对话模型（照实列出，不筛）────────────────────
//! 实测 36 条里含生图（`Qwen/Qwen-Image-Edit`）、基座（`PaddlePaddle/ERNIE-4.5-*-PT`）
//! 等并非 chat 用途的条目。上游给的就是它的清单，这里**不按 id 猜用途去过滤** ——
//! 猜错会把能用的模型藏起来（用户报「xxx 明明能用却不在列表里」比多几条调不通的
//! 更难查）。真调不通时，`classify_error` 会把上游的 400 如实透出。
//!
//! ── 两个地区两套缓存（拆成两个 provider 的核心收益之一）────────
//! 缓存槽按 scope 分开（`catalog_cache::SCOPE_MODELSCOPE_CN` / `_INTL`），
//! 两个站的清单互不覆盖 —— 这正是当初把地区拆成两家 provider 要解决的问题
//! （见 `region.rs` 的模块头）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::server::core::providers::adapter::ModelRefreshOutcome;
use crate::server::core::providers::catalog_cache;

use super::client;
use super::region::Region;

/// 模型列表路径（相对推理域）
pub const MODELS_PATH: &str = "/models";

/// 自动路径的缓存 TTL（与各家同档）
const TTL_MS: i64 = 10 * 60 * 1000;

/// 进程内缓存（首次访问时从落盘缓存读回）
struct CatalogState {
    models: Vec<Value>,
    fetched_at: i64,
}

/// 两个地区各一个槽（`OnceLock` 初始化，避免在 `static` 里做分配）
static STATE_CN: OnceLock<Mutex<Option<CatalogState>>> = OnceLock::new();
static STATE_INTL: OnceLock<Mutex<Option<CatalogState>>> = OnceLock::new();

/// 本地区对应的落盘缓存 scope
fn scope_of(region: Region) -> &'static str {
    match region {
        Region::Cn => catalog_cache::SCOPE_MODELSCOPE_CN,
        Region::Intl => catalog_cache::SCOPE_MODELSCOPE_INTL,
    }
}

/// 取本地区的状态句柄（毒锁按「上一个持有者 panic 过、数据可能半更新」处理：
/// 本项目的取值是「继续用」，因为这份缓存是可重建的展示数据，不值得让进程挂掉）
fn state(region: Region) -> &'static Mutex<Option<CatalogState>> {
    match region {
        Region::Cn => STATE_CN.get_or_init(|| Mutex::new(None)),
        Region::Intl => STATE_INTL.get_or_init(|| Mutex::new(None)),
    }
}

/// 在锁内执行一段取值，并在首次访问时把落盘缓存装回内存
fn with_state<R>(region: Region, f: impl FnOnce(&mut Option<CatalogState>) -> R) -> R {
    let mutex = state(region);
    let mut guard = mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        if let Some(cached) = catalog_cache::load(scope_of(region)) {
            *guard = Some(CatalogState {
                models: cached.models,
                fetched_at: cached.fetched_at,
            });
        }
    }
    f(&mut guard)
}

/// 把一个 OpenAI 目录条目映射成聚合层认的形态
/// （键名对照 `core/models/shape.rs::list_item`：`id` / `name` / 能力位）。
///
/// 上游只给 `id`（实测），所以 `name` 就用 id 本身：魔搭的 id 是
/// `组织/模型名`（`Qwen/Qwen3.8-Flash-Next`），它本身就是可读的展示名，
/// 拆掉组织段反而会让同名模型（不同组织的同名条目）在界面上无法区分。
///
/// 工具位按 OpenAI 兼容网关的既有能力给 true（照 `autoclaw::models` 的处置：
/// 缺失时保守回落）。**思考位不给** —— 上游没有能力位、按 id 猜会猜错
/// （实测同一个模型既可能走 `reasoning_content` 也可能直接答），
/// 交给模型规则层由用户显式绑定更诚实。
fn map_entry(row: &Value) -> Option<Value> {
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(json!({
        "id": id,
        "name": id,
        "supportsToolCall": true,
    }))
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

/// 本地区当前的模型清单（内存 → 落盘缓存；都没有时为空，由刷新路径填）
pub fn list(region: Region) -> Vec<Value> {
    with_state(region, |slot| {
        slot.as_ref().map(|state| state.models.clone()).unwrap_or_default()
    })
}

/// 最近一次成功刷新的时刻（毫秒；没刷过为 0）—— 界面「更新日期」列读它
pub fn last_refreshed_at(region: Region) -> i64 {
    with_state(region, |slot| slot.as_ref().map(|state| state.fetched_at).unwrap_or(0))
}

/// 这份清单是否来自**远程拉取**（`catalog::refresh_meta` 的「来源」列判据）。
/// 本家没有内置兜底清单：有内容就等于远程拉到过。
pub fn remote_refreshed(region: Region) -> bool {
    with_state(region, |slot| {
        slot.as_ref().map(|state| !state.models.is_empty()).unwrap_or(false)
    })
}

/// 拉一次模型目录并落地（内存 + 持久化缓存）。
///
/// `force = false`（自动路径）时 10 分钟 TTL 内早退为 `unchanged()`；
/// `force = true`（用户手动刷新）真打上游。
pub async fn refresh(region: Region, token: &str, force: bool) -> ModelRefreshOutcome {
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
    let payload = match client::inference_get(region, MODELS_PATH, token, "模型目录查询").await {
        Ok(payload) => payload,
        Err(error) => return ModelRefreshOutcome::failed(error.message),
    };
    let models = parse_models(&payload);
    if models.is_empty() {
        return ModelRefreshOutcome::failed("ModelScope 模型目录为空或格式不认识（保留现有清单）");
    }
    let count = models.len();
    with_state(region, |slot| {
        *slot = Some(CatalogState {
            models: models.clone(),
            fetched_at: now,
        });
    });
    catalog_cache::save(scope_of(region), &models, now);
    ModelRefreshOutcome::refreshed(count)
}

#[cfg(test)]
mod tests {
    use super::{map_entry, parse_models};
    use serde_json::json;

    /// 上游的实测形态（只有 id）必须能过
    #[test]
    fn the_measured_entry_shape_is_mapped() {
        let item = map_entry(&json!({
            "id": "Qwen/Qwen3.8-Flash-Next",
            "object": "",
            "owned_by": "system",
            "created": 1789
        }))
        .unwrap_or_else(|| panic!("实测条目应当被映射"));
        assert_eq!(item.get("id").and_then(|v| v.as_str()), Some("Qwen/Qwen3.8-Flash-Next"));
        // 展示名保留组织段（同名不同组织的条目靠它区分）
        assert_eq!(item.get("name").and_then(|v| v.as_str()), Some("Qwen/Qwen3.8-Flash-Next"));
        assert_eq!(item.get("supportsToolCall").and_then(|v| v.as_bool()), Some(true));
        // 不给思考位：上游没有这个信息，按 id 猜会猜错
        assert!(item.get("supportsReasoning").is_none());
    }

    #[test]
    fn entries_without_an_id_are_dropped() {
        assert!(map_entry(&json!({"object": "model"})).is_none());
        assert!(map_entry(&json!({"id": "   "})).is_none());
        assert!(map_entry(&json!({"id": 42})).is_none());
    }

    /// `{data:[…]}` 与裸数组两种形态都认（OpenAI 兼容实现的常见分歧）
    #[test]
    fn both_envelope_shapes_are_parsed() {
        let wrapped = json!({"object": "list", "data": [{"id": "a"}, {"id": "b"}]});
        let bare = json!([{"id": "a"}, {"id": "b"}]);
        assert_eq!(parse_models(&wrapped).len(), 2);
        assert_eq!(parse_models(&bare).len(), 2);
        assert!(parse_models(&json!({"data": []})).is_empty());
        assert!(parse_models(&json!(null)).is_empty());
    }
}