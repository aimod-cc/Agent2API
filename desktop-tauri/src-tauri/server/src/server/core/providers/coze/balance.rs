//! Coze 余额 / 积分明细 / 订阅查询（账号页四格的原料）。
//!
//! ── 三个接口，一只 PAT（2026-10-21 活 PAT 实测）──────────────
//! ```text
//! GET /api/marketplace/trade/credit/balance
//!     → {code:0, data:{items:[{remaining, source, total}],
//!                      total_balance:1500, total_remain:889}}
//! GET /api/marketplace/trade/credit/balance/detail
//!     ?account_type=2&display_status=2&credit_balance_type=6&page_size=50
//!     → {code:0, data:{items:[{source, started_at, expired_at, total,
//!                             remaining, display_status, show_name, title,
//!                             min_available_user_level}]}}
//! GET /api/marketplace/trade/subscription_v2
//!     → {code:0, data:{user_basic_info:{user_level_v2, volc_account_info:{…}}}}
//! ```
//!
//! ⚠️ 三个坑（都实测踩过，别照着「看起来应该」改）：
//!   1. `account_type` 的 **Personal = 2**（Organization = 1）—— 传 1 上游回
//!      `Failed to call downstream server`，看着像服务故障，其实是参数错；
//!   2. `display_status`：1=可用 / 2=即将到期 / 3=用尽 / 4=已过期 / 5=全部 /
//!      6=冻结。查「最近到期」要的是 **2**，但只看 2 会漏掉「可用但已在
//!      倒计时」的包 —— 于是这里改成 `5`（全部）拉回来自己按 `expired_at` 分，
//!      顺带把「还剩多少」也一并拿到（上游的 `remaining` 只在明细里有）；
//!   3. 明细里的时间字段是 **epoch 秒字符串**（`"1792587101"`），不是毫秒也不是
//!      ISO 串；前端 `credits[].expireAt` 要**毫秒**，`credits_of` 负责 ×1000。
//!
//! `source` 口径（renderer 的 11748 chunk 枚举）：21=订阅积分 / 22=购买 /
//! 24=活动 / 27=退款。未知取值不丢弃 —— 原样以 `source_<n>` 作桶名，宁可
//! 显示成陌生名字，也不要让一笔钱凭空消失。
//!
//! ── 归一化形状（`ProviderAdapter::query_usage` 的契约）────────
//! ```jsonc
//! { "available": 889, "unit": "积分",
//!   "wallets": [{type, displayName, balance, total}],
//!   "subscription": {planName, status, expireAt} | null,
//!   "credits": [{name, remain, total, used, expireAt, source}],  // 到期升序
//!   "raw": {balance, detail, subscription} }
//! ```
//! `credits[]` 是「最近到期时间 / 最近到期积分」两列的原料：前端
//! `creditExpiryOf` 读 `expireAt`（毫秒，>0 且未过期才算）、`creditRowsOf`
//! 读 `remain`。因此本家要登记 `creditDetail` 与 `CREDIT_EXPIRY_PROVIDERS`。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Map, Value, json};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::credentials;
use super::http;
use super::WEB_BASE;

/// 单接口超时（余额类接口都在 1 秒内回，20 秒足够容忍排队）
const USAGE_TIMEOUT_MS: u64 = 20_000;

/// `account_type`：**个人号 = 2**（组织 = 1）。实测传 1 回
/// `Failed to call downstream server`，这不是猜测值。
const ACCOUNT_TYPE_PERSONAL: i64 = 2;

/// `display_status`：全部（1 可用 / 2 即将到期 / 3 用尽 / 4 已过期 /
/// 5 全部 / 6 冻结）。拉全部再自己分类，比按 2 猜要全。
const DISPLAY_STATUS_ALL: i64 = 5;

/// 明细一页拉多少（免费号几十条封顶，50 足够；翻页留给将来）
const DETAIL_PAGE_SIZE: i64 = 50;

