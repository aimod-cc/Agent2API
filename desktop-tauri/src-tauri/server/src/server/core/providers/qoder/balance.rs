//! Qoder 个人额度、个人资源包与组织资源包，统一为账号页的余额形态。
//!
//! ── 三个桶分别是什么（2026-09-27 在真实账号上实测）──────────────
//!   - `userQuota`：订阅套餐的额度（官方控制台「订阅版本的资源」，如总额 300、
//!     剩余 225）。`available` 的主口径，字段 `total/used/remaining/unit`；
//!   - `addOnQuota`：官方控制台叫「**个人资源包**」——通过活动/购买获得，
//!     每日签到每签一次发一个 100 credits、30 天有效的包（领 6 次就是 600）。
//!     **上游只在真有资源包时才返回这个键**：没有就整个键缺失（实测 Free 套餐
//!     账号如此），因此要判「有没有」，不能当成恒在的桶去读零；
//!   - `orgResourcePackage`：组织（团队）资源包。**字段形状与上面两个都不同**：
//!     总量叫 `cap`（不是 `total`）、另外多一个 `available` 布尔标它是否真的可用。
//!     实测形状（2026-10-12，国际版 Teams 账号）：
//!     `{"used":0,"remaining":0,"percentage":0,"unit":"credits","cap":0,"available":false}`
//!     ——空桶也会**整个键返回**，所以「键在不在」不能当判据；要看 `available`
//!     与 `cap`，否则会把一个还没发放的组织包画成「组织总额 0」。
//!
//! 漏掉 `addOnQuota` 正是 issue #26 的直接原因：签到领到的 600 credits 全在那个桶里，
//! 而这里此前只读 `userQuota` 与 `orgResourcePackage`，界面上一个数字都不显示。
//! 三个独立实现都是同一字段名（CLIProxyAPI 的 qoder2api 插件标「个人拓展包」、
//! CPA 的 qoder 插件、10router 的 CHANGELOG 里那条同样的修复）。
//!
//! ── `available` 为什么把资源包算进去 ────────────────────────
//! 账号页那一列回答的是「这个账号还能花多少」，而资源包里的 credits 与套餐额度
//! 一样能花（官方控制台把两者并列显示）。所以
//! `available = userQuota.remaining + addOnQuota.remaining + orgResourcePackage.remaining`
//! （组织包那一项仅在 `available:true` 时计入），钱包明细里再拆开给用户看——
//! 相加之后「签到领了 100」会立刻反映在列表上，不用展开面板才发现。
//!
//! ── 套餐名与到期 ───────────────────────────────────────────
//! 套餐名来自 `GET /api/v2/user/plan` 的 `plan_tier_name`（`Free` / `Pro Trial`），
//! 取不到就不显示（额度接口才是主链路，套餐名只是补一行）。到期的**哨兵值**见
//! [`EXPIRY_SENTINEL_MS`]。

use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

use super::credentials::Credentials;
use super::{auth, endpoints, refresh};

/// `expiresAt` 的「永不过期」哨兵值：上游给 `253402214400000`（9999-12-31）。
///
/// 原样透出会让余额面板显示「到期 9999/12/31」——那不是用户能用的信息。
/// 判据取 2100-01-01：真实订阅（含最长的年付）不可能超过它。
const EXPIRY_SENTINEL_MS: f64 = 4_102_444_800_000.0;

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse::<f64>().ok().filter(|value| value.is_finite()),
        _ => None,
    }
}

/// 一行钱包（账号页余额面板按 `displayName` + `balanceView` 渲染）。
fn wallet(kind: &str, name: &str, value: f64, unit: &str) -> Value {
    json!({
        "type": kind,
        "displayName": name,
        "balance": value,
        "balanceView": format!("{value} {unit}"),
    })
}

/// `addOnQuota` 桶本身（键在、是对象、且**不是空桶**）。
///
/// 判据只有这一份（「键缺失或全零 = 没有资源包」，见模块头）：[`add_on`] 的钱包行
/// 与 [`credit_rows`] 的个人资源包行都从这里取桶 —— 两处各判一次迟早漂成两种口径。
fn add_on_bucket(raw: &Value) -> Option<&Value> {
    let bucket = raw.get("addOnQuota").filter(|value| value.is_object())?;
    let total = number(bucket.get("total"));
    let used = number(bucket.get("used"));
    let remaining = number(bucket.get("remaining"));
    if [total, used, remaining].iter().all(|value| value.unwrap_or(0.0) <= 0.0) {
        return None;
    }
    Some(bucket)
}

