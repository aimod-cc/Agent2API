//! 千问办公的模型目录：`GET {gateway}/api/desktop/chat-modes`。
//!
//! ── 上游返回什么（实测 2026-10-19，普通 Bearer JWT 即可，无需签名）──
//! ```json
//! {
//!   "schema_version": 2,
//!   "catalog": [
//!     { "key": "flash", "i18n": {"display_name": {"zh": "标准", "en": "Standard"}},
//!       "is_default": true, "price_factor": 0.1, "show_zero_price_factor": true,
//!       "enable": true, "format": "openai/chat-completions", "source": "system",
//!       "is_reasoning": true, "is_vl": true, "max_input_tokens": 1000000,
//!       "harness": "oc",
//!       "context_config": {"1M": {"token_count": 1000000, "is_default": true},
//!                          "400K": {"token_count": 400000}, "200K": {"token_count": 200000}} },
//!     { "key": "pro", ...  "price_factor": 1,   "is_vl": false, ... },
//!     { "key": "qwen3.8-max-preview", ... "price_factor": 1.8, "is_vl": true, ... }
//!   ],
//!   "subtask_routing": { "auto_mode_classifier": {"key": "lite", ...}, "ocr": {"key":"omni",...}, ... }
//! }
//! ```
//!
//! ── `catalog[].key` 就是模型 id（不要「规范化」它）───────────────
//! `flash` / `pro` / `qwen3.8-max-preview` 是**上游原样接受的模型名**。
//! 本项目其它家常见「人读名 ↔ 线上名」两套（Qoder 的 `qwen3.8-max` 之类），
//! 但本家不需要映射：把 `key` 原样发给上游就能用（已实测三个全 200）。
//!
//! ── `subtask_routing` 里的 `lite` / `omni` 为什么不出现在清单里 ────
//! 那是**客户端内部子任务**（标题生成、OCR、意图分类）用的路由表，
//! 它们的 `key`（`lite` / `omni`）指向的是客户端内置的引擎模型，
//! **不是**可以对外推理的模型 —— 上游 `/v1/chat/completions` 未必认这两个名字。
//! 因此这里只收 `catalog`，不碰 `subtask_routing`。
//!
//! ── 分发形态（与服务契约对齐）────────────────────────────────
//! `ProviderAdapter::list_models` 要求返回**上游原始形态**的记录数组
//! （聚合层用 `models::list_item` 做字段映射）。这里已经把 `catalog[]`
//! 转成统一形状，因为上游的字段名（`key` / `i18n.display_name`）与其它家
//! 差异太大，让通用层去认这些名字等于把 provider 知识漏进公共代码。
//!
//! ── ★ 统一形状用的是**聚合层键名**，不是上游键名 ────────────────
//! `models::shape::list_item` 只认 `maxInputTokens` / `supportsImages` /
//! `supportsReasoning` / `supportsToolCall` / `maxOutputTokens` 这五个键
//! （见那份的逐字段复刻）。本家若在条目里写 `contextWindow` / `vision` /
//! `reasoning`，聚合层一个都读不到 —— 表现是 `/v1/models` 里本家的条目
//! 丢掉 1M 上下文、`supports_images` 与 `supports_reasoning` 恒 false，
//! 而下游客端正是按这几个位决定截断长度、发不发图片、带不带思考参数。
//! 这与 `raccoon/models.rs` 明写的归一规则同一条（它把上游的
//! `contextWindow` 翻成 `maxInputTokens`，理由就是「list_item 认后者」）：
//! **归一在各家目录里做完再交出去，公共层不代做**。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use std::sync::{OnceLock, RwLock};

use serde_json::{json, Value};

use crate::server::errors::GatewayError;
use crate::server::logging;

use super::auth;
use super::endpoints;
use crate::server::core::providers::catalog_cache;

/// 本家在 `catalog_cache` 里的 scope 名。
pub const SCOPE: &str = catalog_cache::SCOPE_QWENWORK;

/// 进程内缓存的模型清单（远程拉成功后写入，供 `list_models` 同步读取）。
///
/// ── 为什么要有这份缓存 ─────────────────────────────────────
/// `ProviderAdapter::list_models` 是**同步**方法（签名如此，见 adapter.rs），
/// 而拉目录是异步 HTTP。所以「拉」与「读」必须分开：后台/手动刷新写缓存，
/// `list_models` 只读缓存；缓存为空时回落到静态兜底（[`fallback_manifest`]）。
static MANIFEST: OnceLock<RwLock<Vec<Value>>> = OnceLock::new();

