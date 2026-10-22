//! QClaw 的模型目录：`GET http://127.0.0.1:{models_port}/proxy/llm/models`。
//!
//! ── 上游返回什么（实测 2026-10-20，v0.3.0.631）────────────────
//! ```json
//! {
//!   "object": "list",
//!   "data": [
//!     { "id": "modelroute", "object": "model", "created": 0,
//!       "owned_by": "qclaw", "name": "Auto",
//!       "description": "兼顾质量与速度，根据任务复杂度从内置模型中智能选择",
//!       "input": ["text", "image"], "contextWindow": 250000,
//!       "status_level": 0, "display_id": "default", "icon": "https://…" },
//!     { "id": "pool-hy3-preview", "name": "Hy3", "input": ["text"],
//!       "contextWindow": 262144, … },
//!     … 共 11 条（modelroute / pool-hy3-preview / pool-deepseek-v4-pro /
//!       pool-deepseek-v4-flash / pool-glm-5.2 / pool-glm-5.2-night /
//!       pool-glm-5.1 / pool-kimi-k2.7-code-highspeed / pool-kimi-k2.6 /
//!       pool-minimax-m3 / pool-minimax-m2.7）
//!   ]
//! }
//! ```
//!
//! ── 两个事实，改这份文件前必须知道 ───────────────────────────
//!   1. **`id` 就是转发时要用的模型名**（`x-openclaw-model: qclaw/<id>`，
//!      见 `mod.rs` 的 `build_chat_request`），不要「规范化」它；
//!   2. **这个接口不校验令牌**（实测任意 Bearer 都 200，包括占位符）——
//!      它是本机 handler 用自己的凭证转 jprx 换来的清单。所以拉取失败
//!      只有一种原因：QClaw 没在跑（连不上 127.0.0.1）。
//!
//! ── 分发形态（与 qwenwork/models.rs 同一套理由）───────────────
//! `ProviderAdapter::list_models` 要的是**聚合层键名**（`maxInputTokens` /
//! `supportsImages` / `supportsReasoning` / `supportsToolCall` /
//! `maxOutputTokens`，见 `models::shape::list_item`），上游的
//! `contextWindow` / `input[]` 在这里翻完再交出去。
//!
//! ── 两个**故意不发**的键 ────────────────────────────────────
//!   * `supportsReasoning`：上游清单没有推理能力字段。GLM / DeepSeek /
//!     Kimi 现实中都是推理模型，但「未声明」与「声明了」是两件事，
//!     按 qwenwork 的口径（`harness: "oc"` 不算协议级承诺）不含证据的键
//!     就先不发，宁可让客户端保守；
//!   * `supportsToolCall`：同理，等实测确认网关接受 `tools` 再补。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use std::sync::{OnceLock, RwLock};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::server::errors::GatewayError;
use crate::server::logging;

use super::local;
use crate::server::core::providers::catalog_cache;

/// 本家在 `catalog_cache` 里的 scope 名。
pub const SCOPE: &str = catalog_cache::SCOPE_QCLAW;

/// 模型清单拉取超时（本机回环，10 秒够；QClaw 没在跑时连接会被立即拒绝）。
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// 进程内缓存的模型清单（远程拉成功后写入，供 `list_models` 同步读取）。
static MANIFEST: OnceLock<RwLock<Vec<Value>>> = OnceLock::new();

/// 最近一次**远程拉取成功**的时刻（毫秒时间戳，0 表示从未拉过）。
///
/// 与 `MANIFEST` 分开存而不是「清单非空就是拉过」：兜底清单也可能被写进
/// 缓存，用清单长度当判据会把「内置兜底」误报成「远程来源」（与
/// `qwenwork/models.rs` 同一取舍）。
static LAST_REFRESHED: OnceLock<AtomicI64> = OnceLock::new();

fn last_refreshed_store() -> &'static AtomicI64 {
    LAST_REFRESHED.get_or_init(|| AtomicI64::new(0))
}

/// 清单是否来自远程（界面「来源」列用）。
pub fn remote_refreshed() -> bool {
    last_refreshed_store().load(Ordering::Relaxed) > 0
}

/// 最近一次远程拉取的时刻（毫秒；0 表示从未拉过）。
pub fn last_refreshed_at() -> i64 {
    last_refreshed_store().load(Ordering::Relaxed)
}