/// 个人资源包（`addOnQuota`）→ 钱包行 + 剩余额度。
///
/// 没有这个键、或全零（上游会返回空对象）时给空结果 —— 与参考实现的判据一致：
/// 「全零」意味着没有资源包，而不是「有 0 个」（判据本体在 [`add_on_bucket`]）。
fn add_on(raw: &Value, fallback_unit: &str) -> (Vec<Value>, Option<f64>) {
    let Some(bucket) = add_on_bucket(raw) else {
        return (Vec::new(), None);
    };
    let total = number(bucket.get("total"));
    let used = number(bucket.get("used"));
    let remaining = number(bucket.get("remaining"));
    let unit = bucket.get("unit").and_then(Value::as_str).unwrap_or(fallback_unit);
    let mut wallets = Vec::new();
    if let Some(value) = used {
        wallets.push(wallet("addon_used", "资源包已用", value, unit));
    }
    if let Some(value) = total {
        wallets.push(wallet("addon_total", "资源包总额", value, unit));
    }
    (wallets, remaining)
}

/// `orgResourcePackage` 桶本身（键在、是对象、且上游说它**可用**）。
///
/// 判据细则见 [`org_package`] 的文档；这里与 [`add_on_bucket`] 同一取向 ——
/// 判据只写一份，钱包行与 [`credit_rows`] 的组织资源包行都从这里取桶。
fn org_bucket(raw: &Value) -> Option<&Value> {
    let bucket = raw.get("orgResourcePackage").filter(|value| value.is_object())?;
    let cap = number(bucket.get("cap")).or_else(|| number(bucket.get("total")));
    let used = number(bucket.get("used"));
    let remaining = number(bucket.get("remaining"));
    let usable = bucket.get("available").and_then(Value::as_bool).unwrap_or_else(|| {
        [cap, used, remaining].iter().any(|value| value.unwrap_or(0.0) > 0.0)
    });
    if usable { Some(bucket) } else { None }
}

/// 组织资源包（`orgResourcePackage`）→ 钱包行 + 剩余额度。
///
/// 判据与上面两桶**都不同**（见模块头）：
///   * 总量字段叫 `cap`（旧形状里叫 `total`，作兜底读）—— 此前只按 `total` 读，
///     而新响应里没有这个键，读到的恒是 `None`，组织额度永远不会出现在面板上；
///   * 空桶也**整个键返回**，所以「键在不在」不能当判据；上游用 `available`
///     表明这份额度当前是否真的可用，`false` 时整桶跳过，免得把还没发放的
///     组织包画成「组织总额 0」；
///   * `remaining` 只在可用时返回 —— 不可用的包即使带着非零 `remaining`
///     也不能算进「还能花多少」。
///
/// `available` 缺失（旧形状）时退回老判据：`cap` / `used` / `remaining`
/// 有一个大于零就显示。判据本体在 [`org_bucket`]。
fn org_package(raw: &Value, fallback_unit: &str) -> (Vec<Value>, Option<f64>) {
    let Some(bucket) = org_bucket(raw) else {
        return (Vec::new(), None);
    };
    let cap = number(bucket.get("cap")).or_else(|| number(bucket.get("total")));
    let used = number(bucket.get("used"));
    let remaining = number(bucket.get("remaining"));
    let unit = bucket.get("unit").and_then(Value::as_str).unwrap_or(fallback_unit);
    let mut wallets = Vec::new();
    if let Some(total) = cap.filter(|value| *value > 0.0) {
        wallets.push(wallet("org_total", "组织总额", total, unit));
    }
    if let Some(value) = used {
        wallets.push(wallet("org_used", "组织已用", value, unit));
    }
    (wallets, remaining)
}

