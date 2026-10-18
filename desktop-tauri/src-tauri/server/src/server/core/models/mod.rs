//! WorkBuddy 模型目录（对照 Node 版 src/workbuddy-models.mjs 全量移植）。
//!
//! 模型清单内嵌于 WorkBuddy 内置 CLI 的 product.json（v5.3.14）。这里收录
//! 主要对话/多模态模型，作为 /v1/models 响应与默认模型回退。
//!
//! 运行时权威来源：`GET {endpoint}/v3/config` → `data.models`（企业账号再取
//! `{endpoint}/console/enterprises/{id}/config/models`，本模块一并实现）。
//! 注意：服务端按官方客户端 UA 区分来源，缺失 UA 时 /v3/config 的 models
//! 可能返回 null，因此 refresh 时必须带 User-Agent。
//!
//! ── 远程清单的持久化（与 Node 版的唯一一处有意分叉）──────────
//! 任务书提到「只在版本变化时写盘 {config_dir}/models.json」；早先按「Node 版
//! 没有任何磁盘缓存」（全仓 grep `models.json` 零命中，workbuddy-models.mjs
//! 全文没有 fs 调用）移植时**没有实现**，目录因此是纯内存态：进程重启回到
//! 内置清单，再由启动时/每次 GET /v1/models 的异步刷新重新拉取。
//!
//! 现在补上了，形态与任务书不同：**不写独立文件，进统一库的 `kv` 表**
//! （`providers::catalog_cache`）。改主意的理由：纯内存态下，重启那次刷新
//! 若失败（网络 / 无账号 / 上游报错），用户看到的就是内置清单 —— 上次拉到的
//! 新清单整个丢失（上游新增的模型消失、已下架的模型回来被广告出去）。
//! 这个分叉是有意的：Node 版没有「多提供商 + 管理页」这套界面，清单丢失的
//! 影响面比现在小得多。缓存的形态与「为什么不留有效期」见那个模块的模块头。
//!
//! ── 并发 ──────────────────────────────────────────────────
//! 目录是「读多写少」的共享态：句柄内一把 `RwLock`，读路径（/v1/models、
//! /health、聊天路由的模型校验，以及经聚合目录间接读它的 /api/session）
//! 拿读锁，刷新落地时拿写锁。
//! 硬约束（与账号存储相同）：**持锁期间绝不做网络请求** —— 刷新先把请求
//! 发完、解析完，最后才在锁内做一次整体替换。
//!
//! ── 多提供商（Agent2API 改造 W2a-T2）后的定位 ──────────────
//! 本模块**只负责 workbuddy 这一家的清单**（内置清单 + /v3/config 远程刷新），
//! 它继续是 workbuddy 的清单事实来源。跨 provider 的合并视图在
//! `core::providers::catalog`（聚合目录），两边共用 `shape.rs` 的「来源无关」
//! 构造件（条目字段映射、响应信封、相近提示纯函数）——
//! 于是「单家清单」与「聚合清单」永远不会在字段映射上分叉。
//!
//! ── 国内版 / 国际版拆家后：**一家两份清单**（2026-10）─────────
//! WorkBuddy 的国际版是独立 provider（`workbuddy-intl`，见
//! `providers::workbuddy::region`），它有**自己的** `/v3/config` 清单与自己的
//! 持久化缓存槽。两个地区的清单在结构上是两份独立数据（上游按站点下发各自
//! 的模型表），因此本模块的目录句柄从「一个单例」变成「按地区各一个」：
//! `global_catalog(region)` 返回对应地区那一份，`ModelCatalog` 自己也记住
//! 「我是谁的清单」（provider id / 缓存 scope / 版本），于是 `owned_by`、
//! 缓存读写、默认规则种子的 provider 键都从实例上取，而不是写死常量。
//!
//! 拆家前这两份清单挤在同一个 `OnceLock` 里：用国内账号拉一次、再用国际账号
//! 拉一次会**互相覆盖**（issue #74 的根因）。现在各刷各的、各自恢复各自的缓存。
//!
//! 内置兜底清单（`builtin_models`）两地共用同一份：它是「远程不可用时的兜底」，
//! 拆家前国际版用户看到的就是它，改成空清单会让首次启动且刷新失败时国际版
//! 一个模型都没有（比现状退化）。
//!
//! 词法上本模块是个目录（`models/mod.rs` + `models/shape.rs`）：拆分的唯一
//! 理由是单文件行数约定（≤800 行），职责边界与拆分前完全一致。
//!
//! ── 进程级句柄 ──────────────────────────────────────────────
//! `global_catalog(region)` 是聚合层的读取口（每个地区一个 `OnceLock` 单例，
//! 与 `config::init` 同一模式）：`ServerState::bootstrap` 与聚合层
//! 拿到的是同一实例，刷新对两边同时可见。

mod shape;

use std::sync::{Arc, OnceLock, RwLock};

use serde_json::{json, Value};

use crate::server::core::auth_http::send_raw;
use crate::server::core::endpoints::{normalize_endpoint, user_agent_for_edition};
use crate::server::core::providers::workbuddy::region::Region;
use crate::server::core::providers::catalog_cache;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::logging;

// 本模块内部用（is_chat_model / apply_filters / get / apply_remote / list_response）
use shape::{js_truthy, value_text};

// 对外再导出：聚合目录（core::providers::catalog）与排障方从 `core::models`
// 取这些名字，保持与拆分前同一条导入路径。
// `shape_value_text` 是 `shape::value_text` 的别名导出：聚合目录要按 `name`
// 匹配模型（对齐 `ModelCatalog::get` 的第二段），需要与这里同一套 JS 文本化。
pub use shape::{list_item, list_response_from, model_id, suggest_from, value_text as shape_value_text};

