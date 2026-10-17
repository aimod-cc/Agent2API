//! Antigravity 余额读数：`POST {base}:retrieveUserQuotaSummary` 的**分组额度窗口**
//! （5 小时 / 每周）+ `loadCodeAssist` 的订阅档。
//!
//! ── 上游形态（逐字；出处 Manager `modules/quota.rs::fetch_quota_summary`）──
//! ```text
//! POST {base}/v1internal:retrieveUserQuotaSummary
//!   header Authorization: Bearer {access_token}
//!          User-Agent: vscode/1.X.X (Antigravity/{ver})   ← 原生 OAuth UA
//!          Content-Type: application/json
//!          x-goog-user-project: {pid}                      ← project 非空才带
//!   body   {"project": "<pid>"}（没有 pid 时 `{}`；上游忽略它，Manager 同款）
//!   resp   { "groups": [ { "displayName": "Gemini Models",
//!                          "description": "…",
//!                          "buckets": [ { "bucketId": "gemini-5h",
//!                                         "window": "5h",
//!                                         "remainingFraction": 0.72,
//!                                         "resetTime": "<RFC3339>",
//!                                         "displayName": "…"?, "description": "…"? } ] } ] }
//! ```
//! 组名常见 `Gemini Models` / `Claude and GPT models`；`window` 实测取值 `5h` /
//! `weekly`（`bucketId` 形如 `gemini-5h` / `gemini-weekly` / `3p-5h` / `3p-weekly`）。
//! 基址三环境（sandbox → daily → prod）与目录刷新同序，处置也与
//! `models::fetch_once` 同一套（403 去头重试、429/5xx 换基址）。
//!
//! ── 与 Manager 的对照（哪几处照搬、哪几处不同）────────────────
//!   · 照搬：端点与基址顺序、请求体两形态、原生 OAuth UA、`paidTier` 优先的
//!     订阅档取值链、组名去掉结尾 ` Models` 与 `Claude and GPT` → `Claude/GPT`
//!     （Manager 前端 `AccountTable.tsx` 的 `.replace(/ models?$/i, …)`）；
//!   · 不同：Manager 把摘要并进它自己的模型配额面板（`quota_groups`），本仓把它
//!     归一成账号页的**统一余额形状**（`wallets[]` / `windows[]` / `availableView`
//!     / …，契约见 `ProviderAdapter::query_usage`）。**两组窗口都出**（Gemini 与
//!     Claude/GPT 各一份 5 小时 / 每周），余额格按 `windows[]` 一格画多条进度条 ——
//!     这正是 issue #288 要的：第三方那组（Claude 4.6 thinking 等）的额度得看得见；
//!   · 不同：Manager 对取不到的订阅档有持久化兜底（留上次的值），本仓**宁缺不编**
//!     —— 拿不到就不写 `subscription` 键。
//!
//! ── 三条关键决定（为什么这么做）──────────────────────────────
//!   1. **上游只给比例、没有绝对数** → `available` 不写、`availableView` 出
//!      「最紧窗口」的展示串。与 `zcode::monitor` 同一处置：这条通道读的是
//!      `remainingFraction`（0~1 的剩余比例），没有「还剩多少 token」这种绝对量；
//!      写一个 0 会踩到缺省的「余额不足跳过档」（阈值 1）—— 账号会被从选路里
//!      剔掉，而它额度充足。缺失 = 不知道 = 不参与判定。
//!   2. **不做模型清单过滤**：上游给几个组就出几个组的窗口。模型白名单
//!      （`models.rs` 的 [`super::models::is_offered_model`]）管的是「哪些模型
//!      能转发」，账户额度是账户的事实 —— Claude/GPT 组的窗口也如实列出来
//!      （那正是「这个账号还剩多少」的一部分）。
//!   3. **`resetTime` 是周期重置、不是「到期」**：窗口到点会自动回满，不是余额
//!      用完。因此不出 `credits`、不做 creditDetail —— 两者在界面上是两回事。
//!
//! ── 凭证链与失败分流 ────────────────────────────────────────
//! 链与 `adapter.rs::refresh_models` 逐字同形：记录 → `credentials::from_record`
//! → `oauth::account_proxy` → 临期时 `oauth::ensure_fresh(false)`（本家
//! access_token 只活一小时，余额查询也必须走这条链）。失败分流：记录点名却不存在
//! → 404；没有可用账号 → `from_record` 原文的 401；**上游 401 → 以 401 上抛**，
//! 交给 `core::usage_query` 的通用「刷新后重试一次」链路（它调本适配器的
//! `refresh_access_token` 强制刷一次、再把查询重打一遍；与其余家的 401 处置同一
//! 口径 —— 手工导入的账号可能没有 `expiresAt`，`expiring()` 判不出来就不会预刷
//! 新，这条链路是它唯一的自愈机会）；其余上游 4xx / 全环境失败 → 502。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic，取值一律走 Option 链。