/// 逐桶明细行（账号页「积分明细」弹层的输入；行形状与 workbuddy / trae 两家
/// 同一套 —— 界面只写一个读取器）。
///
/// 一行一个桶：套餐额度 → 个人资源包 → 组织资源包（与 `wallets` 的 push 顺序
/// 一致）。三个数直接透传桶上的 `total` / `used` / `remaining`，**缺失不写键**
/// （界面 `Number(null) === 0`，写 null 会把「没读到」画成「只剩 0」）。
///
/// ── 出行的判据全部与钱包行同源 ──────────────────────────────
///   * 套餐行：三个数一个都读不到时整行不出 —— 行里没有数字可读，界面只能画
///     「—」（与 ZCode 那边「纯比例窗口行跳过」同一条纪律）；一个桶都没读到
///     时 `credits` 键整个不写，界面按「键在不在」自然退回原样。
///   * 个人资源包 / 组织资源包两行：判据取 [`add_on_bucket`] / [`org_bucket`]
///     —— 「有没有这份额度」只有一份判据，见那两个函数。
///
/// ── 时刻与单位 ──────────────────────────────────────────────
///   * 不写 `expireAt` / `grantAt`：桶级没有这些字段；`subscription.expireAt`
///     是**订阅**到期，与「这一份额度什么时候作废」是两回事，不拿它顶替；
///   * `unit` 只在该桶自己的单位非空且**与文档级不同**时写（文档级取自
///     `userQuota.unit`，缺省「额度」；三桶通常同单位，多数情况都不写）。
fn credit_rows(raw: &Value, doc_unit: &str) -> Vec<Value> {
    let mut rows: Vec<Value> = Vec::new();
    if let Some(bucket) = raw.get("userQuota").filter(|value| value.is_object()) {
        if let Some(row) = credit_row(
            "套餐额度",
            number(bucket.get("total")),
            number(bucket.get("used")),
            number(bucket.get("remaining")),
            bucket.get("unit").and_then(Value::as_str),
            doc_unit,
        ) {
            rows.push(row);
        }
    }
    if let Some(bucket) = add_on_bucket(raw) {
        if let Some(row) = credit_row(
            "个人资源包",
            number(bucket.get("total")),
            number(bucket.get("used")),
            number(bucket.get("remaining")),
            bucket.get("unit").and_then(Value::as_str),
            doc_unit,
        ) {
            rows.push(row);
        }
    }
    if let Some(bucket) = org_bucket(raw) {
        if let Some(row) = credit_row(
            "组织资源包",
            // 总量字段叫 `cap`（旧形状叫 `total`，与钱包行同一兜底，见 [`org_package`]）
            number(bucket.get("cap")).or_else(|| number(bucket.get("total"))),
            number(bucket.get("used")),
            number(bucket.get("remaining")),
            bucket.get("unit").and_then(Value::as_str),
            doc_unit,
        ) {
            rows.push(row);
        }
    }
    rows
}

/// 一行明细：`name` + 三个数 + （与文档级不同的）单位；三个数一个都没有时给
/// `None`（整行不出，理由见 [`credit_rows`]）。
fn credit_row(
    name: &str,
    total: Option<f64>,
    used: Option<f64>,
    remain: Option<f64>,
    unit: Option<&str>,
    doc_unit: &str,
) -> Option<Value> {
    if total.is_none() && used.is_none() && remain.is_none() {
        return None;
    }
    let mut row = Map::new();
    row.insert("name".to_string(), Value::String(name.to_string()));
    // 上游给什么写什么（读到 0 也照写：那是真读数，不是「没读到」）；
    // 缺失一律不写这个键
    for (key, value) in [("total", total), ("used", used), ("remain", remain)] {
        if let Some(value) = value {
            row.insert(key.to_string(), json!(value));
        }
    }
    if let Some(unit) = unit.map(str::trim).filter(|value| !value.is_empty() && *value != doc_unit) {
        row.insert("unit".to_string(), Value::String(unit.to_string()));
    }
    Some(Value::Object(row))
}

/// 订阅到期时间；哨兵值、0 与 null 都按「没有到期时间」处理（不显示）。
fn expiry_of(raw: &Value) -> Option<Value> {
    let value = raw.get("expiresAt")?;
    if value.is_null() {
        return None;
    }
    if let Some(ms) = number(Some(value)) {
        if ms <= 0.0 || ms >= EXPIRY_SENTINEL_MS {
            return None;
        }
    }
    Some(value.clone())
}

