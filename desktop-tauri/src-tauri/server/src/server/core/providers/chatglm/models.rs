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
//! ── 条目必须归一成仓内约定的键名（别照搬上游字段名）──────────────
//! 上游给的是 `selected_model` / `display_name`，而全仓的模型条目一律读
//! `id` / `name`（`models::shape::model_id` 读 `id`、`list_item` 读 `id`+`name`，
//! 其余九家都归一到这两个键）。**照搬上游键名的后果是整家静默消失**：
//! `model_id()` 读出空串 ⇒ `public_builtin_items` 里 `claimed` 去重后拿不到
//! `upstream` 条目 ⇒ 一条模型都进不了 `/v1/models` 与模型管理页，
//! 用户看到的就是「模型管理里根本没有这家、请求也调不通」——
//! 目录其实早就拉到了（缓存里躺着），只是名字没人认得。
//! 与 `StoredAccount` 那次「嵌套 auth 被静默过滤」同一类故障：**形状对不上比没有更隐蔽**。
//!
//! `reasoning_efforts` 同理归一成仓里认的 `reasoningEfforts`
//! （`model_meta::fill` 的 `LEVEL_KEYS` 认它；上游那个下划线形态它不认，
//! 思考等级规格因此一直是空的）。
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
///
/// 形状与远程条目**逐字一致**（含 `reasoning_efforts` 归一后的
/// `reasoningEfforts`）—— 兜底与远程两个来源产出同一个形状，
/// 下游（`model_meta::fill` 读等级、界面读展示名）才不必分辨清单来自哪边。
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

/// 内置兜底清单（本家上游形态的记录数组，**已归一成仓内键名**）
pub fn fallback_models() -> Vec<Value> {
    FALLBACK_MODELS
        .iter()
        .map(|(id, name)| {
            json!({
                "id": id,
                "name": name,
                "description": "",
                "reasoningEfforts": default_efforts(),
            })
        })
        .collect()
}

/// 三档思考等级（取值与实测的上游 `effort` 字段逐字一致）
fn default_efforts() -> Value {
    json!([
        { "effort": "", "am_effort": "low", "display_name": "快速" },
        { "effort": "thinking", "am_effort": "high", "display_name": "深度" },
        { "effort": "deep_thinking", "am_effort": "max", "display_name": "极致" },
    ])
}

