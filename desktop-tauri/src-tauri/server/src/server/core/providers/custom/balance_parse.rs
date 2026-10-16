//! 七种余额查询方案的**响应解析**（纯函数：一份上游 JSON → 一份归一化读数）。
//!
//! 与 `balance.rs` 分工：那边负责「认证、打哪个地址、拿到什么」，这边只做
//! 「拿到的 JSON 怎么读成余额」。这样每家的字段兼容（同一接口在不同版本 /
//! 不同定制下字段名漂过）都集中在一处，查问题不必在请求链路里翻。
//!
//! 移植来源：OmniProxy `server/src/utils/providerBalance.ts` 的
//! `parseSub2api` / `parseNewApiAccount` / `parseNewApiDisplay` / `parseDeepSeek`
//! / `parseOpenCode` / `parseSensenova` / `parseZhipu`，逐字段对读。
//!
//! ── 归一化形状（`ProviderAdapter::query_usage` 的契约）────────
//! ```jsonc
//! {
//!   "available": 62.5,          // 主读数（数字；判不出给 null）
//!   "unit": "%",                // 展示单位
//!   "wallets": [                // 明细行（工具提示里逐行列出）
//!     { "type": "rolling", "displayName": "5 小时", "balance": 62.5, "unit": "%" }
//!   ],
//!   "windows": [                // 逐窗口读数（可选键，现役只有 OpenCode）：一窗一项，
//!     { "key": "rolling", "label": "5 小时",   // 界面据此一格画三条进度条
//!       "remainingPercent": 62.5, "usedPercent": 37.5,
//!       "view": "剩 62.5%", "resetsAt": 1789… }
//!   ],
//!   "subscription": null,       // 这七种方案都没有套餐概念
//!   "raw": { }                  // 上游原始响应（排障用；界面默认不展示）
//! }
//! ```
//! 「缺键不写、null 不是 0」是全仓余额形状的纪律（见 `accounts-credit-detail`
//! 的说明）：读不到的字段给 `null` 而不是 0 —— 界面据此显示「—」而不是
//! 「只剩 0」。因此本文件的 `wallets` 行只在**读到数值**时才产出。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap / expect / panic。

use chrono::TimeZone;
use serde_json::{json, Map, Value};

/// 取对象视图（数组 / 标量给 None）
pub(super) fn map_of(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

/// 取数字：认数字与「数字字符串」（上游常把金额写成字符串）
pub(super) fn number_of(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64().filter(|value| value.is_finite()),
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite()),
        _ => None,
    }
}

/// 取对象里某个键的数字（`number_of(map.get(key))` 的简写）
pub(super) fn number_at(map: &Map<String, Value>, key: &str) -> Option<f64> {
    number_of(map.get(key))
}

/// 秒级 Unix 时间戳（容错毫秒值）→ 毫秒；非法 / 缺失给 None
pub(super) fn unix_to_ms(value: Option<&Value>) -> Option<i64> {
    let number = number_of(value)?;
    if number <= 0.0 {
        return None;
    }
    Some(if number < 1e12 {
        (number * 1000.0).round() as i64
    } else {
        number.round() as i64
    })
}

/// 文案截断（上游错误说明可能是一整页 HTML；给工具提示用）
pub(super) fn shorten(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(max.saturating_sub(3)).collect();
    format!("{head}...")
}

/// 一行明细（`balance` 为 None 时不产出 —— 见模块头的「缺键不写」）
pub(super) fn wallet(kind: &str, display_name: &str, balance: Option<f64>, unit: &str) -> Option<Value> {
    let balance = balance?;
    Some(json!({
        "type": kind,
        "displayName": display_name,
        "balance": balance,
        "unit": unit,
    }))
}

/// 归一化读数的公共骨架（各家只填自己读到的部分）
pub(super) fn document(
    available: Option<f64>,
    unit: &str,
    wallets: Vec<Value>,
    raw: &Value,
) -> Value {
    json!({
        "available": available,
        "unit": unit,
        "wallets": wallets,
        "subscription": Value::Null,
        "raw": raw,
    })
}