/// 积分包的来源桶（`source` → 展示名）。
const SOURCE_NAMES: [(i64, &str); 4] = [
    (21, "订阅积分"),
    (22, "购买积分"),
    (24, "活动积分"),
    (27, "退款积分"),
];

/// 会员等级（`user_level_v2` → 档位名）。免费号是 0。
///
/// 映射来自 renderer 的档位枚举；**未列出的取值不猜**，回落
/// `等级 <n>`（宁可显示数字，也不要编一个不存在的套餐名）。
const LEVEL_NAMES: [(i64, &str); 9] = [
    (0, "免费版"),
    (10, "Premium Lite"),
    (15, "Premium"),
    (20, "Premium Plus"),
    (112, "Coze 个人版"),
    (114, "Coze 个人版 Pro"),
    (116, "Coze 个人版 Max"),
    (118, "Coze 个人版 Ultra"),
    (120, "Coze 团队版"),
];

/// 来源桶的展示名（未知取值给 `积分（来源 n）`）。
fn source_name(source: i64) -> String {
    SOURCE_NAMES
        .iter()
        .find(|(code, _)| *code == source)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| format!("积分（来源 {source}）"))
}

/// 等级档位名（未知取值给 `等级 <n>`）。
fn level_name(level: i64) -> String {
    LEVEL_NAMES
        .iter()
        .find(|(code, _)| *code == level)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| format!("等级 {level}"))
}

/// 时间字段归一成**毫秒**（上游给 epoch 秒，可能带引号；0 / 缺失 → None）。
fn epoch_secs_to_ms(value: Option<&Value>) -> Option<i64> {
    let raw = match value {
        Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
        Some(other) => other.as_i64(),
        None => None,
    }?;
    if raw <= 0 {
        return None;
    }
    // 已经是毫秒的形态（>1e12）原样用，其余按秒 ×1000
    if raw > 1_000_000_000_000 {
        Some(raw)
    } else {
        Some(raw.saturating_mul(1000))
    }
}

/// 一个 JSON 数值（字符串形态也认）。
fn number_of(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        Some(other) => other.as_f64(),
        None => None,
    }
}

/// 业务码判据：`code == 0` 才算成功。失败时给出可读文案（401 一律按登录态失效）。
fn ensure_code(value: &Value, what: &str) -> Result<(), GatewayError> {
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code == 0 {
        return Ok(());
    }
    let message = value
        .get("msg")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("未知错误")
        .to_string();
    // 上游用 700012006 一类业务码表达「登录认证失败」—— 归成 401，
    // 让调用方走「重新登录」而不是把它当成一次服务故障
    if code == 700012006 || message.contains("登录") || message.contains("认证") {
        return Err(GatewayError::with_status(
            401,
            format!("Coze {what}失败（code={code}：{message}）——请重新登录该账号"),
        ));
    }
    Err(GatewayError::with_status(
        502,
        format!("Coze {what}失败（code={code}：{message}）"),
    ))
}