use chrono::TimeZone;
use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{credentials, endpoints, models, oauth, project};

/// 查询某 Antigravity 账号的余额读数（账号级额度窗口 + 订阅档）。
pub async fn query_usage(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    let record = store.antigravity_account_record(account_id);
    if !account_id.is_empty() && record.is_none() {
        return Err(GatewayError::with_status(
            404,
            format!("Antigravity 账号 {account_id} 不存在（记录可能已被删除）"),
        ));
    }
    // account_id 为空且一条记录都没有时，这里的原文错误（401）就是调用方要的答案
    let credentials = credentials::from_record(record.as_ref())?;
    // 代理配置坏了要如实失败（不静默直连）：理由与 `refresh_models` 同一条 ——
    // 直连会拿到一张「从本机看得到」的读数，而用户以为查的是那条账号的出口
    let proxy = match record.as_ref() {
        Some(record) => oauth::account_proxy(record)?,
        // 走不到（record 为 None 时上面已经报错），为零 unwrap 的 Option 链保留
        None => None,
    };
    // 令牌：临期就刷一次（`refresh_models` 同一条链，成功会回写记录）
    let credentials = if credentials.expiring() {
        oauth::ensure_fresh(store, account_id, false).await?
    } else {
        credentials
    };

    let mut last_error = String::new();
    let mut summary: Option<Value> = None;
    for base in endpoints::V1_BASE_URLS {
        match fetch_summary_once(
            base,
            &credentials.access_token,
            &credentials.project_id,
            proxy.as_ref(),
        )
        .await
        {
            SummaryAttempt::Ok(payload) => {
                summary = Some(payload);
                break;
            }
            SummaryAttempt::Retry(reason) => {
                last_error = reason;
                logging::verbose(
                    "[Antigravity]",
                    &format!("额度摘要换下一个环境重试（{base}）：{last_error}"),
                );
            }
            SummaryAttempt::Fatal { status, detail } => {
                if status == 401 {
                    // 上游 401 = 这把 access token 被拒。这里**不自己刷新**：
                    // `core::usage_query` 的通用链路会调适配器的
                    // `refresh_access_token` 强制刷一次、再把查询重打一遍 ——
                    // 手工导入的账号可能没有 `expiresAt`，`expiring()` 判不出来就
                    // 不会预刷新，这条链路是它唯一的自愈机会（与其余家同一口径）
                    return Err(GatewayError::with_status(
                        401,
                        format!("{}（凭证可能已失效，可重试或重新登录）", failure(&detail)),
                    ));
                }
                return Err(GatewayError::with_status(502, failure(&detail)));
            }
        }
    }
    let Some(summary_payload) = summary else {
        let detail = if last_error.is_empty() {
            "所有环境均不可用".to_string()
        } else {
            last_error
        };
        return Err(GatewayError::with_status(502, failure(&detail)));
    };

    let (wallets, windows, available_view) = wallets_of(&summary_payload);

    // ── 订阅档（best-effort，见模块头：失败就不写这个键）────────────
    let mut raw = Map::new();
    raw.insert("quotaSummary".to_string(), summary_payload);
    let mut subscription = Map::new();
    match project::fetch_tier(&credentials.access_token, proxy.as_ref()).await {
        Ok(tier) => {
            // 上游原文照抄进 raw（排障用），归一化只写进 subscription
            raw.insert("tier".to_string(), Value::String(tier.clone()));
            let plan = if tier.trim().is_empty() {
                // 两个 tier 都是 null —— Manager 对这类账号（地理受限 / 受限
                // 账号）的权威判法是 FREE（`modules/quota.rs` 的注释原话：
                // 「权威判为 FREE，绝不误判为 PRO」）
                "Free".to_string()
            } else {
                normalize_tier(&tier)
            };
            subscription.insert("planName".to_string(), Value::String(plan));
        }
        Err(error) => {
            logging::verbose(
                "[Antigravity]",
                &format!("订阅档未读到（余额部分照常返回）：{}", error.message),
            );
        }
    }

    let mut document = json!({
        // `available` **不写**（见模块头第 1 条：写了 0 会让余额跳过档误剔账号）
        "availableView": available_view,
        // 读数与进度条的量纲都是百分比
        "unit": "%",
        "wallets": wallets,
        // 逐窗口读数（一个组 × 一个窗口一项）：余额格按它画**多条**进度条
        // —— 只画最紧的那一条会把另一个模型组盖掉（见 `wallets_of` 的文档）
        "windows": windows,
        "raw": Value::Object(raw),
    });
    if !subscription.is_empty() {
        if let Some(object) = document.as_object_mut() {
            object.insert("subscription".to_string(), Value::Object(subscription));
        }
    }
    Ok(document)
}

