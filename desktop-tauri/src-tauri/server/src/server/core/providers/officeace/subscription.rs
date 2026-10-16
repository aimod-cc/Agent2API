//! OfficeAce 的订阅 / 额度 / 奖励族接口：控制面 **V11-HMAC-SHA256** 签名。
//!
//! ── 挂在网关根，不是 chat 的 `/v2` ──────────────────────────
//! 账号里存的 `baseUrl` 是 `https://{host}/v2`（见 `oauth::fetch_gateway_credential`），
//! 而 `/v1/subscription` 与 `/v1/subscription/bonus/claim` 都挂在**网关根**上；
//! [`gateway_base`] 去掉尾 `/v2` 再拼路径。
//!
//! ── 为什么不用 `auth_http::send_raw` ─────────────────────────
//! `send_raw` 在 body 非空时会**自己补一个 `Content-Type`**，而签名要求
//! `content-type` 以**同一个值**进签名头集合 —— 两条路一起走会发成两个
//! `Content-Type`。照 `codearts::welfare` 的先例：用 `egress::client_for(None)`
//! 直接发，显式带上签名算出的全套头（`Host` 由签名层从 URL 注入，见 `signer`）。
//!
//! ── 三条实测坑（写在这里，别在别处重推）──────────────────────
//!   1. 领奖励 `POST /v1/subscription/bonus/claim` **必须** body 给 `{}` 且带
//!      `Content-Type: application/json` —— 裸 POST（无体）会 500 `OfficeAce.11020001`。
//!   2. `x-subscription-type: v2` 是 `/v1/subscription` 与 bonus/claim 的必要头
//!      （usage / rates 不带）；它进签名头集合。
//!   3. 一天一个活动只领一次，**重复领不报错**（返回同一份列表）—— 幂等由上游保证。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Map, Value};

use crate::server::core::egress;
use crate::server::errors::GatewayError;

use super::credentials::OfficeAceCredential;
use super::signer::{self, Algorithm, SignRequest};

/// 订阅族接口总超时（余额/签到是外部 HTTP，必须设总超时：`egress` 默认 read_timeout
/// 600 秒是给 SSE 长连接的，直接用会把一个挂住的查询拖到天荒地老）。
const TIMEOUT_SECS: u64 = 20;

/// 账号里存的 `https://{host}/v2` → 网关根 `https://{host}`。
pub fn gateway_base(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/v2")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_string()
}

/// 响应体截断（排障用；不打印凭据 —— 这里只截原始响应文本）。
fn excerpt(text: &str) -> String {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= 240 {
        return cleaned;
    }
    cleaned.chars().take(240).collect::<String>() + "…"
}

