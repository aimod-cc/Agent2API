//! `cloudaicompanionProject` 的发现：`loadCodeAssist` →（无 project 时）`onboardUser`。
//!
//! ── 为什么需要它（规格 §3.4 / 坑 #16）──────────────────────────
//! v1internal 的聊天请求体里要带 `"project": "<cloudaicompanionProject>"`；
//! 这个值是**每个 Google 账号各自的**，不在 token 里，只能问上游要。
//! 拿不到时的后果是「聊天请求不受理 / 受限」，因此它是本家必须接的一条链。
//!
//! ※ 目录 / 额度 / `loadCodeAssist` 自己都**忽略** `project` 字段
//! （规格 §3.4 的实测结论，Manager `quota.rs` 的注释逐字确认），所以本步
//! 只把它当「发现 + 存下来」的动作，失败不阻断账号添加与目录刷新。
//!
//! ── 两个调用的逐字形态（两套参考实现核对过）──────────────────
//! ```text
//! POST https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist
//!   Content-Type: application/json
//!   Authorization: Bearer {access_token}
//!   User-Agent: vscode/1.X.X (Antigravity/{ver})
//!   body: {"metadata":{"ideType":"ANTIGRAVITY"}}          ← Manager 的形态
//!   resp: {"cloudaicompanionProject": "…" | {"id": "…"},
//!          "allowedTiers":[{id,isDefault,…}], "currentTier":…, "paidTier":…}
//!
//! POST https://cloudcode-pa.googleapis.com/v1internal:onboardUser
//!   body: {"tierId":"<allowedTiers 里 isDefault 的 id，默认 legacy-tier>",
//!          "metadata":{"ideType":"ANTIGRAVITY"}}
//!   resp: {"done": true, "response": {"cloudaicompanionProject": …}}
//! ```
//! **两个端点固定走 prod**（[`endpoints::PROJECT_BASE_URL`]）：9router 的注释
//! 写明「the daily host rejects these auth/onboarding calls」，规格 §3.4 同款
//! 提醒（Manager 的 `project_resolver.rs` 里那份 daily→prod 的列表是旧实现，
//! 与它自己的 `quota.rs` 注释矛盾 —— 以规格与 9router 为准）。
//!
//! ── 与 9router 的两处差异（如实记录，未实测）──────────────────
//!   1. `metadata` 只带 `ideType`（Manager 形态）。9router 还带 `platform` /
//!      `pluginType`（**数字枚举**，取自官方二进制），但规格 §3.4 把 Manager
//!      那一形态列为第一手来源 —— 本步按 Manager 发，若实测 project 拿不到，
//!      第一件该试的就是把这两个字段补上（TODO）。
//!   2. 轮询：9router 给 `onboardUser` 最多 2 次尝试、间隔 12 秒 + 抖动。
//!      本仓是**同步等待**的调用点（加账号 / 刷目录），一次请求内串行等
//!      二十几秒会拖住界面，因此本步只发一轮 `onboardUser`：`done != true`
//!      就返回「未开通」，由用户下次刷目录时再试（幂等，可重复）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::time::Duration;

use serde_json::{json, Value};

use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::endpoints;

/// 单次请求超时（project 发现是「顺手问一句」，不该拖住添加流程）
const REQUEST_TIMEOUT_MS: u64 = 20_000;

/// `onboardUser` 的 tier 兜底（规格 §3.4：`allowedTiers` 里没有 default 时用它）
pub const DEFAULT_TIER: &str = "legacy-tier";

/// 两个调用共用的 metadata（Manager `project_resolver.rs` 逐字）
pub fn metadata() -> Value {
    json!({ "ideType": "ANTIGRAVITY" })
}

/// 从 `loadCodeAssist` / `onboardUser` 的响应里读 project id。
///
/// 两种形态都认（规格 §3.4：可能是字符串，也可能是 `{id}` 对象）；
/// 读不到给空串，由调用方决定是「再 onboard 一次」还是「放弃」。
pub fn project_of(payload: &Value) -> String {
    let value = payload.get("cloudaicompanionProject");
    read_project_value(value)
}

/// `onboardUser` 的响应形态：project 裹在 `response` 里
pub fn onboard_project_of(payload: &Value) -> String {
    let value = payload
        .get("response")
        .and_then(|response| response.get("cloudaicompanionProject"));
    read_project_value(value)
}

/// 字符串 / `{id}` 两种形态 → project id
fn read_project_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Object(object)) => object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// `onboardUser` 要用的 tier：`allowedTiers` 里 `isDefault == true` 那条的 id，
/// 没有就用 [`DEFAULT_TIER`]（规格 §3.4 / 9router `fetchProjectId` 同款判据）。
pub fn default_tier(payload: &Value) -> String {
    payload
        .get("allowedTiers")
        .and_then(Value::as_array)
        .and_then(|tiers| {
            tiers.iter().find_map(|tier| {
                let is_default = tier.get("isDefault").and_then(Value::as_bool).unwrap_or(false);
                if !is_default {
                    return None;
                }
                tier.get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string)
            })
        })
        .unwrap_or_else(|| DEFAULT_TIER.to_string())
}