/// 一次额度摘要请求的中间结果（供环境轮询判断要不要换下一条基址）
enum SummaryAttempt {
    /// 拿到了摘要
    Ok(Value),
    /// 这次不成（原因），可以换下一条基址
    Retry(String),
    /// 这次不成，且换域名也没用（4xx 之外的确定失败）。
    /// 带上游状态码：`query_usage` 据此把 401 翻成 401（见 [`classify`]）
    Fatal { status: u16, detail: String },
}

/// 打一次额度摘要（单条基址）。
///
/// 处置与 `models::fetch_once` 同一套（同一台主机、同一组头，请求口径复用
/// `models::send_json`）：传输失败 / 429 / 5xx 换下一条基址；403 且带了
/// `x-goog-user-project` 去掉该头重试一次（规格坑 #3）；其余 4xx 是请求本身的
/// 问题，换域名白撞。
async fn fetch_summary_once(
    base: &str,
    access_token: &str,
    project_id: &str,
    proxy: Option<&ResolvedProxy>,
) -> SummaryAttempt {
    let url = endpoints::retrieve_user_quota_summary_url(base);
    let body = if project_id.trim().is_empty() {
        json!({})
    } else {
        json!({ "project": project_id.trim() })
    };
    let response = match models::send_json(&url, access_token, project_id, &body, proxy).await {
        Ok(response) => response,
        Err(reason) => return SummaryAttempt::Retry(reason),
    };
    if response.status == 403 && !project_id.trim().is_empty() {
        // 规格坑 #3：403 + 带了 `x-goog-user-project` → 去掉该头重试一次
        // （body 不变：上游对这类方法忽略请求体里的 project 字段）
        return match models::send_json(&url, access_token, "", &body, proxy).await {
            Ok(retried) => classify(retried),
            Err(reason) => SummaryAttempt::Retry(reason),
        };
    }
    classify(response)
}

/// 一次响应的成败处置（与 `models::fetch_once` 同一套口径）。
///
/// 这里的文案**不带功能前缀**（`"额度接口返回 HTTP …"`，与 `models.rs` 的
/// `"目录接口返回 HTTP …"` 同形态）—— 前缀只在出口拼一次（见 [`failure`]），
/// 两处都拼会变成「Antigravity 额度查询失败：Antigravity 额度查询失败（HTTP …）」。
fn classify(response: models::HttpOutcome) -> SummaryAttempt {
    if (200..300).contains(&response.status) {
        return SummaryAttempt::Ok(response.payload);
    }
    let detail = upstream_message(&response.payload);
    let summary = if detail.is_empty() {
        format!("额度接口返回 HTTP {}", response.status)
    } else {
        format!("额度接口返回 HTTP {}：{detail}", response.status)
    };
    // 429 / 5xx 换下一条基址；其余 4xx 是请求本身的问题，换域名白撞
    // （401 由 `query_usage` 翻成 401，走通用的「刷新后重试一次」链路）
    if response.status == 429 || response.status >= 500 {
        SummaryAttempt::Retry(summary)
    } else {
        SummaryAttempt::Fatal {
            status: response.status,
            detail: summary,
        }
    }
}