/// 查询某账号的余额 + 最近到期 + 订阅（归一化形状见模块头）。
///
/// 三个接口**互不依赖**：任一个失败都不该让整格变红 —— 余额是主读数，
/// 明细 / 订阅是加分项。因此顺序是「余额失败即整体失败，明细 / 订阅失败
/// 只记日志、缺键」，与各家「主额度拿到就算查到」同一取向。
pub async fn query_usage(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let pat = credentials::ensure_pat(store, account_id, false).await?;
    let headers = credentials::pat_headers(&pat);
    // 出口跟着账号走（与转发链同源）：用户给这个账号配的代理，余额查询也该用
    // —— 否则「转发正常、余额失败」会变成一条无法归因的故障。
    let proxy = credentials::account_proxy(store, account_id);

    // 1) 余额（主读数；失败就是这次查询失败）
    let balance_url = format!("{WEB_BASE}/api/marketplace/trade/credit/balance");
    let balance = http::get_json_value(&balance_url, &headers, proxy.as_ref(), Some(USAGE_TIMEOUT_MS)).await?;
    ensure_code(&balance, "余额查询")?;

    // 2) 积分明细（最近到期 / 逐份剩余）
    let detail_url = format!(
        "{WEB_BASE}/api/marketplace/trade/credit/balance/detail\
         ?account_type={ACCOUNT_TYPE_PERSONAL}&display_status={DISPLAY_STATUS_ALL}\
         &credit_balance_type=6&page_size={DETAIL_PAGE_SIZE}"
    );
    let detail = match http::get_json_value(&detail_url, &headers, proxy.as_ref(), Some(USAGE_TIMEOUT_MS)).await {
        Ok(value) => match ensure_code(&value, "积分明细查询") {
            Ok(()) => Some(value),
            Err(error) => {
                crate::server::logging::verbose("[Usage]", &format!("Coze 积分明细跳过：{}", error.message));
                None
            }
        },
        Err(error) => {
            crate::server::logging::verbose("[Usage]", &format!("Coze 积分明细跳过：{}", error.message));
            None
        }
    };

    // 3) 订阅（会员档位 / 到期时刻）
    let subscription_url = format!("{WEB_BASE}/api/marketplace/trade/subscription_v2");
    let subscription =
        match http::get_json_value(&subscription_url, &headers, proxy.as_ref(), Some(USAGE_TIMEOUT_MS)).await {
            Ok(value) => match ensure_code(&value, "订阅查询") {
                Ok(()) => Some(value),
                Err(error) => {
                    crate::server::logging::verbose("[Usage]", &format!("Coze 订阅信息跳过：{}", error.message));
                    None
                }
            },
            Err(error) => {
                crate::server::logging::verbose("[Usage]", &format!("Coze 订阅信息跳过：{}", error.message));
                None
            }
        };

    let data = balance.get("data").cloned().unwrap_or(Value::Null);
    let available = number_of(data.get("total_remain"))
        .or_else(|| number_of(data.get("total_balance")))
        .unwrap_or(0.0);
    let total_balance = number_of(data.get("total_balance"));

    // 钱包：按 source 分桶（上游 items 已经是分桶形态，这里只改名 + 补总量）
    let mut wallets: Vec<Value> = Vec::new();
    if let Some(items) = data.get("items").and_then(Value::as_array) {
        for item in items {
            let source = item.get("source").and_then(Value::as_i64).unwrap_or(0);
            let remaining = number_of(item.get("remaining")).unwrap_or(0.0);
            let total = number_of(item.get("total"));
            wallets.push(json!({
                "type": format!("source_{source}"),
                "displayName": source_name(source),
                "balance": remaining,
                "total": total,
            }));
        }
    }
    // 上游没有分桶明细（`items` 空）但总量非零时补一行，避免「有余额却无来源」
    if wallets.is_empty() && available > 0.0 {
        wallets.push(json!({
            "type": "total",
            "displayName": "可用积分",
            "balance": available,
            "total": total_balance,
        }));
    }

    let credits = credits_of(detail.as_ref());
    let subscription_view = subscription_of(subscription.as_ref());

    let mut raw = Map::new();
    raw.insert("balance".to_string(), balance);
    if let Some(value) = detail {
        raw.insert("detail".to_string(), value);
    }
    if let Some(value) = subscription {
        raw.insert("subscription".to_string(), value);
    }

    Ok(json!({
        "available": available,
        "unit": "积分",
        "wallets": wallets,
        "subscription": subscription_view,
        "credits": credits,
        "raw": Value::Object(raw),
    }))
}