/// Sub2API `/v1/usage`：`{ quota: {limit, used}, remaining|balance, unit }`。
///
/// 三个读数都可以缺（不同版本的 sub2api 给的不一样），只要读到一个就成。
pub(super) fn parse_sub2api(payload: &Value) -> Option<Value> {
    let data = map_of(payload)?;
    let quota = data.get("quota").and_then(map_of);
    let total = quota.and_then(|map| number_at(map, "limit"));
    let used = quota.and_then(|map| number_at(map, "used"));
    let remaining = number_at(data, "remaining")
        .or_else(|| number_at(data, "balance"))
        .or_else(|| quota.and_then(|map| number_at(map, "remaining")))
        .or_else(|| match (total, used) {
            (Some(total), Some(used)) => Some(total - used),
            _ => None,
        });
    if remaining.is_none() && total.is_none() && used.is_none() {
        return None;
    }
    let unit = data
        .get("unit")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("USD")
        .to_string();
    let wallets = vec![
        wallet("remaining", "剩余额度", remaining, &unit),
        wallet("used", "已用额度", used, &unit),
        wallet("total", "总额度", total, &unit),
    ]
    .into_iter()
    .flatten()
    .collect();
    Some(document(remaining, &unit, wallets, payload))
}

/// DeepSeek `/user/balance`：`{ balance_infos: [{currency, total_balance, …}] }`。
///
/// 主读数取**第一条**币种（源实现同口径：上游按账户结算币种优先返回）。
pub(super) fn parse_deepseek(payload: &Value) -> Option<Value> {
    let data = map_of(payload)?;
    let infos = data.get("balance_infos")?.as_array()?;
    let mut wallets: Vec<Value> = Vec::new();
    let mut primary: Option<(f64, String)> = None;
    for item in infos {
        let Some(info) = map_of(item) else { continue };
        let Some(amount) = number_at(info, "total_balance") else { continue };
        let currency = info
            .get("currency")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("CNY")
            .to_string();
        if primary.is_none() {
            primary = Some((amount, currency.clone()));
        }
        if let Some(row) = wallet("total_balance", &currency, Some(amount), &currency) {
            wallets.push(row);
        }
        if let Some(row) = wallet("granted_balance", &format!("{currency} · 赠送余额"), number_at(info, "granted_balance"), &currency) {
            wallets.push(row);
        }
        if let Some(row) = wallet("topped_up_balance", &format!("{currency} · 充值余额"), number_at(info, "topped_up_balance"), &currency) {
            wallets.push(row);
        }
    }
    let (available, unit) = match primary {
        Some((amount, currency)) => (Some(amount), currency),
        None => (None, "CNY".to_string()),
    };
    if available.is_none() {
        return None;
    }
    Some(document(available, &unit, wallets, payload))
}

/// 单个用量窗口的读法（字段名在 OpenCode 的不同实现里漂过，逐个兼容）
struct UsageWindow {
    remaining_percent: Option<f64>,
    used_percent: Option<f64>,
    resets_at_ms: Option<i64>,
}

fn usage_window(window: &Map<String, Value>) -> UsageWindow {
    let percent = number_at(window, "percent")
        .or_else(|| number_at(window, "usage_percent"))
        .or_else(|| number_at(window, "percent_used"));
    let used = number_at(window, "used")
        .or_else(|| number_at(window, "used_quota"))
        .or_else(|| number_at(window, "usage"))
        .or_else(|| number_at(window, "amount_used"));
    let limit = number_at(window, "limit")
        .or_else(|| number_at(window, "total"))
        .or_else(|| number_at(window, "quota"))
        .or_else(|| number_at(window, "allowance"))
        .or_else(|| number_at(window, "total_quota"));
    let remaining = number_at(window, "remaining")
        .or_else(|| number_at(window, "remaining_quota"))
        .or_else(|| number_at(window, "left"));
    // 百分比优先；只有 used/limit（或 remaining/limit）时按比例换算
    let used_percent = percent.or_else(|| match (used, limit) {
        (Some(used), Some(limit)) if limit > 0.0 => Some(used / limit * 100.0),
        _ => match (remaining, limit) {
            (Some(remaining), Some(limit)) if limit > 0.0 => {
                Some(((limit - remaining) / limit * 100.0).max(0.0))
            }
            _ => None,
        },
    });
    let resets_at_ms = [
        "resetsAt",
        "resets_at",
        "reset_at",
        "resetAt",
        "expires_at",
    ]
    .iter()
    .find_map(|key| window.get(*key))
    .and_then(|value| match value {
        // 字符串形态可能是 RFC 3339，也可能是纯数字（秒 / 毫秒），两种都收
        Value::String(text) => {
            let trimmed = text.trim();
            match trimmed.parse::<f64>() {
                Ok(number) => Some(number),
                Err(_) => rfc3339_to_ms(trimmed).map(|ms| ms as f64 / 1000.0),
            }
        }
        other => number_of(Some(other)),
    })
    .and_then(|number| unix_to_ms(Some(&json!(number))));
    UsageWindow { remaining_percent: used_percent.map(|used| (100.0 - used).clamp(0.0, 100.0)), used_percent, resets_at_ms }
}

