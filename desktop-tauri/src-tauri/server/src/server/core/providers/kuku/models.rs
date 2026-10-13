//! KukuAI 模型目录：静态兜底 + 远程刷新（`/wenchain/genflowpro/model/list`）。
//!
//! ── 为什么有两层（与 raccoon / catpaw 同一结构）────────────────
//!   1. **静态兜底**：kuku2api 2026-09-13 实测的 `MODEL_IDS`（上游模型接口
//!      响应体的 `model_name` 集合，13 个）。离线时 `/v1/models` 也有清单可列，
//!      未知模型名照样**显式回退 `auto`**（不静默降级）；
//!   2. **远程刷新**：上游清单会变（新增模型 / 下线），`GET /wenchain/genflowpro/
//!      model/list`（带会话三件套）拉最新列表。TTL 10 分钟，`force = true` 跳过。
//!      注意路径是 `model/list`（2026-10-07 逆向 app.asar + 真实凭证实测；
//!      旧路径 `model_list` 已废弃返回 404），响应是顶层 `{status, data}` 包络
//!      （与余额接口同款，不是 `errno` 顶层）。
//!
//! ── 条目形态 ────────────────────────────────────────────────
//! 聚合层 `models::list_item` 读 `id` / `name` 等字段。kuku 的模型 id 就是上游
//! `model_name`（`gateway-deepseek-v4.1-flash-tencent` 这类长名与别家天然不撞，
//! 客户端可直接点名；`auto` 是上游自动选路）。本模块只给 id + name，其余能力
//! 字段（图像 / 思考 / 工具）上游不声明，不给假值。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic；持锁不跨 await。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::credentials::{self, KukuCredentials, request_headers};
use super::session::{self, TokenTriple};
use super::{BASE_URL, DEFAULT_MODEL, WEB_QUERY};

/// 远程目录缓存有效期（10 分钟，与 raccoon 同值）
const CATALOG_CACHE_TTL_MS: i64 = 10 * 60_000;

/// 目录请求超时
const CATALOG_TIMEOUT_MS: u64 = 10_000;

/// 静态兜底清单（kuku2api 实测的 `MODEL_IDS`，2026-09-13）。
pub const FALLBACK_MODEL_IDS: [&str; 13] = [
    "auto",
    "gateway-deepseek-v4.1-flash-tencent",
    "gateway-deepseek-v4-pro-tencent",
    "gateway-deepseek-v4-flash-tencent",
    "gateway-glm-5.3-flash",
    "glm-5.3",
    "gateway-glm-5.2",
    "gateway-glm-5.1-kuaishou",
    "ernie-5.1",
    "ms-kimi-k3",
    "gateway-kimi-k2.7-code-tencent",
    "gateway-kimi-k2.6",
    "ali-minimax/minimax-m3",
];

struct RemoteCatalog {
    models: Vec<Value>,
    fetched_at_ms: i64,
}

/// 进程级远程目录缓存（不落盘：模型清单变化不频繁，重启后回落到静态兜底，
/// 首次访问或手动刷新时再拉）
static REMOTE: Mutex<Option<RemoteCatalog>> = Mutex::new(None);

fn remote_guard() -> std::sync::MutexGuard<'static, Option<RemoteCatalog>> {
    match REMOTE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 模型清单（远程优先，未拉到 / 未刷过用静态兜底）。
pub fn list() -> Vec<Value> {
    let guard = remote_guard();
    if let Some(catalog) = guard.as_ref() {
        if !catalog.models.is_empty() {
            return catalog.models.clone();
        }
    }
    drop(guard);
    FALLBACK_MODEL_IDS.iter().map(|id| listing_entry(id)).collect()
}

/// 一条清单条目（聚合层 `models::list_item` 的口径：id + name）。
fn listing_entry(model_name: &str) -> Value {
    json!({
        "id": model_name,
        "name": model_name,
    })
}

/// 从上游响应里宽松提取模型名集合（`model_list` 的响应结构未公开，容错读取：
/// 递归找 `model_name` / `modelName` / `id` / `name` 字符串字段；认不出就空集，
/// 由调用方回落静态兜底）。
fn extract_model_names(value: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    collect_model_names(value, &mut names);
    names.sort();
    names.dedup();
    names
}

fn collect_model_names(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let lower = key.to_ascii_lowercase();
                if matches!(lower.as_str(), "model_name" | "modelname" | "modelid")
                    && child.is_string()
                {
                    if let Some(text) = child.as_str() {
                        if !text.trim().is_empty() {
                            out.push(text.trim().to_string());
                            continue;
                        }
                    }
                }
                collect_model_names(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_model_names(item, out);
            }
        }
        _ => {}
    }
}

/// 远程目录是否拉到过（`catalog::refresh_meta` 的「来源」列判据）。
pub fn remote_refreshed() -> bool {
    let guard = remote_guard();
    guard
        .as_ref()
        .map(|catalog| !catalog.models.is_empty())
        .unwrap_or(false)
}