/// 调一次 V11 签名的订阅族接口，返回解析后的响应体。
///
/// `body`（有体时）与 `extra_headers` 都**进签名**；`Content-Type` 由这里有体时补上。
async fn call(
    credential: &OfficeAceCredential,
    method: &str,
    path: &str,
    body: Option<Value>,
    extra_headers: &[(&str, &str)],
) -> Result<Value, GatewayError> {
    if !credential.has_control_plane() {
        return Err(crate::server::core::providers::adapter::usage_not_configured(
            "OfficeAce 果办",
            "控制面临时凭据（AK/SK）",
        ));
    }
    let base = gateway_base(&credential.base_url);
    if base.is_empty() {
        return Err(GatewayError::with_status(400, "OfficeAce 账号缺少模型网关基址"));
    }
    let url = format!("{base}{path}");
    let signing = credential
        .signing()
        .map_err(|reason| GatewayError::with_status(500, format!("OfficeAce 签名凭据不可用：{reason}")))?;
    let payload = body
        .as_ref()
        .map(|value| value.to_string().into_bytes())
        .unwrap_or_default();
    let mut headers: Vec<(String, String)> = extra_headers
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    if body.is_some() {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    let signed = signer::sign(&SignRequest {
        algorithm: Algorithm::V11,
        method,
        url: &url,
        headers: &headers,
        body: &payload,
        credential: &signing,
        region: signer::DEFAULT_REGION,
        date_override: None,
    })
    .map_err(|reason| GatewayError::with_status(500, format!("OfficeAce 订阅请求签名失败：{reason}")))?;
    let built = match method {
        "POST" => egress::client_for(None)
            .post(&url)
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .body(payload),
        _ => egress::client_for(None)
            .get(&url)
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS)),
    };
    let mut request = built;
    for (name, value) in signed {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request.send().await.map_err(|error| {
        GatewayError::with_status(
            502,
            format!("OfficeAce 订阅请求失败：{}", egress::describe_error_detail(&error)),
        )
    })?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    // 401/403 = 控制面临时凭据失效（约 2 小时过期），要用户重新登录，与网络错误区分开
    if status == 401 || status == 403 {
        return Err(GatewayError::with_status(
            i32::from(status),
            "OfficeAce 控制面凭据已失效（临时 AK/SK 会过期）：请重新登录一次",
        ));
    }
    if status != 200 {
        return Err(GatewayError::with_status(
            i32::from(status),
            format!("OfficeAce 订阅接口返回 HTTP {status}：{}", excerpt(&text)),
        ));
    }
    serde_json::from_str::<Value>(&text)
        .map_err(|_| GatewayError::with_status(502, "OfficeAce 订阅接口响应不是合法 JSON"))
}

/// 读订阅快照（套餐 + 积分账本 + 奖励活动）。
pub async fn fetch_subscription(credential: &OfficeAceCredential) -> Result<Value, GatewayError> {
    call(
        credential,
        "GET",
        "/v1/subscription",
        None,
        &[("x-subscription-type", "v2")],
    )
    .await
}

/// 每模型使用倍率：`GET /v1/model-token-rates`（控制面，要 V11 签名）。
///
/// 响应是 `{models: [{model_name, cost_coefficient, prompt_tokens_per_point, …}]}`，
/// `cost_coefficient` 就是官方客户端里那颗 `0.30x`。参考实现同源（`quota.mjs` 的
/// `summary.rates` 与 `/admin/api/model-rates` 那条兜底代理），字段名逐个照抄。
pub async fn fetch_model_token_rates(
    credential: &OfficeAceCredential,
) -> Result<Value, GatewayError> {
    call(credential, "GET", "/v1/model-token-rates", None, &[]).await
}

/// 领奖励积分：`POST /v1/subscription/bonus/claim`（body `{}`，幂等）。
///
/// 返回完整响应体（`bonus_skus` 是当前所有奖励活动，含刚领到的）。
pub async fn claim_bonus(credential: &OfficeAceCredential) -> Result<Value, GatewayError> {
    call(
        credential,
        "POST",
        "/v1/subscription/bonus/claim",
        Some(json!({})),
        &[("x-subscription-type", "v2")],
    )
    .await
}

/// 一个奖励活动的归一形态（从 `bonus_skus` 的条目解出）。
pub struct Bonus {
    pub activity_id: String,
    pub name: String,
    pub total: f64,
    pub used: f64,
    pub expires_at: i64,
}

impl Bonus {
    pub fn remaining(&self) -> f64 {
        (self.total - self.used).max(0.0)
    }
}

