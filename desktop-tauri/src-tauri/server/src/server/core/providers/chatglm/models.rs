//! 智谱清言模型目录：`GET /chatglm/agent-api/operation/detail?tag=available_models`
//! + 持久化缓存 + 内置静态兜底。
//!
//! ── 上游形态（实测，2026-10）────────────────────────────────
//! ```json
//! { "status":0, "result": {
//!     "default": { "guest":"glm-5.3-flash", "vip":"…", "svip":"…",
//!                  "agent":"…", "am":"…", "non_vip":"…" },
//!     "models": [{
//!       "selected_model": "glm-5.3",      ← 发给上游的模型名（本家叫它 selected_model）
//!       "display_name": "GLM-5.3",
//!       "display_tag": [],                 ← 如 ["new"]
//!       "description": "",
//!       "chat_switch": true,               ← 是否出现在主站对话
//!       "am_switch": true,                 ← 是否出现在 AgentMore
//!       "reasoning_efforts": [
//!         { "effort":"",               "am_effort":"low",  "display_name":"快速" },
//!         { "effort":"thinking",       "am_effort":"high", "display_name":"深度" },
//!         { "effort":"deep_thinking",  "am_effort":"max",  "display_name":"极致" }
//!       ]
//!     }]
//! }}
//! ```
//!
//! ── 两条实测结论（写进实现）──────────────────────────────────
//!   1. **清单随账号等级变化**：访客实测只看到 2 个（`glm-5.3` / `glm-5.3-flash`），
//!      登录 / 会员账号能看到更多。因此目录刷新要用**有代表性的账号**
//!      （适配器取库里第一个可用账号，与别家同款）；
//!   2. `reasoning_efforts` 的三档与上游 `meta_data.chat_mode` / `reasoning_effort`
//!      的对应关系是 `effort` 字段（`""` / `thinking` / `deep_thinking`），
//!      本模块只做**透传展示**，不在这里翻译（翻译在 `plan.rs`）。
//!
//! ── 静态兜底 ────────────────────────────────────────────────
//! 拉不到目录时用内置的已知模型名（离线也能点、转发也有名字可用）。
//! 兜底清单是**实测见过的名字**，不编造。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::server::core::providers::catalog_cache;
use crate::server::core::providers::adapter::ModelRefreshOutcome;

use super::{endpoints, http};

/// 本家在 `catalog_cache` 里的 scope 名
pub const SCOPE: &str = catalog_cache::SCOPE_CHATGLM;

/// 自动路径的缓存 TTL（10 分钟，与其余各家同档）
const TTL_MS: i64 = 10 * 60 * 1000;

/// 内置兜底清单（实测见过的名字；拉不到目录时用）
const FALLBACK_MODELS: &[(&str, &str)] = &[
    ("glm-5.3", "GLM-5.3"),
    ("glm-5.3-flash", "GLM-Flash"),
];

/// 进程内缓存（首次访问时从落盘缓存读回）
struct CatalogState {
    models: Vec<Value>,
    fetched_at: i64,
}

static STATE: OnceLock<Mutex<Option<CatalogState>>> = OnceLock::new();

/// 取状态句柄（毒锁按「继续用」处理：这份缓存可重建，不值得让进程挂掉）
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

/// 内置兜底清单（本家上游形态的记录数组）
pub fn fallback_models() -> Vec<Value> {
    FALLBACK_MODELS
        .iter()
        .map(|(id, name)| {
            json!({
                "selected_model": id,
                "display_name": name,
                "description": "",
                "reasoning_efforts": default_efforts(),
            })
        })
        .collect()
}

/// 三档思考等级（与实测的上游形态逐字一致）
fn default_efforts() -> Value {
    json!([
        { "effort": "", "am_effort": "low", "display_name": "快速" },
        { "effort": "thinking", "am_effort": "high", "display_name": "深度" },
        { "effort": "deep_thinking", "am_effort": "max", "display_name": "极致" },
    ])
}

/// 当前清单（远程拉到了就用它，否则内置兜底）
pub fn list() -> Vec<Value> {
    let remote = with_state(|slot| slot.as_ref().map(|state| state.models.clone()));
    match remote {
        Some(models) if !models.is_empty() => models,
        _ => fallback_models(),
    }
}

/// 这份清单是否来自**远程拉取**（`catalog::refresh_meta` 的「来源」列判据）
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