/// 下游可见模型白名单：/v1/models 与路由解析只暴露这些模型。
///
/// 空数组 = 不限制，全量放行（/v3/config 拉回的真实清单，约 51 个模型）。
/// 与 Node 版一样保持空数组 —— 需要收窄时填入模型 id。
pub const MODEL_ALLOWLIST: &[&str] = &[];

/// 非对话模型的排除前缀，照抄 Node 版 CHAT_MODEL_EXCLUDE_RE 的九个分支。
///
/// 判定依据来自 /v3/config 下发的模型元数据（实测 2026-09-10）：
///   - 内部补全/工具模型带 supportsExtra，输出上限 ≤8192
///   - 图像模型没有 maxOutputTokens（只有 id/name/tags）
///   - 旧一代小输出对话模型（r1-0528 / v3-0324 / kimi-k2-instruct / Claude 内部 id）
///     由黑名单排除，官方客户端 UI 也不展示它们
///
/// 注意 `default-1.` 这一条会把内置清单里的 `default-1.1` / `default-1.2`
/// 一起排除 —— 这是 Node 版的既有行为（那两个是 Claude 的内部 id），
/// 移植时保持一致，不做「顺手修复」。
const CHAT_MODEL_EXCLUDE_PREFIXES: &[&str] = &[
    "completion-",
    "codewise-",
    "hunyuan-image",
    "hunyuan-3b",
    "hunyuan-7b",
    "deepseek-r1-0528",
    "deepseek-v3-0324",
    "kimi-k2-instruct",
    "default-1.",
];

/// 远程目录刷新失败时保留内置目录的说明（Node 的 reason 文案）
const REASON_NO_SESSION: &str = "缺少登录态，使用内置模型目录";