/// 上游的数值字段是**字符串**（实测样例：`"sku_value": "500"`、`"points": "4000"`）——
/// `Value::as_f64()` 对字符串恒回 `None`，直接用它会把余额/奖励全算成 0。
/// 所有数值都走这里：数字直接用、字符串解析、其余给 0（与 JS 的 `Number(v)` 同口径）。
fn num(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::String(text)) => text.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// 一个 `bonus_skus` 条目 → [`Bonus`]（解析口径见 [`bonuses_of`]）。
fn bonus_of(item: &Value) -> Option<Bonus> {
    let text = |field: &str| {
        item.get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    // 键的兜底：实测**奖励条目可能没有 `activity_id`**（样例只有 `activity_name`）——
    // 只认 `activity_id` 会把这些条目整条丢掉（表现为「没有任务」）。
    let key = text("activity_id")
        .or_else(|| text("cbc_resource_id"))
        .or_else(|| text("activity_name"))?;
    Some(Bonus {
        name: text("activity_name").unwrap_or_else(|| key.clone()),
        total: num(item.get("points")),
        used: num(item.get("current_value")),
        expires_at: parse_ms(item.get("expired_time")),
        activity_id: key,
    })
}

/// 从 `bonus_skus` 数组解出奖励活动（形状见参考实现的 `summarizeSubscription`）。
pub fn bonuses_of(subscription: &Value) -> Vec<Bonus> {
    subscription
        .get("bonus_skus")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(bonus_of).collect())
        .unwrap_or_default()
}

/// 把「过期时刻」解成毫秒时间戳：上游给 RFC3339 串（`2026-10-10T05:15:00Z`）或数字。
fn parse_ms(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0),
        Some(Value::String(text)) => super::oauth::parse_rfc3339_ms(text).unwrap_or(0),
        _ => 0,
    }
}

/// 积分数值取整（**向下**）—— 口径说明见 [`summarize`]；负值 / 非有限值一律 0。
fn whole_points(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        0.0
    } else {
        value.floor()
    }
}

/// 上游数值字段 → `Option`（缺失 / 空串 / 非数字 / 非有限值一律 `None`）。
///
/// 与 [`num`] 的分工：聚合读数要一个**可加的数**，缺失折 0（口径见 [`summarize`]）；
/// 明细行要回答「上游到底给没给」，缺失必须**整个不写这个键** —— 写成 0 会被界面
/// 画成「只剩 0 / 已用 0」（`Number(null) === 0`），这是明细这条线最要防的事故。
///
/// 非有限值也挡在这里：serde_json 只能把 NaN / ±inf 落成 `null`
/// （`json!(f64::INFINITY)` 出来就是 `null`），而 `null` 与缺键在界面上同义 ——
/// 那就不如缺键，至少不会多写一个假数出去。
fn num_opt(value: Option<&Value>) -> Option<f64> {
    let parsed = match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    parsed.filter(|value| value.is_finite())
}

/// 一行明细 → `(排序键, 名字, 行)`（键顺序与 workbuddy 的 `billing::credit_packs_from`、
/// trae 的 `usage::credit_rows` 一致 —— 界面因此只写一个读取器）。
///
/// ── 谁缺谁不写，一个 `null` 都不许出去 ──────────────────────
/// 三个数（`total` / `used` / `remain`）与 `expireAt` 都收 `Option`，`None` 直接
/// 不出键：界面那道 `Number(null) === 0` 的闸就在这一层之后，写 0 或者写 `null`
/// 都会被画成「剩 0 / 共 0」这种看起来像读到了的假读数。
///
/// `unit` 只在与文档级单位（本文件的「积分」）不同的行上才有值 —— 这里只有联网
/// 搜索那一行（按**次**计），同单位的行不必每行再标一遍。
///
/// `expire_ms` 兼职排序键：**最近到期的排前面**（明细第一个要回答的问题是
/// 「哪笔先作废」），没给到期时刻的排最后（`i64::MAX`），同一时刻按名字比
/// （否则每次刷新行序都在跳，看起来像数据在变）。
fn credit_row(
    name: &str,
    total: Option<f64>,
    used: Option<f64>,
    remain: Option<f64>,
    unit: Option<&str>,
    expire_ms: i64,
) -> (i64, String, Value) {
    let name = name.trim();
    let mut row = Map::new();
    row.insert("name".to_string(), Value::String(name.to_string()));
    for (key, value) in [("total", total), ("used", used), ("remain", remain)] {
        if let Some(value) = value.filter(|value| value.is_finite()) {
            row.insert(key.to_string(), json!(value));
        }
    }
    if let Some(unit) = unit {
        row.insert("unit".to_string(), Value::String(unit.to_string()));
    }
    if expire_ms > 0 {
        row.insert("expireAt".to_string(), json!(expire_ms));
    }
    (
        if expire_ms > 0 { expire_ms } else { i64::MAX },
        name.to_string(),
        Value::Object(row),
    )
}