/// 出口统一文案：功能前缀**只在这里拼一次**（`classify` 的短文案不带前缀，
/// 理由见 [`classify`]）—— `query_usage` 的每一处出口（Fatal 直出 / 上游 401 /
/// 全环境耗尽）都走它。
fn failure(detail: &str) -> String {
    format!("Antigravity 额度查询失败：{detail}")
}

/// 上游错误文案（`error.message` / `message` 兜底；与 `models.rs` 的同名私有
/// 函数同口径，那边是私有实现，不跨模块借）
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

/// 摘要响应 → 钱包行 + **逐窗口读数** + 表头读数。
///
/// 三份产物同源同序（上游组顺序 × 组内 bucket 顺序，**不排序**：窗口的先后本身就是
/// 上游的表述，界面上「5 小时 / 每周」按上游给的顺序读起来最自然）：
///   · `wallets[]` —— 账号级明细行（悬停提示逐行列，展示串带重置时刻与中文全名）；
///   · `windows[]` —— 逐窗口读数（余额格按它**一格多条进度条**，与 `custom::balance`
///     里 OpenCode Go 的同名形状逐键对齐）。本家一次给到**两个模型组 × 两个窗口**
///     （Gemini 与 Claude/GPT 各一份 5 小时 / 每周）；只画「最紧的那一条」会把
///     「另一个组还有多少」盖掉，而第三方那组（Claude/GPT）正是 issue #288 要看的。
///   · `available_view` —— 表头读数，读「最紧的那个窗口」（并列取先出现的）。
///
/// 每个 `remainingFraction` 为有限数字的 bucket 一行 / 一项，其余跳过（没有比例
/// 就没有读数，不拿 0 冒充 —— 见模块头第 1 条）。
///
/// 摘要 200 但 `groups` 缺失 / 空 / 所有 bucket 都没有比例，三份产物都为空
/// （`wallets: []` / `windows: []` + 「无额度窗口」），不是错误。
fn wallets_of(payload: &Value) -> (Vec<Value>, Vec<Value>, String) {
    let mut wallets: Vec<Value> = Vec::new();
    let mut windows: Vec<Value> = Vec::new();
    // (剩余百分比, 展示名) —— 表头读「最紧的那个窗口」，并列取先出现的
    let mut tightest: Option<(f64, String)> = None;
    if let Some(groups) = payload.get("groups").and_then(Value::as_array) {
        for group in groups {
            let group_name = group_name_of(
                group
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            );
            let Some(buckets) = group.get("buckets").and_then(Value::as_array) else {
                continue;
            };
            for bucket in buckets {
                let Some(fraction) = bucket.get("remainingFraction").and_then(Value::as_f64) else {
                    continue;
                };
                if !fraction.is_finite() {
                    continue;
                }
                // 原始数值，不取整不夹取（进度条由前端自己夹，见 `usagePool` 的口径）；
                // 乘 100 之后仍要有限 —— 否则 `remainingPercent` 会落成 null，
                // 与「这一行有比例」的前提矛盾
                let percent = fraction * 100.0;
                if !percent.is_finite() {
                    continue;
                }
                let window = window_name_of(bucket);
                let display_name = if group_name.is_empty() {
                    window.clone()
                } else {
                    format!("{group_name} · {window}")
                };
                let resets_at = reset_millis(bucket.get("resetTime").and_then(Value::as_str));
                let reading = format!("剩 {}%", percent_text(percent));
                // ── 逐窗口项（余额格那条进度条一行的读数）──────────────
                // `view` 只放**读数本身**、重置时刻另走 `resetsAt`：短格子里
                // 「剩 72% · 10-11 03:00 重置」会把读数挤没，而悬停提示由前端
                // 用 resetsAt 现拼（与 OpenCode Go 那份同款：那边的 view 也不带时刻）。
                // `group` / `window` 是**给前端拼紧凑标签的两段料**（`Gemini` + `5h`）：
                // 组名是专名原样透出，窗口码按界面语言译 —— 后端只给码、不给译好的
                // 中文短名，否则英文界面里会冒出一串汉字（与 `zcode::plan_state`
                // 「状态键由后端给、文案只在前端拼」同一条纪律）
                let mut row = Map::new();
                row.insert("key".to_string(), Value::String(bucket_key(bucket)));
                row.insert("label".to_string(), Value::String(display_name.clone()));
                row.insert("group".to_string(), Value::String(group_name.clone()));
                row.insert("window".to_string(), Value::String(window_code_of(bucket)));
                row.insert("remainingPercent".to_string(), json!(percent));
                row.insert("view".to_string(), Value::String(reading.clone()));
                if let Some(at) = resets_at {
                    row.insert("resetsAt".to_string(), Value::from(at));
                }
                windows.push(Value::Object(row));

                // ── 明细行（悬停提示）：展示串带上重置时刻 ─────────────
                let mut view = reading;
                if let Some(text) = reset_text(bucket.get("resetTime").and_then(Value::as_str)) {
                    view.push_str(" · ");
                    view.push_str(&text);
                    view.push_str(" 重置");
                }
                if tightest
                    .as_ref()
                    .map(|(current, _)| percent < *current)
                    .unwrap_or(true)
                {
                    tightest = Some((percent, display_name.clone()));
                }
                wallets.push(json!({
                    "type": "quota_window",
                    "displayName": display_name,
                    "balance": Value::Null,
                    "balanceView": view,
                    "remainingPercent": percent,
                }));
            }
        }
    }
    let available_view = tightest
        .map(|(percent, name)| format!("{name} 剩 {}%", percent_text(percent)))
        .unwrap_or_else(|| "无额度窗口".to_string());
    (wallets, windows, available_view)
}