/// ISO 8601（RFC 3339）→ 毫秒时间戳；认不出给 None。
///
/// 上游给的是带偏移量的标准形态（`2026-10-15T12:34:56.789Z` / `+08:00`），
/// 直接交给 chrono —— 手写一套「闰年 + 时区」的解析只会多一处出错的地方。
fn rfc3339_to_ms(text: &str) -> Option<i64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(text.trim()).ok()?;
    Some(parsed.timestamp_millis())
}

/// OpenCode `/v1/usage`：`{ usage: { rolling: {…}, weekly: {…}, monthly: {…} } }`。
///
/// 主读数是**滚动窗口（5 小时）的剩余百分比**（源实现同口径：`100 - 已用%`）；
/// 周 / 月窗口耗尽（已用 100%）时主读数记 0 —— 那是「彻底没额度」，限流判定
/// 要按它跳过（见 OmniProxy `providerBalanceRule` 的 `opencodeRemainingPercent`）。
///
/// ── 三个窗口各自成条（`windows[]`）───────────────────────────
/// 这一家的额度不是「一个池子剩多少」而是**三个窗口各自的剩余比例**：一个数字
/// 说不清「5 小时还剩着、每周已经用光」这种状态，而它恰恰是限流判定最要紧的
/// 一档（周 / 月耗尽即彻底没额度）。因此除主读数外再给一份 `windows[]`：
/// 一窗一项，带短标签、剩余比例、已用比例、重置时刻与展示串，界面据此
/// **一格画三条**。这是形状能力（有 `windows` 就逐条画）而不是 provider 判据 ——
/// 与 `wallets` / `credits` 同一路数（见前端 `usageWindows`）。
///
/// 展示串两处共用同一份口径：格子里是**前半句**（「剩 62.5%」），重置时刻跟在
/// 后面（悬停提示读整串），分隔符「 · 」与 `zcode::monitor` 的 `wallet_view` 同一个。
pub(super) fn parse_opencode(payload: &Value) -> Option<Value> {
    let data = map_of(payload)?;
    let usage = data.get("usage").and_then(map_of)?;
    let mut reads: Vec<(&str, &str, UsageWindow)> = Vec::new();
    for (key, label) in [
        ("rolling", "5 小时"),
        ("weekly", "每周"),
        ("monthly", "每月"),
    ] {
        let Some(window) = usage.get(key).and_then(map_of) else { continue };
        reads.push((key, label, usage_window(window)));
    }
    if reads.is_empty() {
        return None;
    }
    let exhausted = reads
        .iter()
        .any(|(key, _, window)| (*key == "weekly" || *key == "monthly") && window.used_percent.map(|used| used >= 100.0) == Some(true));
    let rolling = reads.iter().find(|(key, _, _)| *key == "rolling").map(|(_, _, window)| window);
    let primary = rolling.or_else(|| reads.first().map(|(_, _, window)| window));
    let available = if exhausted {
        Some(0.0)
    } else {
        primary.and_then(|window| window.remaining_percent)
    };
    let mut wallets: Vec<Value> = Vec::new();
    let mut windows: Vec<Value> = Vec::new();
    for (key, label, window) in &reads {
        // 窗口在、数读不到（上游换了字段名）：这一条照出，但不编数字 ——
        // 格子里是「—」，与「剩 0%」是两件事（见模块头的「null 不是 0」）
        let mut row = Map::new();
        row.insert("key".to_string(), Value::String((*key).to_string()));
        row.insert("label".to_string(), Value::String((*label).to_string()));
        if let Some(remaining) = window.remaining_percent {
            row.insert("remainingPercent".to_string(), json!(remaining));
            row.insert("view".to_string(), Value::String(format!("剩 {}%", percent_text(remaining))));
        }
        if let Some(used) = window.used_percent {
            row.insert("usedPercent".to_string(), json!(used));
        }
        if let Some(resets) = window.resets_at_ms {
            row.insert("resetsAt".to_string(), Value::from(resets));
        }
        windows.push(Value::Object(row));
        // 明细行只在**读到数值**时才产出（「缺键不写」），展示串带上重置时刻：
        // 这一家的窗口都是「到点回满」，剩余多少与什么时候回满是同一个问题的两面
        let Some(remaining) = window.remaining_percent else { continue };
        let Some(mut row) = wallet(key, label, Some(remaining), "%") else { continue };
        let mut parts = vec![format!("剩 {}%", percent_text(remaining))];
        if let Some(used) = window.used_percent {
            parts.push(format!("已用 {}%", percent_text(used)));
        }
        if let Some(text) = reset_text(window.resets_at_ms) {
            parts.push(format!("{text} 重置"));
        }
        if let Some(object) = row.as_object_mut() {
            object.insert("balanceView".to_string(), Value::String(parts.join(" · ")));
            if let Some(resets) = window.resets_at_ms {
                object.insert("resetsAt".to_string(), Value::from(resets));
            }
        }
        wallets.push(row);
    }
    let mut document = document(available, "%", wallets, payload);
    if let Some(object) = document.as_object_mut() {
        object.insert("windows".to_string(), Value::Array(windows));
    }
    Some(document)
}