/// 把 `/v1/subscription` 归一成 `query_usage` 的形状（见 `adapter::ProviderAdapter`
/// 的 `query_usage` 契约）。
///
/// 只算**未过期**的额度（过期条目不计入，与参考实现的 `summarizeSubscription` 一致）；
/// `available` = 套餐剩余 + 奖励剩余。
///
/// ── 为什么剩余量要取整（2026-10-13，用户报「可用 6499.331797714286 积分」）──
/// 上游的 `current_value` 是**带小数的字符串**（实测 `"0.6682022857142857"`：
/// 模型调用的消耗按比例折算成积分的零头），相减之后剩余量就拖着一长串小数。
/// 而积分本身是整数概念（套餐 500、奖励按千发放），余额列又只有几十像素宽 ——
/// 6499.331797714286 既挤又读不出信息。口径取**向下取整**：积分的用途是
/// 「还能用多少」的判断依据，宁可少报不到 1 分的零头，也不虚报。
pub fn summarize(subscription: &Value) -> Value {
    let now = crate::server::logging::now_ms();
    let mut plan_total = 0.0;
    let mut plan_used = 0.0;
    let mut plan_name = String::new();
    let mut plan_expire = 0i64;
    let mut search_total = 0.0;
    let mut search_used = 0.0;
    // 逐份明细（`credits[]`，账号页「积分明细」弹层的输入）的行在这里顺手收齐：
    // 排序键 + 名字 + 行，读的是同一个循环里的同一份 quotas —— 第二遍遍历只会让
    // 两处口径有机会分叉。循环里被 `continue` 排掉的过期配额不进明细：聚合读数
    // （`available` 与套餐钱包）本来就不计它们，弹层要回答的是「现在还能花什么」。
    let mut rows: Vec<(i64, String, Value)> = Vec::new();
    for sku in subscription
        .get("skus")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for quota in sku.get("quotas").and_then(Value::as_array).into_iter().flatten() {
            let expired = parse_ms(quota.get("expired_time"));
            if expired > 0 && expired <= now {
                continue;
            }
            match quota.get("sku_attr_code").and_then(Value::as_str) {
                Some("officeace_points") => {
                    plan_total += num(quota.get("sku_value"));
                    plan_used += num(quota.get("current_value"));
                    // 聚合之外逐条再出明细行（聚合是读数，明细是账本：一份配额一行）——
                    // 明细这条线走 num_opt，「上游没给」与「真的 0」不混
                    let total = num_opt(quota.get("sku_value"));
                    let used = num_opt(quota.get("current_value"));
                    // 剩余是减出来的：两个基数缺一个就**不写这个键**（拿 0 顶上等于
                    // 替上游宣布「没用过 / 没发过」）
                    let remain = total.zip(used).map(|(total, used)| whole_points(total - used));
                    // 配额自己没有名字，挂所属 sku 的；上游连 sku 名字都没给时写
                    // 「套餐积分」—— 界面的兜底名「积分包」分不出这是套餐还是活动发的
                    let sku_name = sku.get("sku_name").and_then(Value::as_str).unwrap_or("").trim();
                    let name = if sku_name.is_empty() { "套餐积分" } else { sku_name };
                    rows.push(credit_row(name, total, used, remain, None, expired));
                }
                Some("online_search_count") => {
                    search_total += num(quota.get("sku_value"));
                    search_used += num(quota.get("current_value"));
                }
                _ => {}
            }
        }
        if plan_name.is_empty() {
            plan_name = sku
                .get("sku_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
        }
        let expire = parse_ms(sku.get("expired_time"));
        if expire > 0 && (plan_expire == 0 || expire < plan_expire) {
            plan_expire = expire;
        }
    }
    let bonuses = bonuses_of(subscription);
    let bonus_total: f64 = bonuses
        .iter()
        .filter(|bonus| bonus.expires_at == 0 || bonus.expires_at > now)
        .map(|bonus| bonus.total)
        .sum();
    let bonus_used: f64 = bonuses
        .iter()
        .filter(|bonus| bonus.expires_at == 0 || bonus.expires_at > now)
        .map(|bonus| bonus.used)
        .sum();
    // 剩余量一律向下取整（口径见函数文档）：余额列 / 明细弹层读的都是这三处，
    // 取整后「可用 = 套餐 + 奖励」的加法仍自洽（三处同口径）
    let plan_remaining = whole_points(plan_total - plan_used);
    let bonus_remaining = whole_points(bonus_total - bonus_used);
    let available = plan_remaining + bonus_remaining;
    let status = subscription
        .get("subscribe_status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    // 联网搜索聚合成一行：上游按 sku 把它们拆成多条，逐条摆出来只是同一件事的
    // 碎片（聚合口径与 `subscription.searches` 同源，`search_total` / `search_used`
    // 就是上面循环顺手收的两个数）。单位是「次」、与文档的「积分」不同，这一行才
    // 写 `unit`；上游没给搜索额度的到期时刻，`expireAt` 整个不写。
    // 非有限值不出发：`json!` 会把它写成 `null`，那与「没读到」在界面上同义。
    if search_total > 0.0 && search_total.is_finite() {
        rows.push(credit_row(
            "联网搜索",
            Some(search_total),
            Some(search_used),
            Some(whole_points(search_total - search_used)),
            Some("次"),
            0,
        ));
    }
    // 奖励活动**全部条目都出，含已过期的**：聚合读数只算未过期的（`available` /
    // 奖励钱包），明细要能回答「哪笔作废了」—— 界面对已过期的另有折叠分组，
    // 这个差异是刻意的。三个数走 `Bonus` 的既有口径（缺失折 0，与奖励钱包同源），
    // `remain` 与 `Bonus::remaining` 同式再套全文件的向下取整。
    for bonus in &bonuses {
        rows.push(credit_row(
            &bonus.name,
            Some(bonus.total),
            Some(bonus.used),
            Some(whole_points(bonus.remaining())),
            None,
            bonus.expires_at,
        ));
    }
    let mut document = json!({
        "available": available,
        "unit": "积分",
        "wallets": [
            { "type": "plan", "displayName": plan_name_or_default(&plan_name), "balance": plan_remaining },
            { "type": "bonus", "displayName": "奖励积分", "balance": bonus_remaining },
        ],
        "subscription": {
            "planName": plan_name_or_default(&plan_name),
            "status": status,
            "expireAt": plan_expire,
            "remainQuota": plan_remaining,
            "totalQuota": plan_total,
            "searches": if search_total > 0.0 {
                json!({ "remaining": whole_points(search_total - search_used), "total": search_total })
            } else {
                Value::Null
            },
        },
        "raw": subscription,
    });
    // 明细只在有行时挂（键在不在 + 长度就是界面的入口开关）：一条可写的行都没有的
    // 账户挂个空数组，只会多出一个点开什么都没有的入口
    if !rows.is_empty() {
        rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        if let Some(object) = document.as_object_mut() {
            object.insert(
                "credits".to_string(),
                Value::Array(rows.into_iter().map(|(_, _, row)| row).collect()),
            );
        }
    }
    document
}

fn plan_name_or_default(name: &str) -> String {
    if name.is_empty() {
        "OfficeAce 果办".to_string()
    } else {
        name.to_string()
    }
}

/// `activity_id → 该活动的剩余量`（两帧对比、任务行渲染都用它）。
pub fn remaining_map(subscription: &Value) -> std::collections::HashMap<String, f64> {
    bonuses_of(subscription)
        .into_iter()
        .map(|bonus| {
            let remaining = bonus.remaining();
            (bonus.activity_id, remaining)
        })
        .collect()
}

/// 两帧订阅之间「这次多了多少」：按每个奖励活动的**余额增量**求和。
///
/// 首次见到的活动（`before` 里没有）退一步用它的 `remaining`（余额），而**不是**
/// `total` —— 后者是**累计发放**，可能早就由别的渠道（桌面端、活动页）发过，算进去
/// 会让读数虚高（参考实现踩过：签到页显示「领到 100」而实际只多出一点）。
pub fn gained_between(before: &Value, after: &Value) -> f64 {
    let previous = remaining_map(before);
    bonuses_of(after)
        .iter()
        .map(|bonus| match previous.get(&bonus.activity_id) {
            Some(prev) => (bonus.remaining() - prev).max(0.0),
            None => bonus.remaining(),
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_base_strips_the_v2_suffix() {
        assert_eq!("https://h", gateway_base("https://h/v2"));
        assert_eq!("https://h", gateway_base("https://h/v2/"));
        assert_eq!("https://h", gateway_base("https://h"));
        assert_eq!("https://h", gateway_base("  https://h/v2  "));
    }

    #[test]
    fn summarize_adds_plan_and_bonus_remaining() {
        // 关键：上游的数值是**字符串**（实测样例 `"sku_value": "500"`）—— as_f64 会算成 0
        let sub = json!({
            "subscribe_status": "SUBSCRIBED",
            "skus": [{
                "sku_name": "标准版",
                "expired_time": "2099-01-01T00:00:00Z",
                "quotas": [
                    { "sku_attr_code": "officeace_points", "sku_value": "500", "current_value": "120" },
                    { "sku_attr_code": "online_search_count", "sku_value": "200", "current_value": "0" }
                ]
            }],
            "bonus_skus": [
                // 奖励条目**可能没有 activity_id**（只有 activity_name）—— 不能被丢掉
                { "activity_name": "新用户专属福利", "points": "100", "current_value": "0" }
            ]
        });
        let out = summarize(&sub);
        // 500-120 = 380（套餐）+ 100-0 = 100（奖励）= 480
        assert_eq!(480.0, out["available"].as_f64().unwrap());
        assert_eq!("积分", out["unit"]);
        assert_eq!(380.0, out["wallets"][0]["balance"].as_f64().unwrap());
        assert_eq!(100.0, out["wallets"][1]["balance"].as_f64().unwrap());
        assert_eq!(500.0, out["subscription"]["totalQuota"].as_f64().unwrap());
        assert_eq!(200.0, out["subscription"]["searches"]["total"].as_f64().unwrap());
    }

    #[test]
    fn summarize_skips_expired_quotas() {
        let sub = json!({
            "skus": [{
                "quotas": [
                    { "sku_attr_code": "officeace_points", "sku_value": 999, "current_value": 0,
                      "expired_time": "2000-01-01T00:00:00Z" }
                ]
            }],
            "bonus_skus": []
        });
        let out = summarize(&sub);
        assert_eq!(0.0, out["available"].as_f64().unwrap(), "过期的额度不计入");
    }

    #[test]
    fn bonuses_keep_entries_without_activity_id_but_drop_unidentifiable_ones() {
        let sub = json!({ "bonus_skus": [
            { "points": "1" },                                                  // 无任何标识 → 丢
            { "activity_id": "a", "points": "5", "current_value": "2" },        // 有 id
            { "activity_name": "新用户专属福利", "points": "4000", "current_value": "0" }, // 只有名字 → 保留（键退到名字）
        ] });
        let bonuses = bonuses_of(&sub);
        assert_eq!(2, bonuses.len(), "无标识的丢掉，只有名字的保留");
        assert_eq!("a", bonuses[0].activity_id);
        assert_eq!(3.0, bonuses[0].remaining());
        assert_eq!("新用户专属福利", bonuses[1].activity_id);
        assert_eq!(4000.0, bonuses[1].total, "字符串数字要解析出来");
    }
}