/// 套餐名（`plan_tier_name`）。**best-effort**：失败只让这一行不显示，
/// 不影响额度本身（额度接口已经成功，套餐名只是补充信息）。
async fn plan_name(credentials: &Credentials, proxy: Option<&ResolvedProxy>) -> Option<String> {
    let response = auth::request(
        "GET",
        &format!("{}{}", credentials.region.open_api(), endpoints::PLAN_PATH),
        None,
        &endpoints::open_api_headers(Some(&credentials.access_token)),
        proxy,
    ).await.ok()?;
    let name = response.payload?.get("plan_tier_name")?.as_str()?.trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// 额度文档的纯函数半边：三个桶合成钱包明细、`available` 与顶层逐桶明细
/// （`credits`，见 [`credit_rows`]）。
///
/// 抽出来是为了让「组织资源包计不计入 `available`」这类判据能单测钉住——
/// 这一带的历史事故全是「明明读到了却显示成没有」，而症状在界面上，
/// 光看 `query` 的返回值很难回归。
fn usage_document(raw: &Value) -> Value {
    let quota = raw.get("userQuota");
    let remaining = quota.and_then(|quota| number(quota.get("remaining")));
    let total = quota.and_then(|quota| number(quota.get("total")));
    let used = quota.and_then(|quota| number(quota.get("used")));
    let unit = quota.and_then(|quota| quota.get("unit")).and_then(Value::as_str).unwrap_or("额度");
    let mut wallets = Vec::new();
    if let Some(value) = used {
        wallets.push(json!({ "type": "user_used", "displayName": "个人已用", "balance": value }));
    }
    if let Some(value) = total {
        wallets.push(json!({ "type": "user_total", "displayName": "个人总额", "balance": value }));
    }
    let (add_on_wallets, add_on_remaining) = add_on(raw, unit);
    wallets.extend(add_on_wallets);
    let (org_wallets, org_remaining) = org_package(raw, unit);
    wallets.extend(org_wallets);
    // 「还能花多少」= 套餐剩余 + 个人资源包剩余 + 组织资源包剩余（都能花，见模块头）
    let available = [remaining, add_on_remaining, org_remaining]
        .into_iter()
        .flatten()
        .reduce(|acc, value| acc + value);
    let credits = credit_rows(raw, unit);
    let mut document = json!({
        "available": available,
        "unit": unit,
        "wallets": wallets,
    });
    // 逐桶明细挂文档顶层（见 [`credit_rows`]）；空数组**整键不写** —— 界面按
    // 「键在不在 + 长度」决定挂不挂「积分明细」入口，写一个空数组会挂出一个
    // 点开是空的入口。
    if !credits.is_empty() {
        if let Some(object) = document.as_object_mut() {
            object.insert("credits".to_string(), Value::Array(credits));
        }
    }
    document
}

pub async fn query(
    store: &AccountStore,
    region: super::endpoints::Region,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let mut credentials = refresh::ensure_fresh(store, region, account_id, false).await?;
    let (record, _) = refresh::snapshot(store, region, account_id)?;
    let proxy = auth::account_proxy(&record)?;
    let mut response = auth::request(
        "GET",
        &format!("{}{}", credentials.region.open_api(), endpoints::USAGE_PATH),
        None,
        &endpoints::open_api_headers(Some(&credentials.access_token)),
        proxy.as_ref(),
    ).await?;
    if response.status == 401 && credentials.can_refresh() {
        credentials = refresh::ensure_fresh(store, region, account_id, true).await?;
        response = auth::request(
            "GET",
            &format!("{}{}", credentials.region.open_api(), endpoints::USAGE_PATH),
            None,
            &endpoints::open_api_headers(Some(&credentials.access_token)),
            proxy.as_ref(),
        ).await?;
    }
    let raw = auth::payload(response, "额度查询")?;
    let mut document = usage_document(&raw);
    let mut subscription = Map::new();
    if let Some(name) = plan_name(&credentials, proxy.as_ref()).await {
        subscription.insert("planName".to_string(), Value::String(name));
    }
    if let Some(value) = expiry_of(&raw) {
        subscription.insert("expireAt".to_string(), value);
    }
    if let Some(object) = document.as_object_mut() {
        object.insert("subscription".to_string(), Value::Object(subscription));
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    //! 三个桶的判据全是「显示成 0 / 不显示」那一类毛病，所以断言都钉在
    //! 字段形状与求和口径上，不钉文案。
    use super::*;

    fn wallet_by_type<'a>(document: &'a Value, kind: &str) -> Option<&'a Value> {
        document.get("wallets")?.as_array()?.iter()
            .find(|wallet| wallet.get("type").and_then(Value::as_str) == Some(kind))
    }

    /// 组织资源包的字段是 `cap` 而不是 `total`（2026-10-12 实测）。
    ///
    /// 修复前按 `total` 读：真实响应里没有这个键，读到的恒是 `None`，
    /// 组织额度永远不会出现在面板上——「明明有额度却一个数字都不显示」
    /// 的同一类事故，issue 里只会被报成「组织额度不统计」。
    #[test]
    fn the_org_package_total_is_read_from_cap_not_total() {
        let raw: Value = serde_json::from_str(r#"{
            "userQuota": {"total": 3000.0, "used": 100.0, "remaining": 2900.0, "unit": "credits"},
            "orgResourcePackage": {"used": 40.0, "remaining": 460.0, "percentage": 0.08,
                                   "unit": "credits", "cap": 500.0, "available": true}
        }"#).unwrap();
        let document = usage_document(&raw);
        assert_eq!(Some(500.0), wallet_by_type(&document, "org_total").and_then(|wallet| wallet["balance"].as_f64()));
        assert_eq!(Some(40.0), wallet_by_type(&document, "org_used").and_then(|wallet| wallet["balance"].as_f64()));
        // 组织剩余与个人额度一样能花，`available` 必须把它算进去
        assert_eq!(Some(3360.0), document["available"].as_f64(), "2900 个人 + 460 组织");
    }

    /// `available:false` 的空桶是**整个键返回**的（实测 Teams 账号如此），
    /// 键在不在不是判据；画成「组织总额 0」会让用户以为组织包被清空了。
    #[test]
    fn an_unavailable_org_package_contributes_nothing() {
        let raw: Value = serde_json::from_str(r#"{
            "userQuota": {"total": 3000.0, "used": 115.0, "remaining": 2885.0, "unit": "credits"},
            "orgResourcePackage": {"used": 0.0, "remaining": 0.0, "percentage": 0.0,
                                   "unit": "credits", "cap": 0.0, "available": false}
        }"#).unwrap();
        let document = usage_document(&raw);
        assert!(wallet_by_type(&document, "org_total").is_none(), "不可用的组织包不该出行");
        assert!(wallet_by_type(&document, "org_used").is_none());
        assert_eq!(Some(2885.0), document["available"].as_f64(), "只有个人额度");
    }

    /// 不可用的包即使带着非零 `remaining` 也不能算进「还能花多少」——
    /// 上游明确说它现在不能用，加进去就是教用户去花一笔花不掉的钱。
    #[test]
    fn an_unavailable_org_package_is_excluded_from_available_even_with_numbers() {
        let raw: Value = serde_json::from_str(r#"{
            "userQuota": {"remaining": 100.0, "unit": "credits"},
            "orgResourcePackage": {"remaining": 999.0, "cap": 999.0, "available": false}
        }"#).unwrap();
        let document = usage_document(&raw);
        assert_eq!(Some(100.0), document["available"].as_f64());
    }

    /// `available` 缺失（旧形状）时退回「有非零数就显示」的老判据，
    /// 别为了新字段把老响应读成空。
    ///
    /// 注意 `total` 兜底改的是**显示行**（`org_total`），不是 `available`——
    /// `available` 只累加各桶的 `remaining`，这份老形状里没有这个字段，
    /// 该是 `null`（「没读到」），而不是把 `total` 当成剩余量加进去。
    #[test]
    fn a_legacy_org_package_without_the_available_flag_still_shows() {
        let raw: Value = serde_json::from_str(r#"{
            "orgResourcePackage": {"total": 300.0, "used": 10.0, "unit": "credits"}
        }"#).unwrap();
        let document = usage_document(&raw);
        assert!(wallet_by_type(&document, "org_used").is_some(), "老形状也要出行");
        assert_eq!(
            Some(300.0),
            wallet_by_type(&document, "org_total").and_then(|wallet| wallet["balance"].as_f64()),
            "老形状的 total 走 cap 的兜底"
        );
        assert!(document["available"].is_null(), "没有 remaining 就没有「还能花多少」，不能拿 total 顶替");
        // 三个桶全缺时同样不伪造 0：available 是 null，界面区分「没读到」与「真的是 0」
        let empty = usage_document(&serde_json::json!({}));
        assert!(empty["available"].is_null());
    }
}