/// 百分比读数的展示串（1 位小数、去掉多余的 `.0`；与 `zcode::monitor::percent_text`
/// 同一口径 —— 两个模块各留一份，比为一句话把对方的私有函数升成 crate 可见更省事）
fn percent_text(value: f64) -> String {
    format!("{value:.1}").trim_end_matches(".0").to_string()
}

/// 重置时刻 → 本地时区 `MM-DD HH:MM`（与 `zcode::monitor::reset_text` 同口径：
/// 上游给的是绝对时刻，按操作者本机时区渲染；时间戳越界给 None ——
/// 宁可不写这一段，也不编一个时刻）
fn reset_text(ms: Option<i64>) -> Option<String> {
    let ms = ms.filter(|value| *value > 0)?;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|at| at.format("%m-%d %H:%M").to_string())
}

/// NewAPI `/api/status` 里的配额显示口径（`data.quota_per_unit` 等）。
pub(super) struct QuotaDisplay {
    pub display_type: String,
    pub quota_per_unit: f64,
    pub usd_exchange_rate: f64,
    pub custom_currency_symbol: String,
    pub custom_currency_exchange_rate: f64,
}

/// 解析 `/api/status` 的显示口径（读不出 / 汇率缺失时给 None —— 与源实现
/// 同口径：**宁可退回原始配额，也不用默认汇率算出一个错的金额**）。
pub(super) fn new_api_display(payload: &Value) -> Option<QuotaDisplay> {
    let data = payload.get("data").and_then(map_of)?;
    let quota_per_unit = number_at(data, "quota_per_unit").filter(|value| *value > 0.0)?;
    let mut display_type = data
        .get("quota_display_type")
        .and_then(Value::as_str)
        .map(|text| text.trim().to_ascii_uppercase())
        .unwrap_or_default();
    if !["USD", "CNY", "TOKENS", "CUSTOM"].contains(&display_type.as_str()) {
        display_type = if data.get("display_in_currency").and_then(Value::as_bool) == Some(false) {
            "TOKENS".to_string()
        } else {
            "USD".to_string()
        };
    }
    let usd_exchange_rate = number_at(data, "usd_exchange_rate").unwrap_or(1.0);
    let custom_currency_exchange_rate = number_at(data, "custom_currency_exchange_rate").unwrap_or(1.0);
    if display_type == "CNY" && !(usd_exchange_rate > 0.0) {
        return None;
    }
    if display_type == "CUSTOM" && !(custom_currency_exchange_rate > 0.0) {
        return None;
    }
    Some(QuotaDisplay {
        display_type,
        quota_per_unit,
        usd_exchange_rate,
        custom_currency_symbol: data
            .get("custom_currency_symbol")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string(),
        custom_currency_exchange_rate,
    })
}