/// 发一次 JSON POST（两个调用共用一个薄封装：头集合与超时都相同）。
async fn post_json(
    url: &str,
    access_token: &str,
    project_id: &str,
    body: &Value,
    proxy: Option<&ResolvedProxy>,
) -> Result<(u16, Value, String), GatewayError> {
    // 本家统一的出口口径：显式配了代理就用它，否则跟随系统代理
    // （project 发现多在登录链路里跑，那一步没有账号可挂，理由见 `super::client_for`）
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
        .map_err(|error| {
            GatewayError::with_status(
                502,
                format!(
                    "Antigravity project 发现请求失败：{}",
                    crate::server::core::egress::describe_error_detail(&error)
                ),
            )
        })?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, payload, text))
}

/// 发现 `cloudaicompanionProject`：`loadCodeAssist` → 无 project 时 `onboardUser`。
///
/// 返回 `Err` 表示**这次没拿到**（网络 / 权限 / 该账号无资格）。调用点按
/// 「best-effort 增强」处理：不影响账号可用性，只影响聊天（project 缺失时
/// 上游可能限制该请求，见 `adapter.rs`）。
pub async fn discover_project(
    access_token: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<String, GatewayError> {
    if access_token.trim().is_empty() {
        return Err(GatewayError::with_status(401, "Antigravity project 发现需要 access token"));
    }
    let url = endpoints::load_code_assist_url();
    let (status, payload, text) = post_json(
        &url,
        access_token,
        "",
        &json!({ "metadata": metadata() }),
        proxy,
    )
    .await?;
    if !(200..300).contains(&status) {
        // `x-goog-user-project` 在非 content 方法上可能触发 403（规格坑 #3）：
        // 这里 body 里没有 project，所以只可能是别的原因；如实报一句，
        // 调用点按「project 缺失」继续（聊天照发，上游可能因此限制该请求）。
        let detail = describe_body(&payload, &text);
        logging::verbose(
            "[Antigravity]",
            &format!("project 发现未成功（loadCodeAssist HTTP {status}）：{detail}"),
        );
        return Err(GatewayError::with_status(
            502,
            format!("Antigravity project 发现失败（HTTP {status}）：{detail}"),
        ));
    }
    let project = project_of(&payload);
    if !project.is_empty() {
        return Ok(project);
    }
    // 没有 project → 走 onboardUser（规格坑 #16）
    let tier = default_tier(&payload);
    logging::verbose(
        "[Antigravity]",
        &format!("loadCodeAssist 未返回 project，改走 onboardUser（tier {tier}）"),
    );
    onboard(access_token, &tier, proxy).await
}

/// `onboardUser` 一轮（`done == true` 才取 project）—— 见模块头第 2 条。
pub async fn onboard(
    access_token: &str,
    tier_id: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<String, GatewayError> {
    let tier = if tier_id.trim().is_empty() {
        DEFAULT_TIER.to_string()
    } else {
        tier_id.trim().to_string()
    };
    let body = json!({ "tierId": tier, "metadata": metadata() });
    let (status, payload, text) = post_json(
        &endpoints::onboard_user_url(),
        access_token,
        "",
        &body,
        proxy,
    )
    .await?;
    if !(200..300).contains(&status) {
        let detail = describe_body(&payload, &text);
        return Err(GatewayError::with_status(
            502,
            format!("Antigravity project 开通失败（HTTP {status}）：{detail}"),
        ));
    }
    let done = payload.get("done").and_then(Value::as_bool).unwrap_or(false);
    let project = onboard_project_of(&payload);
    if done && !project.is_empty() {
        return Ok(project);
    }
    // 未 done 或 done 但没给 project：**如实报「这次没开通」**，不编造 id
    Err(GatewayError::with_status(
        502,
        if done {
            "Antigravity project 开通响应里没有 cloudaicompanionProject（稍后刷新模型清单时会再试）"
        } else {
            "Antigravity 账号尚未完成开通（onboardUser 未返回 done，稍后刷新模型清单时会再试）"
        },
    ))
}

/// 错误文摘要（上游文案 → 截断的原文兜底）
fn describe_body(payload: &Value, text: &str) -> String {
    let message = payload
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());
    match message {
        Some(message) => truncate(message),
        None => truncate(text.trim()),
    }
}

/// 截断（按字符，UTF-8 安全）
fn truncate(text: &str) -> String {
    let collapsed: String = text.chars().filter(|character| !character.is_control()).collect();
    match collapsed.char_indices().nth(200) {
        Some((index, _)) => format!("{}…", &collapsed[..index]),
        None => collapsed,
    }
}