/// 进程内清单句柄；**首次访问时**从落盘缓存读回（各家同一模式）。
fn manifest_store() -> &'static RwLock<Vec<Value>> {
    MANIFEST.get_or_init(|| {
        let mut restored = Vec::new();
        if let Some(cached) = catalog_cache::load(SCOPE) {
            if cached.fetched_at > 0 {
                last_refreshed_store().store(cached.fetched_at, Ordering::Relaxed);
            }
            restored = cached.models;
        }
        RwLock::new(restored)
    })
}

/// 构造一条统一形状的模型记录。
///
/// `supports_images` 由上游 `input[]` 里有没有 `image` 决定（那是上游
/// 明写的能力声明，不是猜的）。
fn model_entry(id: &str, label: &str, description: &str, context_window: i64, supports_images: bool) -> Value {
    json!({
        "id": id,
        "object": "model",
        "owned_by": "qclaw",
        "name": label,
        "displayName": label,
        "description": description,
        "maxInputTokens": context_window,
        "supportsImages": supports_images,
    })
}

/// 静态兜底清单（2026-10-20 v0.3.0.631 实测的 11 条）。
///
/// ── 为什么必须有兜底 ────────────────────────────────────
/// 入口校验以广告视图为准（`api::pipeline::resolve_model`）：清单为空
/// 意味着用户手里一个模型都点不动，即便网关是通的。QClaw 没在跑时
/// `list_models` 走的就是这条 —— 让「账号加好了但客户端没开」的用户
/// 至少能在界面上看到本家有哪些模型。
pub fn fallback_manifest() -> Vec<Value> {
    vec![
        model_entry("modelroute", "Auto", "兼顾质量与速度，根据任务复杂度从内置模型中智能选择", 250_000, true),
        model_entry("pool-hy3-preview", "Hy3", "", 262_144, false),
        model_entry("pool-deepseek-v4-pro", "DeepSeek-V4-Pro", "", 1_048_576, false),
        model_entry("pool-deepseek-v4-flash", "DeepSeek-V4-Flash", "", 1_048_576, false),
        model_entry("pool-glm-5.2", "GLM-5.2", "", 1_048_576, false),
        model_entry("pool-glm-5.2-night", "GLM-5.2 · 夜间专属", "", 1_048_576, false),
        model_entry("pool-glm-5.1", "GLM-5.1", "", 204_800, false),
        model_entry("pool-kimi-k2.7-code-highspeed", "Kimi-K2.7-Code-HighSpeed", "", 262_144, true),
        model_entry("pool-kimi-k2.6", "Kimi-K2.6", "", 262_144, true),
        model_entry("pool-minimax-m3", "MiniMax-M3", "", 204_800, true),
        model_entry("pool-minimax-m2.7", "MiniMax-M2.7", "", 204_800, false),
    ]
}

/// `list_models` 的同步入口：缓存优先，空则兜底。
pub fn manifest() -> Vec<Value> {
    match manifest_store().read() {
        Ok(guard) if !guard.is_empty() => guard.clone(),
        _ => fallback_manifest(),
    }
}

/// 把上游 `/proxy/llm/models` 的响应转成统一形状。
fn parse_models(body: &Value) -> Vec<Value> {
    let Some(items) = body.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let id = item.get("id").and_then(Value::as_str).unwrap_or("").trim();
        if id.is_empty() {
            continue;
        }
        // 上游用 `status_level` 表达模型状态（0 = 可用）；非 0 的一律跳过
        // （防御性：将来下线模型若还留在清单里，别广告给用户）
        if item.get("status_level").and_then(Value::as_i64).unwrap_or(0) != 0 {
            continue;
        }
        let label = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(id);
        let description = item.get("description").and_then(Value::as_str).unwrap_or("");
        let context_window = item.get("contextWindow").and_then(Value::as_i64).unwrap_or(0);
        let supports_images = item
            .get("input")
            .and_then(Value::as_array)
            .map(|modalities| {
                modalities
                    .iter()
                    .any(|m| m.as_str() == Some("image"))
            })
            .unwrap_or(false);
        out.push(model_entry(id, label, description, context_window, supports_images));
    }
    out
}