/// 最近一次**远程拉取成功**的时刻（毫秒时间戳，0 表示从未拉过）。
///
/// 与 `MANIFEST` 分开存而不是「清单非空就是拉过」：兜底清单也可能被写进
/// 缓存（缓存由 `manifest_store` 首次初始化时读回），用清单长度当判据会把「内置兜底」误报成
/// 「远程来源」，界面的「来源 / 更新日期」两列就都不诚实了。
static LAST_REFRESHED: OnceLock<std::sync::atomic::AtomicI64> = OnceLock::new();

fn last_refreshed_store() -> &'static std::sync::atomic::AtomicI64 {
    LAST_REFRESHED.get_or_init(|| std::sync::atomic::AtomicI64::new(0))
}

/// 清单是否来自远程（界面「来源」列用）。
///
/// 判据是「拉过至少一次」而不是「缓存非空」—— 理由见 [`LAST_REFRESHED`]。
pub fn remote_refreshed() -> bool {
    last_refreshed_store().load(std::sync::atomic::Ordering::Relaxed) > 0
}

/// 最近一次远程拉取的时刻（毫秒；0 表示从未拉过）。
pub fn last_refreshed_at() -> i64 {
    last_refreshed_store().load(std::sync::atomic::Ordering::Relaxed)
}

/// 进程内清单句柄；**首次访问时**从落盘缓存读回（各家同一模式，见
/// `providers::catalog_cache` 的模块头）。
///
/// ── 为什么恢复放在这里而不是另给一个 `restore()` ────────────────
/// `restore_cached_catalogs`（启动恢复）的手段是逐家调一次 `list_models()`
/// —— 它要的只是「让各家的句柄初始化一次」这个副作用，不会、也不该知道
/// 某家内部叫什么。因此恢复必须挂在句柄的首次初始化上，否则那次调用
/// 拿到的仍然是空清单（`raccoon` / `chatglm` / `officeace` 都是这么挂的）。
fn manifest_store() -> &'static RwLock<Vec<Value>> {
    MANIFEST.get_or_init(|| {
        let mut restored = Vec::new();
        if let Some(cached) = catalog_cache::load(SCOPE) {
            // 恢复 `fetchedAt` 而不是记成「现在」：界面「更新日期」那一列
            // 要回答的是「这份清单**什么时候拉到的**」，重启一次就把它刷成
            // 当前时间等于撒谎。
            if cached.fetched_at > 0 {
                last_refreshed_store()
                    .store(cached.fetched_at, std::sync::atomic::Ordering::Relaxed);
            }
            restored = cached.models;
        }
        RwLock::new(restored)
    })
}

/// 静态兜底清单。
///
/// ── 为什么必须有兜底（不能返回空数组）────────────────────────
/// 入口校验以**广告视图**为准（`api::pipeline::resolve_model`）：清单为空
/// 意味着用户手里一个模型都点不动，即便他账号是好的、上游也通。
/// 这三条是实测确认可用的（三个都 200），所以兜底是**可用清单**而不是占位符。
pub fn fallback_manifest() -> Vec<Value> {
    vec![
        model_entry("flash", "标准", 1_000_000, 0.1, true, true, true),
        model_entry("pro", "高级", 1_000_000, 1.0, true, false, false),
        model_entry("qwen3.8-max-preview", "Qwen3.8-Max", 1_000_000, 1.8, true, true, false),
    ]
}

