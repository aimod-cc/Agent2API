//! Coze 模型目录：**网页版（claw）的平台模型清单**。
//!
//! ── ⚠️ 这里修正过一次结论（2026-10-22）──────────────────────
//! 第一版把「模型 = 用户自己的 bot（`api.coze.cn/v3/chat` 的 `bot_id`）」当成
//! 结论，于是清单是 `/v1/bots` —— 用户没发布过 bot 时恒为空，看着像「这家没模型」。
//! 那是**错的**：`v3/chat` 是 OpenAPI（把 bot 当 API 卖的场景），与网页版聊天
//! 是两条平行的产品线。网页版走的是 claw，**平台提供 9 个文本模型**，
//! 按会话选一个。证据与完整字段见 `_recon/coze-code-contract-2026-10-21.md` §4。
//!
//! ── 目录链路（活 PAT 实测）──────────────────────────────────
//!   1. `POST /api/coze_claw/agent/list {}` → `data.agents[0].agent_id`
//!      （实测三个：扣子 / 视频导演 / 编程专家。**模型清单是账号级的**，
//!      三个 agent 返回完全一致，取第一个即可）；
//!   2. `POST /api/coze_claw/resource/get_model_list {claw_id}`
//!      → `data.models[]`（`name` / `display_name` / `model_desc` /
//!      `capability[]` / `has_model_access` / `remaining_quota_percent` /
//!      `consumption_multiplier` / `selected`）。
//! ⚠️ `claw_id` 是**必填**的（漏了回 `1000001 参数错误`，看着像接口坏了）。
//!
//! ── 为什么「有清单」是常态 ──────────────────────────────────
//! 本家的清单是**平台给的**，与用户有没有发布 bot 无关 —— 所以它有内容才是正常的。
//! （第一版「无静态兜底、空即真实」的判断随结论一起作废。）
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 进程级内存 + `catalog_cache::SCOPE_COZE` 落盘（重启恢复，与 AtomCode 同范式）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic；持锁不跨 await。

use std::sync::Mutex;
use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::ModelRefreshOutcome;
use crate::server::core::providers::catalog_cache;
use crate::server::errors::GatewayError;

use super::credentials;
use super::http;
use super::WEB_BASE;

/// 本家目录缓存槽（`ALL_SCOPES` 已登记；单地区单格）
pub const SCOPE: &str = "coze";

/// 远程目录缓存有效期（10 分钟，与各家中位值一致）
const CATALOG_CACHE_TTL_MS: i64 = 10 * 60_000;

/// 目录请求超时
const CATALOG_TIMEOUT_MS: u64 = 20_000;

/// 内存态目录。
struct CatalogState {
    /// 清单条目（`{id, name}`；id = 上游的 `name`）
    models: Vec<Value>,
    fetched_at: i64,
}

/// 进程级目录状态（毒锁按「数据可重建」处理，与本仓其它目录模块同口径）
fn state() -> &'static Mutex<Option<CatalogState>> {
    static STATE: OnceLock<Mutex<Option<CatalogState>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

/// 在锁内执行一段读写，并在首次访问时把落盘缓存装回内存。
fn with_state<R>(f: impl FnOnce(&mut Option<CatalogState>) -> R) -> R {
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_none() {
        if let Some(cached) = catalog_cache::load(SCOPE) {
            let models: Vec<Value> = cached
                .models
                .iter()
                .filter_map(entry_from_cached)
                .collect();
            if !models.is_empty() {
                *guard = Some(CatalogState {
                    models,
                    fetched_at: cached.fetched_at,
                });
            }
        }
    }
    f(&mut guard)
}

/// 从落盘缓存条目还原。
fn entry_from_cached(value: &Value) -> Option<Value> {
    let id = value.get("id").and_then(Value::as_str)?.trim();
    if id.is_empty() {
        return None;
    }
    Some(json!({
        "id": id,
        "name": value.get("name").and_then(Value::as_str).unwrap_or(id),
    }))
}

/// 模型清单（内存态；没拉到过就返回空 —— 界面据此显示「未获取」）。
pub fn list() -> Vec<Value> {
    with_state(|guard| {
        guard
            .as_ref()
            .map(|catalog| catalog.models.clone())
            .unwrap_or_default()
    })
}

/// 远程目录是否拉到过（`catalog::refresh_meta` 的「来源」列判据）。
pub fn remote_refreshed() -> bool {
    with_state(|guard| {
        guard
            .as_ref()
            .map(|catalog| !catalog.models.is_empty())
            .unwrap_or(false)
    })
}

/// 最后一次远程刷新的时刻（毫秒；没刷过给 0）。
pub fn last_refreshed_at() -> i64 {
    with_state(|guard| guard.as_ref().map(|catalog| catalog.fetched_at).unwrap_or(0))
}