/// 拉一次目录并写进进程内缓存。返回条目数。
///
/// ── 失败时不动缓存（重要）───────────────────────────────────
/// 与 qwenwork 同一口径：网络抖动不该把一份好用的清单换成空数组。
pub async fn fetch_and_store(models_port: u16, token: &str) -> Result<usize, GatewayError> {
    // 回环请求必须直连：走系统代理会把 127.0.0.1 发给代理服务器（必败）
    let client = crate::server::core::egress::client_for(None);
    let response = client
        .get(local::models_url(models_port))
        .timeout(FETCH_TIMEOUT)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| {
            logging::log_event(
                "warn",
                "qclaw",
                "模型清单接口不可达",
                Some(json!({ "error": err.to_string() })),
            );
            GatewayError::with_status(
                502,
                format!(
                    "连不上 QClaw 模型清单接口（127.0.0.1:{models_port}）：请确认 QClaw 客户端正在运行",
                ),
            )
        })?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        logging::log_event(
            "warn",
            "qclaw",
            "模型清单拉取失败",
            Some(json!({ "status": status, "body": text.chars().take(300).collect::<String>() })),
        );
        return Err(GatewayError::with_status(
            502,
            format!("QClaw 模型清单接口返回 HTTP {status}"),
        ));
    }
    let body: Value = serde_json::from_str(&text)
        .map_err(|_| GatewayError::with_status(502, "QClaw 模型清单不是合法 JSON"))?;
    let entries = parse_models(&body);
    if entries.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "QClaw 模型清单为空（上游可能调整了 data 结构）",
        ));
    }
    let now = crate::server::logging::now_ms();
    if let Ok(mut guard) = manifest_store().write() {
        *guard = entries.clone();
    }
    last_refreshed_store().store(now, Ordering::Relaxed);
    catalog_cache::save(SCOPE, &entries, now);
    Ok(entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_models_shape() {
        let body = json!({
            "object": "list",
            "data": [
                {"id": "modelroute", "object": "model", "owned_by": "qclaw",
                 "name": "Auto", "description": "智能选择",
                 "input": ["text", "image"], "contextWindow": 250000, "status_level": 0},
                {"id": "pool-hy3-preview", "name": "Hy3",
                 "input": ["text"], "contextWindow": 262144, "status_level": 0},
                {"id": "gone", "name": "下线模型", "status_level": 3},
                {"name": "无 id"}
            ]
        });
        let entries = parse_models(&body);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].get("id").and_then(Value::as_str), Some("modelroute"));
        assert_eq!(entries[0].get("name").and_then(Value::as_str), Some("Auto"));
        // 聚合层键名（不是上游的 contextWindow）—— 见 model_entry
        assert_eq!(
            entries[0].get("maxInputTokens").and_then(Value::as_i64),
            Some(250_000)
        );
        assert_eq!(
            entries[0].get("supportsImages").and_then(Value::as_bool),
            Some(true)
        );
        // 纯文本模型的能力位必须各自跟着上游走
        assert_eq!(
            entries[1].get("supportsImages").and_then(Value::as_bool),
            Some(false)
        );
    }

    /// ★ 条目经**聚合层** `list_item` 之后仍带着上下文与视觉位。
    #[test]
    fn entry_survives_the_aggregation_mapping() {
        use crate::server::core::models::list_item;
        let entry = model_entry("modelroute", "Auto", "", 250_000, true);
        let item = list_item(&entry, "qclaw");
        assert_eq!(
            item.get("max_input_tokens").and_then(Value::as_i64),
            Some(250_000),
            "上下文窗必须出现在 /v1/models 的条目里"
        );
        assert_eq!(
            item.get("supports_images").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            item.pointer("/input_modalities").and_then(Value::as_array).map(Vec::len),
            Some(2),
            "视觉模型的 input_modalities 应含 text + image"
        );
        // 上游没给推理/工具能力声明 ⇒ 这两个位不该被默认成 true
        assert_eq!(
            item.get("supports_reasoning").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn empty_or_wrong_shape_yields_no_entries() {
        assert!(parse_models(&json!({})).is_empty());
        assert!(parse_models(&json!({"data": []})).is_empty());
        assert!(parse_models(&json!({"data": "oops"})).is_empty());
    }

    #[test]
    fn fallback_is_never_empty() {
        assert!(!fallback_manifest().is_empty());
        // 兜底里必须含默认 agent 用的那个模型（openclaw.json 的 primary）
        assert!(fallback_manifest()
            .iter()
            .any(|m| m.get("id").and_then(Value::as_str) == Some("pool-glm-5.2-night")));
    }
}