/// 明细 → `credits[]`（前端 `creditExpiryOf` / `creditRowsOf` 的原料）。
///
/// 只收「还有剩余且未过期」的包：`remaining > 0` 且（`expired_at` 缺失或尚未
/// 到期）。**按到期升序**排好（最近到期的在前），与 trae / minimax 同一取向
/// —— 前端取 `credits[0]` 做「最近到期」时不必再排一次。
fn credits_of(detail: Option<&Value>) -> Vec<Value> {
    let Some(detail) = detail else { return Vec::new() };
    let items = detail
        .get("data")
        .and_then(|data| data.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let now_ms = crate::server::logging::now_ms();
    let mut rows: Vec<Value> = Vec::new();
    for item in &items {
        let remain = number_of(item.get("remaining")).unwrap_or(0.0);
        if remain <= 0.0 {
            continue;
        }
        let expire_at = epoch_secs_to_ms(item.get("expired_at").or_else(|| item.get("expire_at")));
        if let Some(expire) = expire_at {
            if expire <= now_ms {
                continue;
            }
        }
        let source = item.get("source").and_then(Value::as_i64).unwrap_or(0);
        let total = number_of(item.get("total")).unwrap_or(remain);
        let name = item
            .get("show_name")
            .or_else(|| item.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| source_name(source));
        let mut row = Map::new();
        row.insert("name".to_string(), Value::String(name));
        row.insert("source".to_string(), Value::from(source));
        row.insert("remain".to_string(), json!(remain));
        row.insert("total".to_string(), json!(total));
        row.insert("used".to_string(), json!((total - remain).max(0.0)));
        if let Some(expire) = expire_at {
            row.insert("expireAt".to_string(), Value::from(expire));
        }
        if let Some(granted) = epoch_secs_to_ms(item.get("started_at")) {
            row.insert("grantAt".to_string(), Value::from(granted));
        }
        rows.push(Value::Object(row));
    }
    // 到期升序；没有到期时刻的排最后（永久积分不抢「最近到期」的位置）
    rows.sort_by_key(|row| {
        row.get("expireAt")
            .and_then(Value::as_i64)
            .unwrap_or(i64::MAX)
    });
    rows
}

/// 订阅 → 归一化的 `subscription`（档位名 / 状态 / 到期时刻）。
///
/// 免费号 `benefit_type_infos` 是空对象、`endAt` 也没有 —— 此时返回 `null`，
/// 界面那一格自然空着（**不编造到期时间**）。等级名取自 `user_level_v2`，
/// 与 `available` 无关，所以免费号也能显示「免费版」。
fn subscription_of(subscription: Option<&Value>) -> Value {
    let Some(subscription) = subscription else { return Value::Null };
    let data = subscription.get("data").cloned().unwrap_or(Value::Null);
    let basic = data.get("user_basic_info").cloned().unwrap_or(Value::Null);
    let level = basic.get("user_level_v2").and_then(Value::as_i64).unwrap_or(0);
    let plan_name = level_name(level);
    // 到期：顶层的 endAt / nextRenewalTime（秒或毫秒字符串都可能）
    let expire_at = epoch_secs_to_ms(
        data.get("endAt")
            .or_else(|| data.get("end_at"))
            .or_else(|| data.get("nextRenewalTime"))
            .or_else(|| data.get("next_renewal_time")),
    );
    let status = data
        .get("lifecycleState")
        .or_else(|| data.get("lifecycle_state"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut view = Map::new();
    view.insert("planName".to_string(), Value::String(plan_name));
    if let Some(status) = status {
        view.insert("status".to_string(), Value::String(status));
    }
    if let Some(expire) = expire_at {
        view.insert("expireAt".to_string(), Value::from(expire));
    }
    Value::Object(view)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{credits_of, epoch_secs_to_ms, level_name, source_name, subscription_of};

    /// 上游时间字段是 **epoch 秒的字符串**，前端要毫秒 —— 换算必须在这一处，
    /// 且已经像毫秒的形态不能被再乘一次（×1000 会溢出成 2032 年之后）。
    #[test]
    fn the_upstream_gives_seconds_and_the_client_wants_milliseconds() {
        assert_eq!(epoch_secs_to_ms(Some(&json!("1792587101"))), Some(1_792_587_101_000));
        assert_eq!(epoch_secs_to_ms(Some(&json!(1_792_587_101i64))), Some(1_792_587_101_000));
        // 已经是毫秒
        assert_eq!(epoch_secs_to_ms(Some(&json!(1_792_587_101_000i64))), Some(1_792_587_101_000));
        // 0 / 缺失 / 非数字 → 不写这个键
        assert_eq!(epoch_secs_to_ms(Some(&json!("0"))), None);
        assert_eq!(epoch_secs_to_ms(Some(&json!(""))), None);
        assert_eq!(epoch_secs_to_ms(None), None);
    }

    /// 明细只收「还有剩余且未过期」的包，并按到期升序排好；
    /// 没有到期时刻的排最后（永久积分不抢「最近到期」的位置）。
    #[test]
    fn only_packs_with_money_and_a_future_expiry_survive_the_sort() {
        let now_secs = crate::server::logging::now_ms() / 1000;
        let detail = json!({
            "code": 0,
            "data": {
                "items": [
                    { "source": 24, "total": 1500, "remaining": 889,
                      "expired_at": (now_secs + 86400 * 30).to_string(), "show_name": "活动积分" },
                    { "source": 22, "total": 500, "remaining": 500,
                      "expired_at": (now_secs + 86400 * 3).to_string(), "show_name": "购买积分" },
                    // 已过期：不进来
                    { "source": 21, "total": 100, "remaining": 60,
                      "expired_at": (now_secs - 86400).to_string() },
                    // 已花完：不进来
                    { "source": 24, "total": 200, "remaining": 0,
                      "expired_at": (now_secs + 86400 * 9).to_string() },
                    // 永久（没有到期）：进来但排最后
                    { "source": 27, "total": 50, "remaining": 50 }
                ]
            }
        });
        let rows = credits_of(Some(&detail));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["name"], json!("购买积分"), "最近到期的必须排第一");
        assert_eq!(rows[1]["name"], json!("活动积分"));
        assert!(rows[2].get("expireAt").is_none(), "永久包不写到期时刻");
        // used = total - remain（不是上游给的，自己算）
        assert_eq!(rows[1]["used"], json!(611.0));
    }

    /// 「免费版」是等级 0，不是「读不到」：`user_level_v2` 缺失或为 0 都该
    /// 显示免费版，而不是编一个套餐名。未知等级回落 `等级 n`。
    #[test]
    fn a_zero_level_is_the_free_plan_not_a_missing_value() {
        assert_eq!(level_name(0), "免费版");
        assert_eq!(level_name(112), "Coze 个人版");
        assert_eq!(level_name(999), "等级 999");
        // 免费号没有 benefit_type_infos / endAt ⇒ 只出档位名，不编到期时刻
        let free = json!({ "code": 0, "data": { "user_basic_info": { "user_level_v2": 0 } } });
        let view = subscription_of(Some(&free));
        assert_eq!(view["planName"], json!("免费版"));
        assert!(view.get("expireAt").is_none());
    }

    /// 四个已知来源各有展示名；未知来源不能丢（宁可显示成陌生名字，
    /// 也不要让一笔钱凭空消失）。
    #[test]
    fn every_source_keeps_a_name() {
        assert_eq!(source_name(21), "订阅积分");
        assert_eq!(source_name(22), "购买积分");
        assert_eq!(source_name(24), "活动积分");
        assert_eq!(source_name(27), "退款积分");
        assert_eq!(source_name(66), "积分（来源 66）");
    }

    /// 没有明细时 `credits` 是空数组（不是 null）—— 前端按数组读，
    /// `null` 会让「最近到期」两列抛类型错。
    #[test]
    fn missing_detail_gives_an_empty_array() {
        assert!(credits_of(None).is_empty());
        assert!(credits_of(Some(&json!({ "code": 0, "data": {} }))).is_empty());
    }
}