/// 窗口的**码**（`5h` / `weekly` / …）：给前端拼紧凑标签用的那半段 ——
/// 专名（组名）原样透出，窗口这一段必须有码可译，否则英文界面里会冒出中文。
///
/// 与 [`window_name_of`] 同一套判据（含 `week` / `7d` → 每周；含 `5h` / `hour`
/// → 5 小时），只是这里给的是码不是中文名；认不出的原样透出（不编码）。
fn window_code_of(bucket: &Value) -> String {
    let raw = bucket
        .get("window")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            bucket
                .get("bucketId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
        .unwrap_or("");
    let lowered = raw.to_ascii_lowercase();
    if lowered.contains("week") || lowered.contains("7d") {
        return "weekly".to_string();
    }
    if lowered.contains("5h") || lowered.contains("hour") {
        return "5h".to_string();
    }
    if lowered.contains("month") {
        return "monthly".to_string();
    }
    if lowered.contains("day") || lowered.contains("24h") {
        return "daily".to_string();
    }
    raw.to_string()
}

/// 窗口的稳定标识：上游 `bucketId`（`gemini-5h` / `3p-weekly`）优先 ——
/// 它是上游对窗口的正式命名，前端拿它作 React 键与去重依据。
/// 缺失时退回展示名，至少不落一个空键（宁可用一个丑键，也不要空串）。
fn bucket_key(bucket: &Value) -> String {
    bucket
        .get("bucketId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| window_name_of(bucket))
}

/// 组名：去掉结尾的 ` Models` / ` Model`（大小写不敏感；Manager 前端
/// `AccountTable.tsx` 的 `.replace(/ models?$/i, …)` 同口径），再把同时写着
/// Claude 与 GPT 的组写成 `Claude/GPT`（同文件的 `.replace(/Claude and GPT/i, …)`）。
/// 空名返回空串 —— 调用方据此不拼组名前缀。
fn group_name_of(raw: &str) -> String {
    let trimmed = raw.trim();
    let lowered = trimmed.to_ascii_lowercase();
    let mut stripped = trimmed;
    for suffix in [" models", " model"] {
        if lowered.ends_with(suffix) {
            // suffix 全是 ASCII 且逐字节相等：减掉它的字节数一定落在字符边界上
            stripped = trimmed
                .get(..trimmed.len() - suffix.len())
                .unwrap_or(trimmed)
                .trim_end();
            break;
        }
    }
    if !stripped.is_empty() {
        let lowered = stripped.to_ascii_lowercase();
        if lowered.contains("claude") && lowered.contains("gpt") {
            return "Claude/GPT".to_string();
        }
    }
    stripped.to_string()
}

/// 窗口名：`displayName` 优先；否则由 `window`（缺则 `bucketId`）判 —— 含
/// `week` / `7d` → 「每周」，含 `5h` / `hour` → 「5 小时」（这两个窗口在界面上就
/// 叫这个名字，见 `zcode::monitor` 的措辞口径），认不出的原样透出；都空 →
/// 「额度」。
fn window_name_of(bucket: &Value) -> String {
    if let Some(text) = bucket
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return text.to_string();
    }
    let raw = bucket
        .get("window")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            bucket
                .get("bucketId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
        .unwrap_or("");
    let lowered = raw.to_ascii_lowercase();
    if lowered.contains("week") || lowered.contains("7d") {
        return "每周".to_string();
    }
    if lowered.contains("5h") || lowered.contains("hour") {
        return "5 小时".to_string();
    }
    if raw.is_empty() {
        "额度".to_string()
    } else {
        raw.to_string()
    }
}

/// 重置时刻：`resetTime`（RFC3339）→ 本机时区 `MM-DD HH:MM`。
///
/// 与 `zcode::monitor::reset_text` 同一口径（那边收 epoch 毫秒，这里先把 RFC3339
/// 解成毫秒）；解不出 / 时区换算越界时返回 None —— 宁可不写这一段，也不编一个
/// 时刻。
fn reset_text(raw: Option<&str>) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(raw?.trim()).ok()?;
    chrono::Local
        .timestamp_millis_opt(at.timestamp_millis())
        .single()
        .map(|local| local.format("%m-%d %H:%M").to_string())
}

/// 重置时刻 → Unix 毫秒（同一份输入、另一种出口：展示串给 `wallets` 的悬停，
/// 毫秒给 `windows` 的行 —— 前端自己按界面语言拼「什么时候重置」）。
fn reset_millis(raw: Option<&str>) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw?.trim())
        .ok()
        .map(|at| at.timestamp_millis())
}

/// 百分比 → 紧凑串（`72` / `72.5`；一位小数，照抄 `zcode::monitor::percent_text`）
fn percent_text(value: f64) -> String {
    format!("{value:.1}").trim_end_matches(".0").to_string()
}

/// 订阅档归一化：Manager `models/quota.rs::normalize_subscription_tier` 的三档，
/// 展示用**首字母大写**（本仓口径；前端 `isFreePlan` 按 `/^free$/i` 匹配，
/// 「免费版」徽标挂得上）。
///
/// 顺序照 Manager：含 `ultra` / `helium`（Ultra 的内部代号）→ Ultra；含 `free` /
/// `starter` → Free；含 `pro` / `premium` / `advanced` → Pro；认不出的**原样
/// 透出**（不猜）。
fn normalize_tier(raw: &str) -> String {
    let trimmed = raw.trim();
    let lowered = trimmed.to_ascii_lowercase();
    if lowered.contains("ultra") || lowered.contains("helium") {
        return "Ultra".to_string();
    }
    if lowered.contains("free") || lowered.contains("starter") {
        return "Free".to_string();
    }
    if lowered.contains("pro") || lowered.contains("premium") || lowered.contains("advanced") {
        return "Pro".to_string();
    }
    trimmed.to_string()
}