/// 刷新远程目录（`force = true` 跳过 TTL；失败不返回错误，保留现有清单）。
pub async fn refresh(
    store: &AccountStore,
    account_id: &str,
    force: bool,
) -> ModelRefreshOutcome {
    // TTL 早退（只有自动路径会走到）
    if !force {
        let now = crate::server::logging::now_ms();
        let fresh = with_state(|guard| {
            guard
                .as_ref()
                .map(|catalog| now - catalog.fetched_at < CATALOG_CACHE_TTL_MS)
                .unwrap_or(false)
        });
        if fresh {
            return ModelRefreshOutcome::unchanged();
        }
    }
    // PAT 铸造失败（没账号 / 登录态失效）→ 「没刷」而不是「失败」
    let pat = match credentials::ensure_pat(store, account_id, false).await {
        Ok(pat) => pat,
        Err(error) => {
            crate::server::logging::verbose(
                "[Models]",
                &format!("Coze 模型目录刷新跳过：{}", error.message),
            );
            return ModelRefreshOutcome::unchanged();
        }
    };
    // 出口跟着账号走（与转发链同源）：目录也是一次真实上游请求，没理由绕过
    // 用户给这个账号配的代理。
    let proxy = credentials::account_proxy(store, account_id);
    match fetch_remote(&pat, proxy.as_ref()).await {
        Ok(models) => {
            let fetched_at = crate::server::logging::now_ms();
            catalog_cache::save(SCOPE, &models, fetched_at);
            let count = models.len();
            with_state(|guard| {
                *guard = Some(CatalogState {
                    models,
                    fetched_at,
                });
            });
            crate::server::logging::log(
                "[Models]",
                &format!("✅ Coze 模型目录已更新（{count} 个模型）"),
            );
            ModelRefreshOutcome::refreshed(count)
        }
        Err(error) => {
            crate::server::logging::verbose(
                "[Models]",
                &format!("Coze 模型目录刷新失败：{}", error.message),
            );
            ModelRefreshOutcome::failed(error.message)
        }
    }
}

/// 真打一次上游目录链路（agents → get_model_list，两步都只要 PAT）。
async fn fetch_remote(
    pat: &str,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
) -> Result<Vec<Value>, GatewayError> {
    let headers = credentials::pat_headers(pat);
    let claw_id = first_claw_id_for(&headers, proxy).await?;
    let url = format!("{WEB_BASE}/api/coze_claw/resource/get_model_list");
    // ⚠️ `claw_id` 必填：漏了上游回 `1000001 参数错误`
    let body = json!({ "claw_id": claw_id });
    let value =
        http::post_json_value(&url, &headers, &body, proxy, Some(CATALOG_TIMEOUT_MS)).await?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("未知错误")
            .to_string();
        return Err(GatewayError::with_status(
            502,
            format!("Coze 模型清单接口失败（code={code}：{message}）"),
        ));
    }
    let mut models: Vec<Value> = Vec::new();
    if let Some(items) = value
        .get("data")
        .and_then(|data| data.get("models"))
        .and_then(Value::as_array)
    {
        for item in items {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let label = item
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .unwrap_or(&name)
                .to_string();
            // 没有访问权的模型照样列（管理员可能还没开），转发时上游会明确报错 ——
            // 比「清单里干脆看不到」好排障。
            models.push(json!({ "id": name, "name": label }));
        }
    }
    if models.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "Coze 模型清单返回空（上游响应异常，或该账号没有任何可用模型）",
        ));
    }
    Ok(models)
}

/// 取一个可用的 `claw_id`（`agent/list` 的第一条），带账号级出口。
///
/// 模型清单是**账号级**的（三个 agent 返回完全一致，实测），所以取第一个就够。
/// 转发（`chat.rs`）与目录刷新（`fetch_remote`）共用这一个实现 —— 两处都传账号
/// 自己的代理；「不带代理」的那个包装已删除（留着只会让下一处调用悄悄退回直连）。
pub(super) async fn first_claw_id_for(
    headers: &[(String, String)],
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
) -> Result<String, GatewayError> {
    let url = format!("{WEB_BASE}/api/coze_claw/agent/list");
    let value =
        http::post_json_value(&url, headers, &json!({}), proxy, Some(CATALOG_TIMEOUT_MS)).await?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("未知错误")
            .to_string();
        return Err(GatewayError::with_status(
            502,
            format!("Coze 智能体列表接口失败（code={code}：{message}）"),
        ));
    }
    value
        .get("data")
        .and_then(|data| data.get("agents"))
        .and_then(Value::as_array)
        .and_then(|agents| agents.first())
        .and_then(|agent| agent.get("agent_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            GatewayError::with_status(
                502,
                "Coze 智能体列表为空（该账号还没有扣子空间智能体，无法取模型清单）",
            )
        })
}