/// 原始配额 → 展示口径（源实现 `displayedAmountRemaining` 的同一条换算链）
pub(super) fn new_api_display_amount(quota: f64, display: Option<&QuotaDisplay>) -> (f64, String) {
    let Some(display) = display else { return (quota, "quota".to_string()) };
    if display.quota_per_unit <= 0.0 {
        return (quota, "quota".to_string());
    }
    let amount_usd = quota / display.quota_per_unit;
    match display.display_type.as_str() {
        "CNY" if display.usd_exchange_rate > 0.0 => (amount_usd * display.usd_exchange_rate, "CNY".to_string()),
        "CUSTOM" if display.custom_currency_exchange_rate > 0.0 => (
            amount_usd * display.custom_currency_exchange_rate,
            if display.custom_currency_symbol.is_empty() {
                "CUSTOM".to_string()
            } else {
                display.custom_currency_symbol.clone()
            },
        ),
        "TOKENS" => (quota, "quota".to_string()),
        // USD 与认不出的取值（后者已被 new_api_display 归一成 USD/TOKENS）
        _ => (amount_usd, "USD".to_string()),
    }
}

/// NewAPI `/api/user/self`：`data.quota`（剩余配额）/ `data.used_quota`（已用）。
pub(super) fn parse_new_api_account(payload: &Value) -> Option<(f64, Option<f64>)> {
    let data = payload.get("data").and_then(map_of)?;
    let remaining = number_at(data, "quota")?;
    Some((remaining, number_at(data, "used_quota")))
}

/// NewAPI 整套读数组装：主读数按展示口径换算（`/api/status` 没读到就用原始配额）。
pub(super) fn new_api_document(
    remaining_raw: f64,
    used_raw: Option<f64>,
    display: Option<&QuotaDisplay>,
    account_payload: &Value,
    status_payload: Option<&Value>,
) -> Value {
    let (remaining, unit) = new_api_display_amount(remaining_raw, display);
    let used = used_raw.map(|used| new_api_display_amount(used, display).0);
    let wallets = vec![
        wallet("remaining", "剩余配额", Some(remaining), &unit),
        wallet("used", "已用配额", used, &unit),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut document = document(Some(remaining), &unit, wallets, account_payload);
    if let Some(object) = document.as_object_mut() {
        // 状态接口读到了什么、原始配额是多少：排障时这两样最有用
        object.insert("quota".to_string(), json!(remaining_raw));
        if let Some(used_raw) = used_raw {
            object.insert("usedQuota".to_string(), json!(used_raw));
        }
        if let Some(status) = status_payload {
            object.insert("status".to_string(), status.clone());
        }
    }
    document
}

/// 智谱 `query-customer-account-report`（两站同一条接口、同一个信封）：
/// `{code, msg, data: {balance, rechargeAmount, giveAmount, totalSpendAmount, …}}`。
///
/// 金额单位随站点（国内站人民币元 / 国际站美元），由调用方传入。
/// `code != 200` 是失败：鉴权失败一族（401 令牌失效；1000 签名校验不过；
/// 1001 没带 Authorization —— 后两个是实测值，两站一致）归进「凭证失效」
/// 一档（文案要能告诉用户去换哪份凭证，两站的凭证不同：国内站控制台令牌、
/// 国际站 API Key 签名），其余原样带上上游消息。
pub(super) fn parse_zhipu(
    payload: &Value,
    unit: &str,
    invalid_credential: &str,
) -> Result<Value, String> {
    let body = map_of(payload).ok_or_else(|| "智谱余额响应不是 JSON 对象".to_string())?;
    let code = number_at(body, "code");
    if code != Some(200.0) {
        let message = body
            .get("msg")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .map(|text| shorten(text, 120))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "未知错误".to_string());
        return Err(matches!(code, Some(401.0) | Some(1000.0) | Some(1001.0))
            .then(|| format!("{invalid_credential}：{message}"))
            .unwrap_or_else(|| {
                format!(
                    "智谱余额接口返回错误（code={}）：{message}",
                    code.map(|value| value.to_string()).unwrap_or_else(|| "?".to_string())
                )
            }));
    }
    let data = body.get("data").and_then(map_of);
    let balance = data.and_then(|map| number_at(map, "balance"))
        .or_else(|| data.and_then(|map| number_at(map, "availableBalance")));
    let Some(balance) = balance else {
        return Err("智谱余额响应里没有余额字段".to_string());
    };
    let wallets = data
        .map(|map| {
            vec![
                wallet("rechargeAmount", "充值余额", number_at(map, "rechargeAmount"), unit),
                wallet("giveAmount", "赠送余额", number_at(map, "giveAmount"), unit),
                wallet("frozenBalance", "冻结金额", number_at(map, "frozenBalance"), unit),
            ]
            .into_iter()
            .flatten()
            .collect()
        })
        .unwrap_or_default();
    let mut document = document(Some(balance), unit, wallets, payload);
    if let Some(object) = document.as_object_mut() {
        if let Some(spent) = data.and_then(|map| number_at(map, "totalSpendAmount")) {
            object.insert("totalSpendAmount".to_string(), json!(spent));
        }
    }
    Ok(document)
}