/// 默认模型名（上游 `result.default.guest` 的口径；取不到给兜底第一个）
pub fn default_model() -> String {
    with_state(|slot| {
        slot.as_ref()
            .and_then(|state| state.models.first())
            .and_then(|model| model.get("selected_model"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
    .unwrap_or_else(|| {
        FALLBACK_MODELS
            .first()
            .map(|(id, _)| (*id).to_string())
            .unwrap_or_default()
    })
}

/// 把上游一条模型记录归一成本仓的目录形态（字段名沿用上游，便于对照）。
fn map_entry(entry: &Value) -> Option<Value> {
    let id = entry
        .get("selected_model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let display_name = entry
        .get("display_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(id);
    let description = entry
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    // 思考等级：上游给就带上，没给补默认三档（本家上游恒给，这里只是稳妥）
    let efforts = entry
        .get("reasoning_efforts")
        .filter(|value| value.is_array())
        .cloned()
        .unwrap_or_else(default_efforts);
    let mut record = json!({
        "selected_model": id,
        "display_name": display_name,
        "description": description,
        "reasoning_efforts": efforts,
    });
    if let Some(object) = record.as_object_mut() {
        // 标签（如 ["new"]）原样带上，界面可用来打角标
        if let Some(tags) = entry.get("display_tag").filter(|value| value.is_array()) {
            object.insert("display_tag".to_string(), tags.clone());
        }
        // 开关位原样带上（chat_switch / am_switch）：界面可据此提示
        for key in ["chat_switch", "am_switch", "am_description"] {
            if let Some(value) = entry.get(key) {
                object.insert(key.to_string(), value.clone());
            }
        }
    }
    Some(record)
}

/// 解析上游响应体成记录数组（`result.models`）
pub fn parse_models(payload: &Value) -> Vec<Value> {
    let result = match payload.get("result") {
        Some(value) => value,
        None => payload,
    };
    let Some(list) = result.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter().filter_map(map_entry).collect()
}

/// 从上游响应里读默认模型映射（`result.default`，界面「默认模型」用）
pub fn parse_defaults(payload: &Value) -> Value {
    let result = match payload.get("result") {
        Some(value) => value,
        None => payload,
    };
    result.get("default").cloned().unwrap_or(Value::Null)
}

/// 拉一次模型目录并落地（内存 + 持久化缓存）。
///
/// `force = false`（自动路径）时 TTL 内早退为 `unchanged()`；
/// `force = true`（用户手动刷新）真打上游。
pub async fn refresh(
    token: &str,
    device_id: &str,
    force: bool,
) -> ModelRefreshOutcome {
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
    // 模型目录只读、不需要访客身份的特殊处理：带现有令牌直接问
    let payload = match http::get_json(
        endpoints::PATH_AVAILABLE_MODELS,
        token,
        device_id,
        None,
        "模型目录查询",
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => return ModelRefreshOutcome::failed(error.message),
    };
    let models = parse_models(&payload);
    if models.is_empty() {
        return ModelRefreshOutcome::failed(
            "智谱清言模型目录为空或格式不认识（保留现有清单）",
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 真实响应形态（2026-10 实测抓取）能被解析出模型名与三档等级
    #[test]
    fn parses_real_upstream_shape() {
        let payload = json!({
            "status": 0,
            "message": "success",
            "result": {
                "default": { "guest": "glm-5.3-flash", "vip": "glm-5.3" },
                "models": [
                    {
                        "selected_model": "glm-5.3",
                        "display_name": "GLM-5.3",
                        "display_tag": [],
                        "description": "",
                        "chat_switch": true,
                        "am_switch": true,
                        "reasoning_efforts": [
                            { "effort": "", "am_effort": "low", "display_name": "快速" },
                            { "effort": "thinking", "am_effort": "high", "display_name": "深度" },
                            { "effort": "deep_thinking", "am_effort": "max", "display_name": "极致" }
                        ]
                    },
                    {
                        "selected_model": "glm-5.3-flash",
                        "display_name": "GLM-Flash",
                        "description": "5.3-Flash，回复速度快",
                        "display_tag": ["new"],
                        "reasoning_efforts": []
                    }
                ]
            }
        });
        let models = parse_models(&payload);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["selected_model"], json!("glm-5.3"));
        assert_eq!(models[0]["display_name"], json!("GLM-5.3"));
        // 三档等级原样保留
        let efforts = models[0]["reasoning_efforts"].as_array().map(Vec::len);
        assert_eq!(efforts, Some(3));
        assert_eq!(models[0]["reasoning_efforts"][2]["effort"], json!("deep_thinking"));
        // 标签保留
        assert_eq!(models[1]["display_tag"][0], json!("new"));
        // 默认模型映射
        let defaults = parse_defaults(&payload);
        assert_eq!(defaults["guest"], json!("glm-5.3-flash"));
    }

    /// 缺 selected_model 的条目被丢弃（不能拿空名字去转发）
    #[test]
    fn drops_entries_without_model_name() {
        let payload = json!({
            "result": { "models": [
                { "display_name": "没有 id" },
                { "selected_model": "  " },
                { "selected_model": "ok" }
            ]}
        });
        let models = parse_models(&payload);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["selected_model"], json!("ok"));
    }

    /// 没有 reasoning_efforts 时补默认三档（界面不会空着）
    #[test]
    fn fills_default_efforts() {
        let payload = json!({ "result": { "models": [{ "selected_model": "m" }] } });
        let models = parse_models(&payload);
        assert_eq!(models[0]["display_name"], json!("m"), "缺展示名时用 id 顶替");
        assert_eq!(
            models[0]["reasoning_efforts"].as_array().map(Vec::len),
            Some(3)
        );
    }

    /// 不认识的响应形态 → 空清单（调用方据此报错并保留旧目录）
    #[test]
    fn unknown_shape_yields_empty() {
        assert!(parse_models(&json!({ "result": {} })).is_empty());
        assert!(parse_models(&json!({ "result": { "models": "nope" } })).is_empty());
        assert!(parse_models(&json!({})).is_empty());
    }

    /// 没拉过远程时 `list()` 给内置兜底（离线也能点）
    #[test]
    fn falls_back_to_builtin_list() {
        // 注意：本测试不依赖全局状态（可能已被其他测试改动），只验兜底函数本身
        let fallback = fallback_models();
        assert_eq!(fallback.len(), FALLBACK_MODELS.len());
        assert_eq!(fallback[0]["selected_model"], json!("glm-5.3"));
        assert_eq!(fallback[1]["selected_model"], json!("glm-5.3-flash"));
    }

    /// 兜底清单里的每个名字都带默认三档等级
    #[test]
    fn fallback_entries_carry_efforts() {
        for model in fallback_models() {
            assert_eq!(
                model["reasoning_efforts"].as_array().map(Vec::len),
                Some(3),
                "兜底条目应带三档等级: {model}"
            );
        }
    }
}
