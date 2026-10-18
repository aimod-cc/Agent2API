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
//! ── 接哪些模型：三个家族（2026-10 放开 Claude/GPT）────────────
//! 同一个端点除 Gemini 外还下发 `claude-sonnet-4-6` / `claude-opus-4-6-*` /
//! `gpt-oss-120b-medium`（规格 §5.3）。本家曾经**只接 Gemini**（按 `gemini-`
//! 前缀白名单过滤），issue #288 之后放开到三个家族：
//!   - `gemini-*`、`claude-*`、`gpt-oss-*` 三种前缀的条目进广告清单与缓存；
//!   - 其余的一律不收 —— 「前缀白名单」而不是「黑名单排除已知的」，上游将来
//!     再下发别的家族（veo 之类）时默认仍是「不接」，而不是误广告一个没实现的通道。
//! 上游对**所有账号**（免费号与 Pro 号）下发的是同一份全量目录（Manager
//! `models/quota.rs` 的注释：两档拿到的 33 个模型完全相同），所以「账号里没有
//! Claude」不是档位问题，而是本仓此前自己把它过滤掉了 —— 放开后能否真正调用
//! 由上游按订阅档判定，不归本仓预判。
//!
//! Claude 的两条额外要求（规格 §5.3 与坑清单，都在别处落地）：
//!   1. 请求要带 `anthropic-beta` 头（见 `endpoints::content_headers` 的
//!      `claude` 分支）；
//!   2. 思考预算用 Claude 家族的默认档（1024，Manager `SPEC_CLAUDE_*`），
//!      不是 Gemini 的 32768（见 [`fallback_thinking_budget`]）。
//!
//! ── 兜底清单的 id 与「发给上游的名字」是两回事（映射表）──────
//! 规格 §5.2 的 id 是**本家对外的名字**：`gemini-3.6-flash` 发出去时上游要的是
//! `gemini-3.6-flash-tiered`，`gemini-3.1-pro-high` 对应上游真名
//! `gemini-pro-agent`（`gemini-3-pro-high` 同理）。映射表见
//! [`upstream_model_id`]（出处：Manager `CLAUDE_TO_GEMINI` 的 Gemini 部分 +
//! `is_bare_gemini_v36_or_above_flash` 的 tiered 规则；9router registry 的 id
//! 集合交叉验证）。**对不上的 id 原样透传** —— 远程
//! `fetchAvailableModels` 下发的 id 本身就是上游真名（规格 §5.2 末句 /
//! §8.11），不该被这张表改写。
//! Claude 的**旧 id 重定向**也在同一张表里（`claude-opus-4-5-thinking` →
//! `claude-opus-4-6-thinking` 这类，Manager `CLAUDE_TO_GEMINI` 逐条）：上游把
//! 上一代 Claude 统一重定向到 4.6，重定向规则因此要跟着走。**注意本仓与
//! Manager 的结构差异**：单一上游的 Manager 会把旧名原样收进来再改写，而本仓
//! 的模型门是「广告视图里没有就 404」，旧代 id 不在广告里 —— 那几条规则是
//! 清单漂移时的兜底，不是「写旧名就能用」的承诺（详见 `UPSTREAM_ALIASES` 的
//! 文档）。
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
/// Manager `CLAUDE_TO_GEMINI` 逐条）。
///
/// ```text
///   ── Gemini（规格 §5.2）────────────────────────────────────
///   gemini-3.6/3.7/3.8-flash  → <同 id>-tiered    （无后缀 Flash 走自适应档）
///   gemini-3.1-pro-high       → gemini-pro-agent  （Manager 表逐字）
///   gemini-3-pro-high         → gemini-pro-agent  （同上）
///   gemini-2.5-flash-lite     → gemini-2.5-flash  （Manager 表逐字；9router
///                               registry 不含这条 id，属两个参考的差异）
///   ── Claude 旧代 → 4.6 的重定向（Manager 表逐条）───────────
///   上游把上一代 Claude 统一重定向到 4.6（规格 §5.3 的括注：
///   `claude-opus-4-5*` / `claude-3-5-sonnet*` 被重定向到 4.6）。这张表照抄
///   Manager 的 `CLAUDE_TO_GEMINI` 那一组，作用与 Gemini 那几条**完全相同**：
///   **清单里出现过的 id** 在发送前改写成上游真名。
///   ⚠️ 别把这句读成「客户端写旧名字就能用」—— 本仓与 Manager 的单上游形态
///   有一处结构差异，值得写清楚：**网关的模型门是「广告视图里没有就 404」**
///   （`api::pipeline::resolve_model` 的 ③），而广告视图 = 本家清单（远程目录
///   过滤三家 + 兜底清单 + 用户登记的自定义模型）。远程目录下发的 id 就是上游
///   真名、且只有 4.6 一代，所以**点名旧代 id 会被 404 挡下**，走不到这张表。
///   它们的实际用途是**让旧名字在进了清单之后仍能发对**：
///     · 用户在模型管理里把某个旧代 id 登记成自定义模型（那是为「上游灰度、
///       目录没广告但能路由」准备的入口）时，改写规则已经在位；
///     · 远程目录哪天又下发回旧 id 时同理。
///   也就是说这几条是**改写规则**而不是**广告承诺** —— 后者只有兜底清单里
///   那三条新代 Claude。
///   **表里有映射的那几条 id 因此不进兜底清单**：旧名属于兼容项，不是本家主动
///   广告的模型。
/// ```
///
/// 表里没有的一律原样透传（见模块头：远程目录下发的 id 就是上游真名）。
const UPSTREAM_ALIASES: [(&str, &str); 21] = [
    ("gemini-3.6-flash", "gemini-3.6-flash-tiered"),
    ("gemini-3.7-flash", "gemini-3.7-flash-tiered"),
    ("gemini-3.8-flash", "gemini-3.8-flash-tiered"),
    ("gemini-3.1-pro-high", "gemini-pro-agent"),
    ("gemini-3-pro-high", "gemini-pro-agent"),
    ("gemini-2.5-flash-lite", "gemini-2.5-flash"),
    // Claude 4.5 / 4 系 → 4.6（`-thinking` 变体保留思考属性）
    ("claude-sonnet-4-5", "claude-sonnet-4-6"),
    ("claude-sonnet-4-5-thinking", "claude-sonnet-4-6-thinking"),
    ("claude-sonnet-4-5-20250929", "claude-sonnet-4-6-thinking"),
    ("claude-opus-4", "claude-opus-4-6-thinking"),
    ("claude-opus-4-5-thinking", "claude-opus-4-6-thinking"),
    ("claude-opus-4-5-20251101", "claude-opus-4-6-thinking"),
    // Claude 4.6 的**点分写法与裸名**：`claude-opus-4.6(-thinking)` 是客户端
    // 常见的另一种写法；裸 `claude-opus-4-6` 上游同样要 `-thinking` 那个真名
    ("claude-opus-4-6", "claude-opus-4-6-thinking"),
    ("claude-opus-4.6", "claude-opus-4-6-thinking"),
    ("claude-opus-4.6-thinking", "claude-opus-4-6-thinking"),
    ("claude-opus-4-6-20260201", "claude-opus-4-6-thinking"),
    // Claude 3.5 / Haiku 一代 → 最近的可用款
    ("claude-3-5-sonnet-20241022", "claude-sonnet-4-6"),
    ("claude-3-5-sonnet-20240620", "claude-sonnet-4-6"),
    ("claude-haiku-4", "claude-sonnet-4-6"),
    ("claude-haiku-4-5-20251001", "claude-sonnet-4-6"),
    ("claude-3-haiku-20240307", "claude-sonnet-4-6"),
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

/// 内置兜底清单（规格 §5.2 的 Gemini id **逐字** + §5.3 的 Claude / GPT-OSS）。
///
/// 键是**对外的模型 id**（转发时要映射成上游真名，见模块头）；
/// 值是展示名（上游 `displayName` 拉不到时的界面文案，不影响转发）。
///
/// ── Claude / GPT-OSS 这几条为什么也要在这里 ──────────────────
/// 兜底清单是「远程目录一次都没拉到」时的唯一文案来源（首次启动、离线、
/// 上游 4xx）。若只列 Gemini，刚装好还没刷过目录的用户在模型管理里就看不到
/// Claude —— 而他对这个功能的期待恰恰是「加了账号就该有 Claude」
/// （issue #288）。id 取自规格 §5.3 与 Manager 的 `CLAUDE_TO_GEMINI` 键集：
/// `claude-sonnet-4-6(-thinking)` / `claude-opus-4-6-thinking` /
/// `gpt-oss-120b-medium` 都是**上游真名**（三家走同一 v1internal 端点，
/// model 直接填这些 id，不经映射表改写）。
/// **旧代 Claude 不进兜底清单**（`claude-opus-4-5*` / `claude-3-5-sonnet*`）：
/// 它们在映射表里被重定向到 4.6，属于「客户端配置里的历史名字仍要能用」的
/// 兼容项，不是本家主动广告的模型。
const FALLBACK_MODELS: [(&str, &str); 29] = [
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
    ("claude-sonnet-4-6", "Claude Sonnet 4.6"),
    ("claude-sonnet-4-6-thinking", "Claude Sonnet 4.6 Thinking"),
    ("claude-opus-4-6-thinking", "Claude Opus 4.6 Thinking"),
    ("gpt-oss-120b-medium", "GPT-OSS 120B Medium"),
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

/// 这个 id 是不是本家对外提供的模型：**三个家族**（Gemini / Claude / GPT-OSS）。
///
/// 判据是**前缀白名单**（不是「排除已知的黑名单」）：上游将来再下发别的家族
/// （veo 之类）时，默认仍是「不接」，而不是误广告一个没实现的通道。
/// 三个前缀的出处见模块头：Gemini 是原生家族，Claude 与 GPT-OSS 是同端点下发的
/// 第三方模型（规格 §5.3）。
pub fn is_offered_model(id: &str) -> bool {
    let lowered = id.trim().to_ascii_lowercase();
    lowered.starts_with("gemini-") || lowered.starts_with("claude-") || lowered.starts_with("gpt-oss-")
}

/// 这个 id 是不是 Claude 家族（判据取**上游真名**，见 `adapter::build_chat_request`
/// 的调用点）。
///
/// 唯一消费方是「要不要带 `anthropic-beta`」（规格 §5.3：那个头仅 Claude）。
/// 与 [`is_offered_model`] 同一前缀判法 —— 家族的认定在对外 id 与上游真名上
/// 逐字相同（映射表只改中间的版本段，不改前缀）。
pub fn is_claude_model(id: &str) -> bool {
    id.trim().to_ascii_lowercase().starts_with("claude-")
}

/// 该 id 是否支持思考（兜底清单的判据；远程清单用上游自己的 `supportsThinking`）。
///
/// - **Gemini**：3 系全体 + 名字里带 `thinking` 的（2.5 那一支）给 true；
///   其余（2.0、2.5 普通档）保守 false。
/// - **Claude**：本家在 Antigravity 下提供的 Claude 全部支持 extended thinking
///   （`claude-sonnet-4-6` 与 `-thinking` 变体、`claude-opus-4-6-thinking`；
///   规格 §5.3 与 Manager 的 `SPEC_CLAUDE_*` 都给 1024 起的思考预算）。带
///   `-thinking` 后缀的更是从名字就能看出来。
/// - **GPT-OSS**：`gpt-oss-120b-medium` 是推理模型（Manager
///   `SPEC_GPT_OSS_120B` 给 8192 预算），故 true。
fn fallback_thinking_supported(id: &str) -> bool {
    let lowered = id.to_ascii_lowercase();
    if lowered.starts_with("claude-") || lowered.starts_with("gpt-oss-") {
        return true;
    }
    lowered.starts_with("gemini-3") || lowered.contains("pro-agent") || lowered.contains("thinking")
}

/// 兜底清单里的「是否推荐为默认」：规格 §5.2 把 `gemini-3-flash` 标为推荐入口
fn fallback_is_default(id: &str) -> bool {
    id == "gemini-3-flash"
}

/// 目录条目**没给** `thinkingBudget` 时的家族默认预算（0 = 不注入
/// `thinkingConfig`，见 `protocol::antigravity_outbound` 的 `thinking_budget`）。
///
/// 三档各有出处，**不要统一成一个数**：
///   · **Gemini**：`None` → 调用方回落到 [`DEFAULT_THINKING_BUDGET`]（32768，
///     规格 §3.3 官方客户端样例值）；
///   · **Claude**：`Some(1024)`。Manager `SPEC_CLAUDE_SONNET_46` /
///     `SPEC_CLAUDE_OPUS_46` 都写 1024，注释说明这是 Anthropic SDK 的规范最小值
///     —— **不能沿用 Gemini 的 32768**，Claude 的思考预算与输出预算是同一池子，
///     给大了会把回答挤没（实测口径见 Manager 的 `infer_tier`：`1024` 是
///     Anthropic 默认档，不是「最低档」）；
///   · **GPT-OSS**：`Some(8192)`（Manager `SPEC_GPT_OSS_120B` 逐字）。
pub fn fallback_thinking_budget(id: &str) -> Option<i64> {
    let lowered = id.trim().to_ascii_lowercase();
    if lowered.starts_with("claude-") {
        return Some(1_024);
    }
    if lowered.starts_with("gpt-oss-") {
        return Some(8_192);
    }
    None
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
    if !is_offered_model(id) {
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
    } else if let Some(budget) = fallback_thinking_budget(id) {
        // 上游**没给** budget 时的家族默认档（Claude 1024 / GPT-OSS 8192，
        // 出处见 [`fallback_thinking_budget`]）。写进条目而不是留在发送侧判断：
        // `protocol::antigravity_outbound` 的取值链本来就是「条目 → 家族默认」，
        // 这里补上之后那条链对三家是同一条，发送侧不必认识家族名。
        // Gemini 不走这一支（返回 None）—— 它的兜底 32768 在发送侧，
        // 且那个值对 Claude 是错的。
        item.insert("thinkingBudget".to_string(), Value::from(budget));
    }
    // 配额（规格 §5 / §5.4）：原样搬运 `remainingFraction` 与 `resetTime`
    if let Some(quota) = info.get("quotaInfo") {
        if quota.is_object() {
            item.insert("quota".to_string(), quota.clone());
        }
    }
    // 可见性：上游目录是**超集**，官方客户端只展示其中一部分。
    // 上游自报的内部模型（`isInternal`）优先 —— 那个标记比世代判据更直接。
    let visibility = if is_internal_entry(info) {
        "hidden"
    } else {
        visibility_of(id)
    };
    item.insert(
        "visibility".to_string(),
        Value::String(visibility.to_string()),
    );
    Some(Value::Object(item))
}

/// 上游 `isInternal` 显式标记的内部模型直接算不可见。
///
/// 上游目录里有这个字段（9router / 10router 的解析器都用它过滤，
/// `Acankao/9router/src/app/api/providers/[id]/models/route.js:44`），但
/// 它**不覆盖** 2.5 / 2.0 那批 —— 那批带的是正常的 `quotaInfo` 与
/// `displayName`，只是官方选择器不列。所以两个判据要并用。
fn is_internal_entry(info: &Value) -> bool {
    info.get("isInternal").and_then(Value::as_bool).unwrap_or(false)
}

/// 该 id 在**官方客户端里是否可见**（`"shown"` / `"hidden"` / `"undeclared"`）。
///
/// ── 判据为什么是「世代」而不是「上游说了什么」──────────────────
/// 官方 Antigravity 文档的模型选择器只列 9 条（Gemini 3.8 / 3.7 / 3.6 Flash、
/// Gemini 3.1 Pro、Claude Sonnet 5.5 / Opus 5.5 / Sonnet 4.6 / Opus 4.6、
/// GPT-OSS-120b；<https://antigravity.google/docs/models>）。而
/// `fetchAvailableModels` 下发的是**全量静态目录** —— Manager 实测免费号与
/// Pro 号都拿到**同样的 33 个**模型（`Acankao/Antigravity-Manager/
/// src-tauri/src/models/quota.rs:203`），9router / 10router 的「重要模型清单」
/// 同样不含 2.5 / 2.0 系列（`open-sse/services/usage/google.js:171-186`）。
/// 2.5 那一支在本家是**后台任务**的降级模型（Manager 的标题生成 / 摘要用它），
/// 不是给用户选的。
///
/// 因此：`gemini-2.x` 与 `gemini-2.0-flash-exp` 一律 `hidden`；其余
/// `gemini-3*` / `claude-*` / `gpt-oss-*` 都是 `shown`。**只按世代判**，
/// 不按具体 id 列白名单 —— 上游加了新的 3.x 时不用改这里。
///
/// `undeclared` 留给「上游条目自己说不清」的情形（当前没用上；语义与
/// Trae 的「缺省 = 可见」同一纪律：没有依据就不下判断）。
fn visibility_of(id: &str) -> &'static str {
    let lowered = id.trim().to_ascii_lowercase();
    // `gemini-2.5-*` / `gemini-2.0-*`：官方选择器不列（见上面论证）
    if lowered.starts_with("gemini-2.") {
        return "hidden";
    }
    if lowered.starts_with("gemini-") || lowered.starts_with("claude-") || lowered.starts_with("gpt-oss-") {
        return "shown";
    }
    "undeclared"
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

/// 一次 HTTP 结果（目录请求只需要状态 + 解析后的 body）。
///
/// `pub(super)`：`balance.rs` 的额度摘要复用同一条请求口径 —— 同一台主机
/// （v1internal 的同一组基址）、同一组头、同一超时；两处各写一份必然分叉。
pub(super) struct HttpOutcome {
    pub(super) status: u16,
    pub(super) payload: Value,
}

/// 发一次 JSON POST（头集合与超时统一在这里）。
///
/// `pub(super)`：`balance.rs` 的额度摘要直接复用它 —— 同一台主机、同一组头、
/// 同一超时，两处各写一份必然分叉（该文件不再自建请求口径）。
pub(super) async fn send_json(
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