/// 从一条目录条目里取模型名：**先认仓内键 `id`，再兼容历史形状的
/// `selected_model`**。
///
/// ── 为什么读取侧要认两种形状 ──────────────────────────────────
/// `catalog_cache::load` 是通用的（各家共用一份落盘缓存），它**不做任何归一**，
/// 把库里那份原样装回内存。于是「首版用上游键名缓存下来的那份清单」会在升级后
/// 继续被读出来 —— 只认 `id` 的话，用户升级完照样看不到这家，且要等下一次
/// 目录刷新（最多 10 分钟 TTL / 或手动点刷新）才会自愈。
///
/// 与 `credentials::from_record` 认「嵌套 auth 与顶层」两种形状同一处置：
/// **读取侧兼容历史数据，写入侧只产一种形状**（见 `map_entry`）。
fn entry_model_name(entry: &Value) -> Option<&str> {
    entry
        .get("id")
        .or_else(|| entry.get("selected_model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// 从一条目录条目里取展示名：先认 `name`，再兼容历史形状的 `display_name`，
/// 最后回落到模型名本身（与 `map_entry` 的兜底一致）。
fn entry_display_name<'a>(entry: &'a Value, fallback: &'a str) -> &'a str {
    entry
        .get("name")
        .or_else(|| entry.get("display_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

/// 当前清单（远程拉到了就用它，否则内置兜底）。
///
/// 出厂前过一次 [`normalize_entry`]：内存里那份可能来自历史形状的落盘缓存
/// （见 `entry_model_name` 的说明），归一之后下游（`model_id` / `list_item` /
/// 界面）才只看一种形状 —— 否则「读入口认两种、出口只有一种」这条约定会在
/// 缓存这条路上漏掉。
pub fn list() -> Vec<Value> {
    let remote = with_state(|slot| slot.as_ref().map(|state| state.models.clone()));
    match remote {
        Some(models) if !models.is_empty() => models.iter().map(normalize_entry).collect(),
        _ => fallback_models(),
    }
}

/// 把一条（可能来自历史缓存的）条目归一成仓内形状。
///
/// 已经是新形状的原样返回（幂等）；旧形状的按 [`map_entry`] 的键名翻译一次。
fn normalize_entry(entry: &Value) -> Value {
    // 新形状的标记：`id` 与 `reasoningEfforts` 都在 —— 那两个是首版都没有的键
    if entry.get("id").is_some() && entry.get("reasoningEfforts").is_some() {
        return entry.clone();
    }
    let Some(id) = entry_model_name(entry) else {
        return entry.clone();
    };
    let name = entry_display_name(entry, id);
    let description = entry.get("description").and_then(Value::as_str).unwrap_or("");
    let efforts = entry
        .get("reasoningEfforts")
        .or_else(|| entry.get("reasoning_efforts"))
        .filter(|value| value.is_array())
        .cloned()
        .unwrap_or_else(default_efforts);
    let mut record = json!({
        "id": id,
        "name": name,
        "description": description,
        "reasoningEfforts": efforts,
    });
    if let Some(object) = record.as_object_mut() {
        if let Some(tags) = entry.get("display_tag").filter(|value| value.is_array()) {
            object.insert("display_tag".to_string(), tags.clone());
        }
        for key in ["chat_switch", "am_switch", "am_description"] {
            if let Some(value) = entry.get(key) {
                object.insert(key.to_string(), value.clone());
            }
        }
    }
    record
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
            // 认两种形状：内存里这份可能来自历史缓存（见 `entry_model_name`）
            .and_then(entry_model_name)
            .map(str::to_string)
    })
    .unwrap_or_else(|| {
        FALLBACK_MODELS
            .first()
            .map(|(id, _)| (*id).to_string())
            .unwrap_or_default()
    })
}

/// 把上游一条模型记录归一成本仓的目录形态。
///
/// ── 键名必须翻译，不能照搬（见模块头）──────────────────────────
/// 上游叫 `selected_model` / `display_name` / `reasoning_efforts`，
/// 仓内一律 `id` / `name` / `reasoningEfforts`。`model_id()` 只认 `id` ——
/// 照搬上游键名等于把整家的模型都变成「没有名字的条目」，一条都进不了
/// 对外列表（症状见模块头）。
///
/// 上游那套下划线字段名**不保留**：留着它们只会制造第二份「模型叫什么」
/// 的读法，而没有一个消费方认（转发侧读的是编排层改写后的 `body.model`）。
fn map_entry(entry: &Value) -> Option<Value> {
    let id = entry
        .get("selected_model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let name = entry_display_name(entry, id);
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
        "id": id,
        "name": name,
        "description": description,
        "reasoningEfforts": efforts,
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
        // ── 键名必须是仓内约定（这是那扇让整家消失的门，见模块头）──────
        assert_eq!(models[0]["id"], json!("glm-5.3"));
        assert_eq!(models[0]["name"], json!("GLM-5.3"));
        assert_eq!(models[1]["id"], json!("glm-5.3-flash"));
        assert_eq!(models[1]["name"], json!("GLM-Flash"));
        // 上游那套下划线键名不该继续出现在条目里（留着就是第二份读法）
        assert!(models[0].get("selected_model").is_none());
        assert!(models[0].get("display_name").is_none());
        assert!(models[0].get("reasoning_efforts").is_none());
        // 三档等级归一成仓里认的键（`model_meta::fill` 的 LEVEL_KEYS 认它）
        let efforts = models[0]["reasoningEfforts"].as_array().map(Vec::len);
        assert_eq!(efforts, Some(3));
        assert_eq!(
            models[0]["reasoningEfforts"][2]["effort"],
            json!("deep_thinking")
        );
        // 标签保留
        assert_eq!(models[1]["display_tag"][0], json!("new"));
        // 默认模型映射
        let defaults = parse_defaults(&payload);
        assert_eq!(defaults["guest"], json!("glm-5.3-flash"));
    }

    /// **每条目录条目都必须带非空 `id`** —— 这是它能不能进对外列表的充要条件。
    ///
    /// ── 锁的是真实发生过的故障 ──────────────────────────────────
    /// 首版把上游的 `selected_model` 原样当条目键名，`model_id()`（只读 `id`）
    /// 于是在每条上都读出空串：模型管理页里整家不见、请求也调不通，而目录
    /// 其实早就拉到并缓存好了。症状与「没加账号」极像，极难自查 ——
    /// 因此拿这条测试把形状钉住。
    #[test]
    fn every_entry_carries_a_usable_id() {
        use crate::server::core::models::model_id;
        let payload = json!({
            "result": { "models": [
                { "selected_model": "glm-5.3", "display_name": "GLM-5.3" },
                { "selected_model": "glm-5.3-flash" },
            ]}
        });
        for model in parse_models(&payload) {
            assert!(
                !model_id(&model).is_empty(),
                "条目缺 id 会让整家从 /v1/models 消失: {model}"
            );
        }
        // 兜底清单走同一条判据（两个来源必须同形）
        for model in fallback_models() {
            assert!(
                !model_id(&model).is_empty(),
                "兜底条目同样必须带 id: {model}"
            );
        }
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
        assert_eq!(models[0]["id"], json!("ok"));
    }

    /// 没有 reasoning_efforts 时补默认三档（界面不会空着）
    #[test]
    fn fills_default_efforts() {
        let payload = json!({ "result": { "models": [{ "selected_model": "m" }] } });
        let models = parse_models(&payload);
        assert_eq!(models[0]["name"], json!("m"), "缺展示名时用 id 顶替");
        assert_eq!(
            models[0]["reasoningEfforts"].as_array().map(Vec::len),
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
        assert_eq!(fallback[0]["id"], json!("glm-5.3"));
        assert_eq!(fallback[1]["id"], json!("glm-5.3-flash"));
    }

    /// 兜底清单里的每个名字都带默认三档等级
    #[test]
    fn fallback_entries_carry_efforts() {
        for model in fallback_models() {
            assert_eq!(
                model["reasoningEfforts"].as_array().map(Vec::len),
                Some(3),
                "兜底条目应带三档等级: {model}"
            );
        }
    }

    /// **历史缓存的旧形状也能读出来**：`catalog_cache::load` 不做归一，会把
    /// 首版用上游键名缓存的那份清单原样装回内存 —— 只认新形状的话，用户升级完
    /// 要等下一次目录刷新才自愈（见 `entry_model_name` 的说明）。
    #[test]
    fn reads_legacy_cached_shape() {
        use crate::server::core::models::model_id;
        let legacy = json!({
            "selected_model": "glm-5.3",
            "display_name": "GLM-5.3",
            "description": "",
            "reasoning_efforts": [{ "effort": "thinking", "display_name": "深度" }]
        });
        let normalized = normalize_entry(&legacy);
        assert_eq!(model_id(&normalized), "glm-5.3", "旧形状必须能读出模型名");
        assert_eq!(normalized["name"], json!("GLM-5.3"));
        assert_eq!(normalized["reasoningEfforts"].as_array().map(Vec::len), Some(1));
        // 幂等：归一过的再归一一次不变
        assert_eq!(normalize_entry(&normalized), normalized);
    }

    /// 旧形状缺展示名时用模型名顶替（与 `map_entry` 同一兜底）
    #[test]
    fn legacy_shape_falls_back_to_id_for_name() {
        let legacy = json!({ "selected_model": "glm-5.3-flash" });
        let normalized = normalize_entry(&legacy);
        assert_eq!(normalized["name"], json!("glm-5.3-flash"));
    }
}