/// 构造一条统一形状的模型记录。
///
/// `price_factor` 是相对倍率（`flash` 0.1x、`pro` 1x、`qwen3.8-max` 1.8x），
/// 面板上用它显示「这条请求大概花多少额度」—— 上游不给绝对价格，
/// 相对倍率是唯一能拿到的信息。
///
/// ── 键名为什么是 `maxInputTokens` / `supportsImages` / `supportsReasoning`
///    （模块头有完整依据，改回 `contextWindow` 之前先读那段）──────────
/// 这三个是 `models::shape::list_item` 认的键。上游 `catalog` 用的是
/// `max_input_tokens` / `is_vl` / `is_reasoning`，翻成聚合层键名是本函数的
/// 职责之一 —— 翻错等于把 1M 上下文与两个能力位整份丢掉。
///
/// ── 两个**故意不发**的键 ────────────────────────────────────
///   * `maxOutputTokens`：上游 `catalog` 没有输出上限字段，写 `0` 会让
///     `/v1/models` 透出 `max_output_tokens: 0`（客户端按「输出上限 0」理解），
///     而不写这个键聚合层就不输出该字段 —— 「未知」与「0」是两件事。
///     `accio/models.rs` 与 `codearts/models.rs` 是同一取舍；
///   * `supportsToolCall`：上游 `catalog` 未声明工具调用（`harness: "oc"`
///     只说明它跑在编码 harness 上，不是协议级承诺）。不含证据的键就先不发，
///     等实测确认 MaaS 端接受 `tools` 后再补 —— 宁可让客户端保守，
///     也不替上游承诺一项能力。
fn model_entry(
    id: &str,
    label: &str,
    context_window: i64,
    price_factor: f64,
    reasoning: bool,
    vision: bool,
    is_default: bool,
) -> Value {
    json!({
        "id": id,
        "object": "model",
        "owned_by": "qwenwork",
        "name": label,
        "displayName": label,
        "maxInputTokens": context_window,
        "supportsReasoning": reasoning,
        "supportsImages": vision,
        "isDefault": is_default,
        "priceFactor": price_factor,
    })
}

/// `list_models` 的同步入口：缓存优先，空则兜底。
pub fn manifest() -> Vec<Value> {
    match manifest_store().read() {
        Ok(guard) if !guard.is_empty() => guard.clone(),
        _ => fallback_manifest(),
    }
}

/// 把上游 `chat-modes` 的响应转成统一形状。
///
/// 只收 `enable != false` 的条目（上游用 `enable` 下线模型），
/// 并跳过没有 `key` 的记录（防御上游加新字段）。
fn parse_catalog(body: &Value) -> Vec<Value> {
    let Some(items) = body.get("catalog").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if item.get("enable").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let id = item.get("key").and_then(Value::as_str).unwrap_or("").trim();
        if id.is_empty() {
            continue;
        }
        let label = item
            .pointer("/i18n/display_name/zh")
            .and_then(Value::as_str)
            .or_else(|| item.pointer("/i18n/display_name/en").and_then(Value::as_str))
            .unwrap_or(id);
        let context_window = item
            .get("max_input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let price_factor = item
            .get("price_factor")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let is_default = item
            .get("is_default")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        out.push(model_entry(
            id,
            label,
            context_window,
            price_factor,
            item.get("is_reasoning").and_then(Value::as_bool).unwrap_or(false),
            item.get("is_vl").and_then(Value::as_bool).unwrap_or(false),
            is_default,
        ));
    }
    out
}