/// 内置模型目录（id → 元数据），逐字段照抄 Node 版 BUILTIN_MODELS。
///
/// credits 字段为桌面端展示的单价，仅作参考。
fn builtin_models() -> Vec<Value> {
    json!([
        { "id": "auto", "name": "Auto", "vendor": "f", "credits": "", "maxOutputTokens": 32000, "maxInputTokens": 168000, "supportsToolCall": true, "supportsImages": true, "isDefault": true, "descriptionZh": "平衡效果与速度" },
        { "id": "default", "name": "Default", "vendor": "v", "credits": "x2.00 credits", "maxOutputTokens": 24000, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": false },

        { "id": "deepseek-v4-pro", "name": "Deepseek-V4-Pro", "vendor": "f", "credits": "x0.16 credits", "maxOutputTokens": 50000, "maxInputTokens": 1000000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true, "descriptionZh": "DeepSeek 旗舰模型，支持 1M 上下文窗口" },
        { "id": "deepseek-v4-flash", "name": "Deepseek-V4-Flash", "vendor": "f", "credits": "x0.06 credits", "maxOutputTokens": 50000, "maxInputTokens": 1000000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true, "descriptionZh": "DeepSeek 旗舰模型，支持 1M 上下文窗口" },
        // deepseek-v4.1-flash 不在 5.3.14 内置清单/旧版 /v3/config 里，但上游实测可路由；
        // 作为远程清单不可用时的兜底定义（远程正常时以远程元数据为准）。
        { "id": "deepseek-v4.1-flash", "name": "Deepseek-V4.1-Flash", "vendor": "f", "credits": "x0.03 credits", "maxOutputTokens": 50000, "maxInputTokens": 1000000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true, "descriptionZh": "DeepSeek 快速模型，支持 1M 上下文窗口" },
        { "id": "deepseek-v3-2-volc", "name": "DeepSeek-V3.2", "vendor": "f", "credits": "", "maxOutputTokens": 32000, "maxInputTokens": 96000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },

        { "id": "glm-5.1", "name": "GLM-5.1", "vendor": "e", "credits": "", "maxOutputTokens": 48000, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "glm-5.0", "name": "GLM-5.0", "vendor": "f", "credits": "", "maxOutputTokens": 48000, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "glm-5.0-turbo", "name": "GLM-5.0-Turbo", "vendor": "e", "credits": "", "maxOutputTokens": 48000, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "glm-5v-turbo", "name": "GLM-5v-Turbo", "vendor": "e", "credits": "", "maxOutputTokens": 38000, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "glm-4.7", "name": "GLM-4.7", "vendor": "f", "credits": "", "maxOutputTokens": 48000, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "glm-4.6", "name": "GLM-4.6", "vendor": "f", "credits": "", "maxOutputTokens": 32000, "maxInputTokens": 168000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true },
        { "id": "glm-4.6v", "name": "GLM-4.6V", "vendor": "f", "credits": "", "maxOutputTokens": 32000, "maxInputTokens": 128000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },

        { "id": "minimax-m2.5", "name": "MiniMax-M2.5", "vendor": "f", "credits": "", "maxOutputTokens": 48000, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "minimax-m2.7", "name": "MiniMax-M2.7", "vendor": "f", "credits": "", "maxOutputTokens": 48000, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "minimax-m3-play", "name": "MiniMax-M3", "vendor": "f", "credits": "x0.25 credits", "maxOutputTokens": 48000, "maxInputTokens": 512000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true, "descriptionZh": "原生多模态，擅长代码、智能体任务" },

        { "id": "kimi-k2.6", "name": "Kimi-K2.6", "vendor": "f", "credits": "", "maxOutputTokens": 32000, "maxInputTokens": 256000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "kimi-k2.5", "name": "Kimi-K2.5", "vendor": "f", "credits": "", "maxOutputTokens": 32000, "maxInputTokens": 256000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "kimi-k2-thinking", "name": "Kimi-K2-Thinking", "vendor": "f", "credits": "", "maxOutputTokens": 32000, "maxInputTokens": 256000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "kimi-k2-instruct-taiji", "name": "Kimi-K2", "vendor": "f", "credits": "", "maxOutputTokens": 8192, "maxInputTokens": 31000, "supportsToolCall": true, "supportsImages": true },

        { "id": "hy3-preview", "name": "Hy3 preview", "vendor": "j", "credits": "", "maxOutputTokens": 64000, "maxInputTokens": 192000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "hy3-preview-agent", "name": "Hy3 preview", "vendor": "j", "credits": "x0.04 credits", "maxOutputTokens": 64000, "maxInputTokens": 192000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "hunyuan-chat", "name": "Hunyuan-Turbos", "vendor": "j", "credits": "", "maxOutputTokens": 8192, "maxInputTokens": 128000, "supportsToolCall": true, "supportsImages": true, "descriptionZh": "腾讯自研的轻量、快速的通用模型" },
        { "id": "hunyuan-2.0-instruct", "name": "Hunyuan-2.0-Instruct", "vendor": "j", "credits": "", "maxOutputTokens": 16000, "maxInputTokens": 128000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "hunyuan-2.0-thinking", "name": "Hunyuan-2.0-Thinking", "vendor": "e", "credits": "", "maxOutputTokens": 24000, "maxInputTokens": 128000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },

        { "id": "default-1.1", "name": "Claude-3.7-Sonnet", "vendor": "e", "credits": "", "maxOutputTokens": 8192, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
        { "id": "default-1.2", "name": "Claude-4.0-Sonnet", "vendor": "e", "credits": "", "maxOutputTokens": 24000, "maxInputTokens": 200000, "supportsToolCall": true, "supportsImages": true, "supportsReasoning": true, "onlyReasoning": true },
    ])
    .as_array()
    .cloned()
    .unwrap_or_default()
}

/// 对话模型判定：网关只服务对话模型，非对话项（补全/内部工具/图像小模型）
/// 不进目录 —— /v1/models、/api/session（经聚合目录）与路由解析三者保持一致。
pub fn is_chat_model(model: &Value) -> bool {
    let id = model.get("id").map(value_text).unwrap_or_default();
    if CHAT_MODEL_EXCLUDE_PREFIXES
        .iter()
        .any(|prefix| id.starts_with(prefix))
    {
        return false;
    }
    // `model.supportsExtra` 真值判定（任何真值都排除）
    if model.get("supportsExtra").map(js_truthy).unwrap_or(false) {
        return false;
    }
    // `typeof model.maxOutputTokens !== 'number'` → 排除
    let Some(max_output) = model.get("maxOutputTokens").and_then(Value::as_f64) else {
        return false;
    };
    max_output >= 16_000.0
}

/// 目录的内部状态：模型清单 + 刷新元信息（对应 Node 版的闭包变量）
#[derive(Clone, Debug)]
struct CatalogState {
    models: Vec<Value>,
    /// 最后一次成功的远程刷新时间（毫秒），初始 0
    last_refreshed_at: i64,
    /// 是否曾成功采用远程清单（决定 list_response 的 meta.source）
    remote_refreshed: bool,
}

/// 目录准入过滤：对话模型判定 + 白名单（空白名单全量放行）
fn apply_filters(models: Vec<Value>) -> Vec<Value> {
    models
        .into_iter()
        .filter(is_chat_model)
        .filter(|model| {
            if MODEL_ALLOWLIST.is_empty() {
                return true;
            }
            let id = model.get("id").map(value_text).unwrap_or_default();
            MODEL_ALLOWLIST.contains(&id.as_str())
        })
        .collect()
}

/// 白名单模型必须有可用定义：远程/传入清单缺失时回退内置定义
fn allowlist_fallback() -> Vec<Value> {
    builtin_models()
        .into_iter()
        .filter(|model| {
            MODEL_ALLOWLIST.contains(&model.get("id").map(value_text).unwrap_or_default().as_str())
        })
        .filter(is_chat_model)
        .collect()
}

/// 初始目录：**优先上次成功拉到的远程清单**（持久化缓存，见 `catalog_cache`
/// 的模块头），没有再退回内置清单（经准入过滤）——与 Node 版
/// `createModelCatalog` 的初始化相比，这里多出来的只有缓存恢复这一层。
///
/// 缓存恢复的清单**同样算「远程」**（`remote_refreshed = true`）：它确实是
/// 上游下发的清单，只是可能旧一些。管理页的「来源」列与 `/v1/models` 的
/// `meta.source` 因此显示「远程」而不是「内置」——那是事实，且时间戳
/// （`last_refreshed_at`）会如实给出它是什么时候拉的。
///
/// 缓存槽与内置兜底按**地区**分（见模块头的拆家说明）：两个地区各读各的
/// scope，内置兜底两地共用同一份。
fn initial_state(region: Region) -> CatalogState {
    if let Some(cached) = catalog_cache::load(region.catalog_scope()) {
        return CatalogState {
            models: cached.models,
            last_refreshed_at: cached.fetched_at,
            remote_refreshed: true,
        };
    }
    let allowed = apply_filters(builtin_models());
    let models = if allowed.is_empty() { allowlist_fallback() } else { allowed };
    CatalogState { models, last_refreshed_at: 0, remote_refreshed: false }
}

/// 模型目录句柄：内部一把 `RwLock`，克隆共享同一份状态。
///
/// `region` 是**这一份清单属于谁**（拆家后一个地区一个实例）：
/// provider id 进 `owned_by`、缓存 scope 定位读写、版本决定 UA 与端点兜底 ——
/// 三处都从实例取，而不是像拆家前那样写死常量（那正是「两份清单挤在一个
/// 句柄里」的形态）。
#[derive(Clone)]
pub struct ModelCatalog {
    region: Region,
    inner: Arc<RwLock<CatalogState>>,
}

impl Default for ModelCatalog {
    fn default() -> Self {
        Self::new(Region::Cn)
    }
}

impl ModelCatalog {
    /// 构造目录（不刷新；刷新由启动流程与 /v1/models 触发）
    pub fn new(region: Region) -> Self {
        Self { region, inner: Arc::new(RwLock::new(initial_state(region))) }
    }

    /// 这一份清单属于哪个地区
    pub fn region(&self) -> Region {
        self.region
    }

    /// 读取状态快照；锁中毒（持锁 panic）时接管内部数据继续用，
    /// 与账号存储同一策略：宁可容忍一次中毒，也不要让网关永久不可用。
    fn read(&self) -> CatalogState {
        match self.inner.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn write(&self, next: CatalogState) {
        match self.inner.write() {
            Ok(mut guard) => *guard = next,
            Err(poisoned) => *poisoned.into_inner() = next,
        }
    }

    /// 按 id 或 name 查模型（大小写不敏感）；查不到返回 None。
    ///
    /// 顺序照抄 Node：先按 id 精确（忽略大小写），再按 name。
    pub fn get(&self, id: Option<&str>) -> Option<Value> {
        let id = id.map(str::trim).filter(|value| !value.is_empty())?;
        let lower = id.to_lowercase();
        let models = self.read().models;
        models
            .iter()
            .find(|model| value_text(&model["id"]).to_lowercase() == lower)
            .or_else(|| {
                models.iter().find(|model| {
                    model
                        .get("name")
                        .map(value_text)
                        .unwrap_or_default()
                        .to_lowercase()
                        == lower
                })
            })
            .cloned()
    }

    /// 目录里是否有该模型。
    ///
    /// **不再被对话链路调用**（Agent2API 改造 W2b-T3 起模型校验走
    /// `providers::catalog::providers_for_model`，视图是聚合目录）：这里是
    /// 「只看 workbuddy 这一家」的判定，排障时与聚合判定对比即可分辨
    /// 「是清单不对」还是「聚合/去重不对」。
    #[allow(dead_code)]
    pub fn has(&self, id: &str) -> bool {
        self.get(Some(id)).is_some()
    }

    /// 解析模型 id：只接受目录里真实存在的 id（大小写不敏感）。
    ///
    /// 未知名直接返回 None —— 绝不静默改写/回退：用户点名要 A 就必须路由到 A，
    /// 不存在就报错让下游自己改（路由层据此返回 400 与相近模型提示）。
    ///
    /// Node 版 `createModelCatalog` 导出的 `resolve(id)` 对等物（Node 里未命中会抛
    /// ModelRouteError）：本进程的请求校验走 `models().has()` + `suggest()`，
    /// 因此这条暂无调用点，保留以便与 Node 的目录 API 一一对应。
    #[allow(dead_code)]
    pub fn resolve(&self, id: &str) -> Option<String> {
        self.get(Some(id))
            .and_then(|model| model.get("id").map(value_text))
    }

    /// 仅供 400 报错信息用的「你是不是想要」提示：列出目录里与请求名最相近的 id。
    ///
    /// 结果不参与路由 —— 路由永远只认精确存在的模型。
    /// 判定逻辑在 `suggest_from`（聚合目录共用同一份，保证单一来源与聚合两种
    /// 视角下的提示口径一致）。
    ///
    /// **不再被对话链路调用**（Agent2API 改造 W2b-T3 起 400 提示走
    /// `providers::catalog::suggest_models`，数据源是聚合目录）：这里是
    /// 「只看 workbuddy 这一家」的视图，排障时与聚合提示对比即可分辨
    /// 「是清单不对」还是「聚合/去重不对」。对只有 workbuddy 一家的用户，
    /// 两者结果相同（共用 `suggest_from`）。
    #[allow(dead_code)]
    pub fn suggest(&self, id: &str, limit: usize) -> Vec<String> {
        let ids: Vec<String> = self
            .read()
            .models
            .iter()
            .filter_map(|model| model.get("id").map(value_text))
            .collect();
        suggest_from(ids, id, limit)
    }

    /// 当前目录（克隆；调用方不会看到后续刷新）
    pub fn list(&self) -> Vec<Value> {
        self.read().models
    }

    /// 是否曾成功采用远程清单（挂进响应 meta 用；聚合层据此算「来源」）
    pub fn remote_refreshed(&self) -> bool {
        self.read().remote_refreshed
    }

    /// 最后一次成功远程刷新的时间（毫秒，未刷新过为 0）
    pub fn last_refreshed_at(&self) -> i64 {
        self.read().last_refreshed_at
    }

    /// 目录条数（/health 的 models 字段）
    pub fn count(&self) -> usize {
        self.read().models.len()
    }

    /// GET /v1/models 的 OpenAI 风格响应：`{object, data: [...], meta: {...}}`
    ///
    /// 响应体本身的拼装走 `list_response_from`（聚合目录共用同一函数，
    /// 区别只在 data 是合并后的数组）—— 单家与聚合两个视角的信封字段
    /// 与顺序不可能分叉。
    ///
    /// **保留但不再被 /v1/models 调用**（Agent2API 改造 W2a-T2 起该端点走
    /// `providers::catalog::models_response`）：这里是「只看 workbuddy 这一家」
    /// 的视图，排障时与聚合输出对比即可立刻分辨「是 workbuddy 清单不对」
    /// 还是「聚合/过滤逻辑不对」。对只有 workbuddy 一家的用户，
    /// 它与聚合输出逐字段相同（聚合层复用同一个 list_item / 信封）。
    ///
    /// `owned_by` 取本实例的地区（拆家后 `workbuddy` / `workbuddy-intl`）：
    /// 写死国内版会让国际版的排障视图指错家。
    #[allow(dead_code)]
    pub fn list_response(&self) -> Value {
        let state = self.read();
        let data: Vec<Value> = state
            .models
            .iter()
            .map(|model| list_item(model, self.region.provider_id()))
            .collect();
        list_response_from(
            data,
            if state.remote_refreshed { "remote" } else { "builtin" },
            state.last_refreshed_at,
        )
    }

    /// 是否已采用远程清单（排障用）。
    /// Node 版 `createModelCatalog` 导出的 `get isRemote()` 对等物（Node 里同样
    /// 没有生产调用点）：`list_response` 的 `meta.source` 已表达同一事实，
    /// 这条保留给排障探针按布尔直读。
    #[allow(dead_code)]
    pub fn is_remote(&self) -> bool {
        self.read().remote_refreshed
    }

    /// 远程配置刷新（与桌面端行为一致）：
    ///
    ///   1) GET {endpoint}/v3/config  → data.models 为运行时清单
    ///   2) 企业账号再取 /console/enterprises/{id}/config/models
    ///
    /// 两者都不可用时保留内置目录。拉回的清单先过 MODEL_ALLOWLIST 白名单。
    /// 返回 `{refreshed, count, source}` 或 `{refreshed:false, reason}` 语义
    /// 与 Node 版逐字一致 —— 调用方据此决定打 ✅ 还是 verbose。
    pub async fn refresh(&self, params: RefreshParams<'_>) -> RefreshOutcome {
        let base = params.endpoint.trim_end_matches('/').to_string();
        let has_auth = params.headers.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("authorization") && !value.trim().is_empty()
        });

        if !base.is_empty() && has_auth {
            let url = format!("{base}/v3/config");
            // 与 Node 一致：显式 Accept + 官方 UA（服务端按 UA 识别客户端通道，
            // 缺失时会只下发旧版兼容清单）。Accept 由 send_raw 统一补
            // `application/json`（Node 这里给的也是同一个值），不必重复声明 ——
            // reqwest 的 header() 是 append，重复给会变成两个 Accept 头。
            let mut headers: Vec<(String, String)> =
                vec![("User-Agent".to_string(), params.user_agent.to_string())];
            headers.extend(params.headers.iter().cloned());
            match send_raw("GET", &url, None, &headers, params.proxy, None).await {
                // 不校验 HTTP 状态：与 Node 一致 —— 先按 JSON 解析，再看有没有可用模型
                Ok(response) => match response.payload {
                    Some(payload) => {
                        // 诊断落盘：把 `/v3/config` 的**原始响应**整份存一份（覆盖式，
                        // 只留最近一次）。用途见 `dump_v3_config` 的说明 —— 这里先
                        // 存，再解析，所以 `agents` 那类本模块不消费的字段也在里面。
                        dump_v3_config(&payload);
                        let raw = payload
                            .get("data")
                            .and_then(|data| data.get("models"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let allowed = apply_filters(raw.clone());
                        if !allowed.is_empty() {
                            // 官方 picker 名单（`agents[name=="cli"].models`）：
                            // 决定每条模型在「可见性」列里标什么。
                            let picker = picker_model_ids(&payload);
                            self.apply_remote_with_picker(
                                allowed,
                                "auto",
                                false,
                                picker.as_deref(),
                            );
                            return RefreshOutcome::refreshed(
                                self.count(),
                                "v3/config",
                            );
                        }
                        logging::verbose(
                            "[Models]",
                            &format!("远程清单({} 个)不含可用对话模型，保留内置目录", raw.len()),
                        );
                    }
                    None => {
                        logging::verbose("[Models]", "远程配置拉取失败: 响应不是合法 JSON");
                    }
                },
                Err(error) => {
                    logging::verbose(
                        "[Models]",
                        &format!(
                            "远程配置拉取失败: {}",
                            crate::server::core::egress::describe_error_detail(&error)
                        ),
                    );
                }
            }
        }

        if !base.is_empty() {
            if let Some(enterprise_id) = params.enterprise_id.filter(|id| !id.is_empty()) {
                let url = format!(
                    "{base}/console/enterprises/{}/config/models",
                    encode_uri_component(enterprise_id)
                );
                let headers: Vec<(String, String)> = params.headers.to_vec();
                return match send_raw("GET", &url, None, &headers, params.proxy, None).await {
                    Ok(response) => {
                        let raw = response
                            .payload
                            .as_ref()
                            .and_then(|payload| payload.get("data"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let allowed = apply_filters(raw);
                        if allowed.is_empty() {
                            RefreshOutcome::not_refreshed("企业模型列表为空")
                        } else {
                            self.apply_remote(allowed, "custom:", true);
                            RefreshOutcome::refreshed(self.count(), "enterprise")
                        }
                    }
                    Err(error) => RefreshOutcome::not_refreshed(
                        crate::server::core::egress::describe_error_detail(&error),
                    ),
                };
            }
        }

        RefreshOutcome::not_refreshed(REASON_NO_SESSION)
    }

    /// 落地一份远程清单（锁内只做替换，不做任何 IO；锁外再补一次默认规则
    /// 种子，见函数尾）
    ///
    /// 默认模型统一为 auto（远程可能把 fast-model 等档位模型标为默认，
    /// 与网关默认不一致）。`enterprise` 为 true 时走企业分支：
    /// modelType 按 `custom:` 前缀区分企业定制/内置模型
    /// （判定用**原始 id**，大小写不敏感的处理只用于 isDefault 那一处）。
    fn apply_remote(&self, models: Vec<Value>, custom_prefix: &str, enterprise: bool) {
        self.apply_remote_with_picker(models, custom_prefix, enterprise, None)
    }

    /// [`Self::apply_remote`] 的实现，多一个「官方 picker 名单」参数。
    ///
    /// ── `picker` 是什么（2026-10-17 补上，此前缺这一层）────────────
    /// `/v3/config` 的 `data.agents` 里有一组 `name == "cli"` 的 agent，它的
    /// `models` 是**裸字符串数组** —— 那正是官方客户端模型选择器展示的名单。
    /// 实测（2026-10-17，本机真实响应）：`data.models` 有 60 条，而
    /// `agents[cli].models` 只有 18 条，其余 42 条是补全 / 改写 / 内部 / 图像 /
    /// 视频 / 旧代条目，**官方 picker 不列**。
    ///
    /// 参考实现同样按这份名单收窄：`Acankao/workbuddy2api/internal/upstream/
    /// client.go:1387` 先取 `agents[name=="cli"].models` 再按它筛
    /// `data.models`，注释写「实测 CN v3 51 模型含大量非 cli 面模型，cli 过滤后
    /// 与 console 口径才有可比性」；`Acankao/workbuddy2api-hub/wb_proxy.py:10565`
    /// 的 `_agent_model_lists()` 同一手法（注释：「The catalogue the picker shows
    /// rides in agents[].models」）。
    ///
    /// `None` = 这次没拿到名单（企业分支 / 上游没下发 `agents`）。此时**不标
    /// 可见性**（条目不写 `visibility` 键）——「没依据就不下判断」，与
    /// Trae 的「缺省 = 可见」同一纪律。
    fn apply_remote_with_picker(
        &self,
        models: Vec<Value>,
        custom_prefix: &str,
        enterprise: bool,
        picker: Option<&[String]>,
    ) {
        let mapped: Vec<Value> = models
            .into_iter()
            .map(|model| {
                let raw_id = model.get("id").map(value_text).unwrap_or_default();
                let lower_id = raw_id.to_lowercase();
                let is_auto = lower_id == "auto";
                let mut object = match model {
                    Value::Object(map) => map,
                    other => return other,
                };
                object.insert("isDefault".to_string(), Value::Bool(is_auto));
                let model_type = if enterprise {
                    if raw_id.starts_with(custom_prefix) { "enterprise" } else { "built-in" }
                } else if is_auto {
                    "auto"
                } else {
                    "built-in"
                };
                object.insert("modelType".to_string(), Value::String(model_type.to_string()));
                // 可见性：在不在官方 picker 名单里。名单是大小写敏感的裸 id，
                // 但上游 id 一贯小写；这里仍做一次不区分大小写的比较，避免
                // 上游哪天改了大小写导致整份清单被判成不可见。
                if let Some(list) = picker {
                    let visible = list.iter().any(|id| id.eq_ignore_ascii_case(&raw_id));
                    object.insert(
                        "visibility".to_string(),
                        Value::String(if visible { "shown" } else { "hidden" }.to_string()),
                    );
                }
                Value::Object(object)
            })
            .collect();
        let ids: Vec<String> = mapped
            .iter()
            .filter_map(|model| model.get("id").map(value_text))
            .collect();
        let state = CatalogState {
            models: mapped,
            last_refreshed_at: logging::now_ms(),
            remote_refreshed: true,
        };
        self.write(state.clone());
        // 落持久化缓存（进程重启后由 `initial_state` 读回，见 `catalog_cache`
        // 的模块头）。必须在写锁**之外**：缓存写入要拿库连接锁，而本目录的
        // 硬约束是「持锁期间不做 IO」。槽位按地区取 —— 两个地区的清单互不
        // 覆盖（这正是拆家要修的那件事，见模块头）。
        catalog_cache::save(
            self.region.catalog_scope(),
            &state.models,
            state.last_refreshed_at,
        );
        // 远程清单首次落地时补一次 WorkBuddy 默认规则种子（默认只启用白名单内的
        // 模型，见 `model_rules::seed_workbuddy_defaults`）。必须放在写锁之外：种子
        // 要写 config.json，而「持锁期间不做任何 IO」是本目录的硬约束。
        // provider 键取本实例的地区：种子按 `(provider, id)` 记账，国际版的
        // 模型名与国内版可能同名，记错家会让两家的开关互相污染。
        if let Some(summary) =
            crate::server::core::model_rules::seed_workbuddy_defaults(
                self.region.provider_id(),
                &ids,
            )
        {
            logging::log("[Models]", &summary);
        }
    }

    /// 按当前账号的凭证/端点/UA/出口刷新一次（路由层与启动流程的入口）。
    ///
    /// ── 「当前账号」是**本地区**的队首（拆家后的口径）──────────────
    /// 本实例只承载一个地区，因此这里的「当前账号」必须收窄到本家的 provider
    /// （`current_entry_for_provider`）—— 两个地区的账号混在一个队列里取队首，
    /// 会让国内版的清单用国际版账号去拉（拆家前正是这个形态，症状是「拉一次
    /// 换一个地区」，见 issue #74）。
    ///
    /// `account_id` 非空 = 用户在「获取模型」弹窗里点名的那条：按 id 直取，
    /// **取不到就明确失败、不回落到队首**（否则清单会变成「选了 A、用的是 B」）。
    ///
    /// 返回 [`RefreshOutcome`]（本函数内部本来就算出了它，只是过去只用于打日志）：
    /// 自动路径不看返回值（照旧只写日志），**手动刷新路径**靠它如实汇报
    /// 「刷到了几个 / 为什么没刷」—— 见 `providers::adapter` 的 `refresh_models`
    /// 契约。取不到可用登录态、`/v3/config` 失败或返回的清单不含对话模型，
    /// 都走 `not_refreshed(原因)`，既有行为（保留现有清单 + verbose 日志）不变。
    pub async fn refresh_with_current_account(
        &self,
        store: &crate::server::core::account_store::AccountStore,
        auth: &crate::server::core::auth::AuthService,
        account_id: &str,
    ) -> RefreshOutcome {
        // 账号选取三档：
        //   · 点名了（`account_id` 非空）→ 按 id 直取那一条，取不到即失败（见上）；
        //   · 没点名 → 本地区的队首账号优先（多账号时与转发一致）；
        //   · 本地区一条账号都没有 → 回落到本地区的默认登录态（环境变量
        //     `WORKBUDDY[_INTL]_TOKEN` 或单账号 auth.json）。
        let requested = account_id.trim();
        let (session, proxy) = if requested.is_empty() {
            match store.current_entry_for_provider(self.region.provider_id()) {
                Some(entry) => {
                    let proxy = crate::server::core::proxies::session_proxy(&entry.session);
                    (Some(entry.session), proxy)
                }
                None => match auth.get_current_session().await {
                    Ok(session) => (session, None),
                    Err(error) => {
                        logging::verbose("[Models]", &format!("模型目录刷新失败: {}", error.message));
                        return RefreshOutcome::not_refreshed(error.message);
                    }
                },
            }
        } else {
            match store.session_for_account(requested) {
                Some(entry) => {
                    let proxy = crate::server::core::proxies::session_proxy(&entry.session);
                    (Some(entry.session), proxy)
                }
                None => {
                    logging::verbose(
                        "[Models]",
                        &format!("模型目录刷新失败：指定的账号不存在或不可用（{requested}）"),
                    );
                    return RefreshOutcome::not_refreshed("指定的账号不存在或不可用，请重新选择");
                }
            }
        };
        let Some(session) = session else {
            logging::verbose("[Models]", &format!("模型目录未更新: {REASON_NO_SESSION}"));
            return RefreshOutcome::not_refreshed(REASON_NO_SESSION);
        };

        // 端点兜底按**本地区**取（账号记录里的 endpoint 仍是最高优先的覆盖项：
        // 它带着账号自己的站点，也正是 staging / 反向代理用户的落点）。
        let fallback_endpoint = self
            .region
            .env_endpoint_override()
            .unwrap_or_else(|| self.region.default_endpoint().to_string());
        let endpoint = session
            .get("endpoint")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(normalize_endpoint)
            .unwrap_or_else(|| normalize_endpoint(fallback_endpoint.as_str()));
        let enterprise_id = session
            .get("account")
            .and_then(|account| account.get("enterpriseId"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let headers = crate::server::core::auth::AuthService::build_auth_headers(&session);
        // UA 按本地区的客户端身份（国际版是 `WorkBuddy AI/5.5.2`，国内版
        // `WorkBuddy/5.5.4`；服务端按 UA 识别通道，给错会拿到另一套清单）
        let user_agent = user_agent_for_edition(Some(self.region.id()));

        let outcome = self
            .refresh(RefreshParams {
                endpoint: &endpoint,
                enterprise_id: Some(&enterprise_id),
                headers: &headers,
                user_agent: &user_agent,
                proxy: proxy.as_ref(),
            })
            .await;
        if outcome.refreshed {
            logging::log(
                "[Models]",
                &format!("✅ 模型目录已更新（{} 个，来源 {}）", outcome.count, outcome.source),
            );
        } else {
            logging::verbose("[Models]", &format!("模型目录未更新: {}", outcome.reason));
        }
        outcome
    }
}

/// 刷新入参（对应 Node 版 refresh 的 options 对象）
pub struct RefreshParams<'a> {
    pub endpoint: &'a str,
    pub enterprise_id: Option<&'a str>,
    pub headers: &'a [(String, String)],
    pub user_agent: &'a str,
    pub proxy: Option<&'a ResolvedProxy>,
}

/// 刷新结果：成功 `{refreshed:true, count, source}`，
/// 失败 `{refreshed:false, reason}`（与 Node 版返回值同形）。
#[derive(Clone, Debug)]
pub struct RefreshOutcome {
    pub refreshed: bool,
    pub count: usize,
    pub source: String,
    pub reason: String,
}

impl RefreshOutcome {
    pub fn refreshed(count: usize, source: &str) -> Self {
        Self {
            refreshed: true,
            count,
            source: source.to_string(),
            reason: String::new(),
        }
    }

    pub fn not_refreshed(reason: impl Into<String>) -> Self {
        Self {
            refreshed: false,
            count: 0,
            source: String::new(),
            reason: reason.into(),
        }
    }
}

/// 对照 JS 的 `encodeURIComponent`（企业 id 进 URL 路径前编码）
fn encode_uri_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')')
        {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 供 /api/endpoints 与排障展示：内置清单（不含运行时刷新结果）。
/// Node 版 workbuddy-models.mjs 同名导出 `builtinModels()` 的对等物
/// （Node 侧注释「供 CLI/调试」，本进程没有 CLI，故只作排障口保留）。
#[allow(dead_code)]
pub fn builtin_models_snapshot() -> Value {
    Value::Array(builtin_models())
}

/// 从 `/v3/config` 响应里取**官方 picker 名单**（`agents[name=="cli"].models`）。
///
/// ── 形态（2026-10-17 实测，本机真实响应）──────────────────────
/// ```json
/// { "data": { "agents": [ { "name": "cli",
///                           "models": ["auto", "hy4-preview", "hy3", …] } ] } }
/// ```
/// `models` 是**裸字符串 id 数组**（不是对象数组），共 18 条；`data.models`
/// 则是 60 条带完整元数据的对象数组。两边的 id 是同一套命名，可直接比对。
///
/// ── 兼容形态 ────────────────────────────────────────────────
/// `workbuddy2api-hub` 的 `_agent_model_lists()` 同时接受 `agents` 是**对象**
/// （`{name: {...}}`）的形态 —— 桌面端缓存文件写盘时是那个样子。这里同样兼容，
/// 免得上游一改形态就整份判成「没有名单」。
///
/// 返回 `None` 的情形（调用方据此**不标可见性**）：
///   - 响应里没有 `agents`（企业分支 / 上游没下发）；
///   - 有 `agents` 但没有 `cli` 那一组；
///   - `models` 不是非空的字符串数组。
fn picker_model_ids(payload: &Value) -> Option<Vec<String>> {
    let agents = payload.get("data")?.get("agents")?;
    let collect = |name: &str, entry: &Value| -> Option<Vec<String>> {
        if !name.trim().eq_ignore_ascii_case("cli") {
            return None;
        }
        let models = entry.get("models")?.as_array()?;
        let ids: Vec<String> = models
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect();
        if ids.is_empty() { None } else { Some(ids) }
    };
    if let Some(rows) = agents.as_array() {
        for row in rows {
            let name = row.get("name").and_then(Value::as_str).unwrap_or("");
            if let Some(ids) = collect(name, row) {
                return Some(ids);
            }
        }
        return None;
    }
    if let Some(map) = agents.as_object() {
        for (name, row) in map {
            if let Some(ids) = collect(name, row) {
                return Some(ids);
            }
        }
    }
    None
}

/// 把 `/v3/config` 的**原始响应**覆盖式落盘一份，供人工核对上游结构。
///
/// ── 为什么留着它（不是临时调试代码）────────────────────────────
/// 本模块只消费 `data.models` 与 `data.agents[cli].models` 两处，其余字段
/// （`modelTiers` / `modelPromotions` / `featureToggles` …）随时可能藏着下一批
/// 「该收窄什么」的依据。没有一份原始响应，任何一次判据调整都得重新抓包 ——
/// 而抓包要真账号、真网络、真上游。
///
/// ── 落点与脱敏 ──────────────────────────────────────────────
/// `{config_dir}/debug/v3-config.json`（覆盖式，只留最近一次；`debug/` 已经是
/// 排障产物的目录，见 `debug_traffic` 模块头）。**不做脱敏**：这份响应里没有
/// 凭据（模型目录是公开信息），脱敏反而会把结构改坏、失去核对价值 —— 凭据在
/// 请求头里，从来不在响应体里。
///
/// ── 失败一律静默 ────────────────────────────────────────────
/// 这是排障辅助：目录创建失败、写盘失败都不该影响刷新本身（与
/// `catalog_cache::save` 同一取舍 —— 缓存写不进去不能让刷新失败）。
fn dump_v3_config(payload: &Value) {
    let dir = crate::paths::config_dir().join("debug");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let text = match serde_json::to_string_pretty(payload) {
        Ok(text) => text,
        Err(_) => return,
    };
    let _ = std::fs::write(dir.join("v3-config.json"), text);
}

// ─── 进程级目录句柄（聚合目录的读取口）──────────────────────

/// 进程级 workbuddy 目录句柄，**按地区各一个**。**为什么要全局**：聚合目录
/// （`core::providers::catalog`）的公开 API 只收 `&AccountStore`（handler 手边
/// 就有），不该再要求调用方层层传一个 ModelCatalog —— 那会把 `list_models` 与
/// 将来 chat_completions 的签名都改一遍。全局句柄与 `config::current()` /
/// `config::current()` 是同一模式：启动时装一次，之后所有模块共用。
///
/// 取用方式是 `get_or_init`：`ServerState::bootstrap` 与聚合层拿到的是**同一个
/// 实例**（共享同一把 RwLock），即使 bootstrap 被调用多次（重启路径）也不会
/// 出现「ServerState 里刷新了、聚合层读的是另一份旧目录」这种分叉。
static WORKBUDDY_CATALOG: OnceLock<ModelCatalog> = OnceLock::new();
/// 国际版的目录句柄（拆家后与国内版各自独立，见模块头）
static WORKBUDDY_INTL_CATALOG: OnceLock<ModelCatalog> = OnceLock::new();

/// 取某个地区的进程级目录句柄；首次调用即构造（内置清单，不读盘、不刷新）。
///
/// 构造顺序无关：任何模块在启动流程的任何阶段调它都能拿到一个可用的目录
/// （最坏情况是「还没被 `/v3/config` 刷新过」的内置清单，与改造前的初始态一致）。
pub fn global_catalog(region: Region) -> ModelCatalog {
    match region {
        Region::Cn => WORKBUDDY_CATALOG.get_or_init(|| ModelCatalog::new(region)).clone(),
        Region::Intl => WORKBUDDY_INTL_CATALOG.get_or_init(|| ModelCatalog::new(region)).clone(),
    }
}