/// 商汤日日新 `tokenplan/pool-usage`：
/// `{ plan, pools: [{ id, name, pool_type, model_ids, window_5h, window_7d, grant_balance }] }`。
///
/// 主读数是**通用池（`pool_type = default`）5 小时窗口的剩余百分比**（缺失时
/// 退 7 天窗口）—— 与 OmniProxy 的自动跳过判定同一口径。
/// 明细行按池 × 窗口展开（两个池各两层窗口），返赠余额单独一行。
pub(super) fn parse_sensenova(payload: &Value) -> Option<Value> {
    let data = map_of(payload)?;
    let pools_raw = data.get("pools")?.as_array()?;
    struct Pool {
        name: String,
        pool_type: String,
        window_5h: Option<Value>,
        window_7d: Option<Value>,
        grant_balance: Option<f64>,
    }
    let mut pools: Vec<Pool> = Vec::new();
    for item in pools_raw {
        let Some(map) = map_of(item) else { continue };
        let window_5h = read_sensenova_window(map.get("window_5h"));
        let window_7d = read_sensenova_window(map.get("window_7d"));
        if window_5h.is_none() && window_7d.is_none() {
            continue;
        }
        pools.push(Pool {
            name: map
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .unwrap_or("积分池")
                .to_string(),
            pool_type: map
                .get("pool_type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            window_5h,
            window_7d,
            grant_balance: number_at(map, "grant_balance"),
        });
    }
    if pools.is_empty() {
        return None;
    }
    let general = pools
        .iter()
        .find(|pool| pool.pool_type == "default")
        .or_else(|| pools.first())?;
    let available = window_remaining_percent(general.window_5h.as_ref())
        .or_else(|| window_remaining_percent(general.window_7d.as_ref()));
    let mut wallets: Vec<Value> = Vec::new();
    for pool in &pools {
        for (key, label, window) in [
            ("5h", "5 小时", pool.window_5h.as_ref()),
            ("7d", "7 天", pool.window_7d.as_ref()),
        ] {
            let Some(window) = window else { continue };
            if let Some(row) = wallet(
                key,
                &format!("{} · {}", pool.name, label),
                window_remaining_percent(Some(window)),
                "%",
            ) {
                wallets.push(row);
            }
        }
        if let Some(row) = wallet("grant", &format!("{} · 活动返赠", pool.name), pool.grant_balance, "积分") {
            wallets.push(row);
        }
    }
    Some(document(available, "%", wallets, payload))
}

/// 单层窗口 → 只留三个数值（`{limit, used, remaining, reset_at}`）
fn read_sensenova_window(value: Option<&Value>) -> Option<Value> {
    let map = value.and_then(map_of)?;
    let limit = number_at(map, "limit");
    let used = number_at(map, "used");
    let remaining = number_at(map, "remaining");
    if limit.is_none() && used.is_none() && remaining.is_none() {
        return None;
    }
    let mut out = Map::new();
    if let Some(limit) = limit {
        out.insert("limit".to_string(), json!(limit));
    }
    if let Some(used) = used {
        out.insert("used".to_string(), json!(used));
    }
    if let Some(remaining) = remaining {
        out.insert("remaining".to_string(), json!(remaining));
    }
    if let Some(resets) = unix_to_ms(map.get("reset_at")) {
        out.insert("resetsAt".to_string(), Value::from(resets));
    }
    Some(Value::Object(out))
}

/// 窗口剩余百分比（`remaining / limit`，夹在 0~100；缺值给 None）
fn window_remaining_percent(window: Option<&Value>) -> Option<f64> {
    let window = window.and_then(map_of)?;
    let limit = number_at(window, "limit").filter(|value| *value > 0.0)?;
    let remaining = number_at(window, "remaining")?;
    Some((remaining / limit * 100.0).clamp(0.0, 100.0))
}