/// 最后一次远程刷新的时刻（毫秒；没刷过给 0）。
pub fn last_refreshed_at() -> i64 {
    let guard = remote_guard();
    guard.as_ref().map(|catalog| catalog.fetched_at_ms).unwrap_or(0)
}

/// 刷新远程目录（`force = true` 跳过 TTL；失败不返回错误，保留现有清单）。
pub async fn refresh(
    store: &AccountStore,
    account_id: &str,
    force: bool,
) -> crate::server::core::providers::adapter::ModelRefreshOutcome {
    // 凭证解析失败（没账号、也没本机登录态）→ 「没刷」而不是「失败」
    let credentials = match credentials::snapshot_for(store, account_id) {
        Ok(credentials) => credentials,
        Err(error) => {
            crate::server::logging::verbose(
                "[Models]",
                &format!("KukuAI 模型目录刷新跳过：{}", error.message),
            );
            return crate::server::core::providers::adapter::ModelRefreshOutcome::unchanged();
        }
    };
    let proxy = None;
    // TTL 早退（只有自动路径会走到）
    if !force {
        let now = crate::server::logging::now_ms();
        let guard = remote_guard();
        if let Some(catalog) = guard.as_ref() {
            if now - catalog.fetched_at_ms < CATALOG_CACHE_TTL_MS {
                return crate::server::core::providers::adapter::ModelRefreshOutcome::unchanged();
            }
        }
        drop(guard);
    }
    match fetch_remote(&credentials, proxy).await {
        Ok(models) => {
            let now = crate::server::logging::now_ms();
            let mut guard = remote_guard();
            let catalog = guard.get_or_insert_with(|| RemoteCatalog {
                models: Vec::new(),
                fetched_at_ms: 0,
            });
            catalog.models = models.clone();
            catalog.fetched_at_ms = now;
            drop(guard);
            crate::server::logging::log(
                "[Models]",
                &format!("✅ KukuAI 模型目录已更新（{} 个）", models.len()),
            );
            crate::server::core::providers::adapter::ModelRefreshOutcome::refreshed(models.len())
        }
        Err(error) => {
            crate::server::logging::verbose(
                "[Models]",
                &format!("KukuAI 模型目录刷新失败：{}", error.message),
            );
            crate::server::core::providers::adapter::ModelRefreshOutcome::failed(error.message)
        }
    }
}

/// 真打一次上游目录接口。
async fn fetch_remote(
    credentials: &KukuCredentials,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
) -> Result<Vec<Value>, GatewayError> {
    let triple: TokenTriple = session::ensure_tokens(credentials, proxy, false).await?;
    // 客户端路径是 `/wenchain/genflowpro/model/list`（2026-10-07 逆向
    // app.asar + 真实凭证实测；旧路径 `model_list` 已废弃返回 404）。
    let url = format!(
        "{BASE_URL}/wenchain/genflowpro/model/list?{WEB_QUERY}{}",
        session::token_query(&triple)
    );
    let headers = request_headers(credentials);
    let value = super::http::get_json_value(&url, &headers, proxy, Some(CATALOG_TIMEOUT_MS)).await?;
    // 顶层包络（与余额接口同款，2026-10-07 实测）：判据在 status.code，
    // 业务字段在 data.model_list（`errno` 顶层是旧版/其他接口的形态）。
    let status = value.get("status").cloned().unwrap_or(Value::Null);
    let code = status.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = status
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("未知错误")
            .to_string();
        return Err(GatewayError::with_status(
            502,
            format!("KukuAI 模型目录接口失败（status.code={code}：{message}）"),
        ));
    }
    let names = value
        .get("data")
        .map(extract_model_names)
        .unwrap_or_default();
    let mut seen: HashMap<String, bool> = HashMap::new();
    let mut models: Vec<Value> = Vec::new();
    for name in names {
        if seen.insert(name.clone(), true).is_none() {
            models.push(listing_entry(&name));
        }
    }
    if models.is_empty() {
        // 认不出结构 → 回落静态兜底（保底：至少 auto 可用）
        models = FALLBACK_MODEL_IDS.iter().map(|id| listing_entry(id)).collect();
    }
    Ok(models)
}

/// 把客户端请求的模型名归一成上游 `model_name`。
///
/// 接受 `auto` / `kuku/<name>` / 裸名；未知模型显式回退 `auto`
/// （上游对未知名会静默降级，显式回退更清晰，与参考实现同一取舍）。
pub fn resolve_model(name: &str) -> String {
    let mut name = name.trim().to_string();
    if let Some(rest) = name.strip_prefix("kuku/") {
        name = rest.trim().to_string();
    }
    if name.is_empty() {
        return DEFAULT_MODEL.to_string();
    }
    let known = {
        let guard = remote_guard();
        let remote_known = guard
            .as_ref()
            .map(|catalog| catalog.models.iter().any(|m| m.get("id").and_then(Value::as_str) == Some(name.as_str())))
            .unwrap_or(false);
        drop(guard);
        remote_known || FALLBACK_MODEL_IDS.contains(&name.as_str())
    };
    if known {
        name
    } else {
        DEFAULT_MODEL.to_string()
    }
}