/// 拉一次目录并写进进程内缓存。返回条目数。
///
/// ── 失败时不动缓存（重要）───────────────────────────────────
/// 网络抖动或上游 5xx 不该把一份**好用的清单**换成空数组 —— 那会让
/// 用户「刷新了一下，模型全没了」。失败保留旧值，把原因交给调用方显示。
pub async fn fetch_and_store(jwt: &str) -> Result<usize, GatewayError> {
    let url = endpoints::gateway_url(endpoints::CHAT_MODES_PATH);
    let client = crate::server::core::egress::client_for_system_proxy();
    let response = client
        .get(&url)
        .timeout(super::CONTROL_TIMEOUT)
        .header("Authorization", format!("Bearer {jwt}"))
        .header("Accept", "application/json")
        .header("User-Agent", super::user_agent())
        .send()
        .await
        .map_err(|err| GatewayError::with_status(502, format!("千问办公模型目录不可达：{err}")))?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        logging::log_event(
            "warn",
            "qwenwork",
            "模型目录拉取失败",
            Some(json!({ "status": status, "body": text.chars().take(300).collect::<String>() })),
        );
        return Err(GatewayError::with_status(
            if status == 401 { 401 } else { 502 },
            format!("千问办公模型目录返回 HTTP {status}"),
        ));
    }
    let body: Value = serde_json::from_str(&text)
        .map_err(|_| GatewayError::with_status(502, "千问办公模型目录不是合法 JSON"))?;
    let entries = parse_catalog(&body);
    if entries.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "千问办公模型目录为空（上游可能调整了 catalog 结构）",
        ));
    }
    let now = auth::now_ms();
    if let Ok(mut guard) = manifest_store().write() {
        *guard = entries.clone();
    }
    // 记下拉取时刻：界面「来源 / 更新日期」两列读这里（见 `LAST_REFRESHED`）
    last_refreshed_store().store(now, std::sync::atomic::Ordering::Relaxed);
    // 落盘：进程重启后从这份恢复，而不是退回静态兜底（见 `catalog_cache`
    // 的模块头）。写失败只记日志，不影响本次刷新的成功。
    catalog_cache::save(SCOPE, &entries, now);
    Ok(entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_catalog_shape() {
        let body = json!({
            "schema_version": 2,
            "catalog": [
                {"key": "flash", "i18n": {"display_name": {"zh": "标准", "en": "Standard"}},
                 "is_default": true, "price_factor": 0.1, "enable": true,
                 "is_reasoning": true, "is_vl": true, "max_input_tokens": 1000000},
                {"key": "pro", "i18n": {"display_name": {"zh": "高级"}},
                 "is_default": false, "price_factor": 1.0, "enable": true,
                 "is_reasoning": true, "is_vl": false, "max_input_tokens": 1000000}
            ]
        });
        let entries = parse_catalog(&body);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].get("id").and_then(Value::as_str), Some("flash"));
        assert_eq!(entries[0].get("name").and_then(Value::as_str), Some("标准"));
        // 聚合层键名（不是上游的 `max_input_tokens`）—— 见 `model_entry`
        assert_eq!(
            entries[0].get("maxInputTokens").and_then(Value::as_i64),
            Some(1_000_000)
        );
        assert_eq!(
            entries[0].get("supportsImages").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            entries[0].get("supportsReasoning").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(entries[0].get("isDefault").and_then(Value::as_bool), Some(true));
        // `pro` 的 is_vl 是 false：两个能力位必须各自跟着上游走，不能一起写死
        assert_eq!(
            entries[1].get("supportsImages").and_then(Value::as_bool),
            Some(false)
        );
    }

    /// ★ 条目经**聚合层** `list_item` 之后仍然带着上下文与两个能力位。
    ///
    /// 这条锁的是 `model_entry` 与 `models::shape::list_item` 之间的契约：
    /// 两家各写各的键名时单测照样通过（各自测自己的键），只有走一遍真正的
    /// 映射函数才会暴露「1M 上下文丢了 / 能力位恒 false」。
    #[test]
    fn entry_survives_the_aggregation_mapping() {
        // `shape` 是私有模块，`list_item` 由 `models` 重导出（`pub use shape::…`）
        use crate::server::core::models::list_item;
        let entry = model_entry("flash", "标准", 1_000_000, 0.1, true, true, true);
        let item = list_item(&entry, "qwenwork");
        assert_eq!(
            item.get("max_input_tokens").and_then(Value::as_i64),
            Some(1_000_000),
            "1M 上下文必须出现在 /v1/models 的条目里"
        );
        assert_eq!(
            item.get("supports_images").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            item.get("supports_reasoning").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            item.pointer("/input_modalities").and_then(Value::as_array).map(Vec::len),
            Some(2),
            "视觉模型的 input_modalities 应含 text + image"
        );
        // 上游没给输出上限 ⇒ 不该透出 `max_output_tokens: 0`
        assert!(
            item.get("max_output_tokens").is_none(),
            "没有输出上限时不该透出 0 ——「未知」与「0」是两件事"
        );
    }

    #[test]
    fn skips_disabled_and_keyless() {
        let body = json!({
            "catalog": [
                {"key": "off", "enable": false},
                {"i18n": {"display_name": {"zh": "无 key"}}},
                {"key": "on", "enable": true}
            ]
        });
        let entries = parse_catalog(&body);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].get("id").and_then(Value::as_str), Some("on"));
    }

    #[test]
    fn empty_catalog_yields_no_entries() {
        assert!(parse_catalog(&json!({})).is_empty());
        assert!(parse_catalog(&json!({"catalog": []})).is_empty());
    }

    #[test]
    fn fallback_is_never_empty() {
        // 兜底清单必须非空，否则入口校验会把用户所有模型挡掉
        assert!(!fallback_manifest().is_empty());
    }
}
