//! Qoder 个人额度、个人资源包与组织资源包，统一为账号页的余额形态。
//!
//! ── 三个桶分别是什么（2026-09-27 在真实账号上实测）──────────────
//!   - `userQuota`：订阅套餐的额度（官方控制台「订阅版本的资源」，如总额 300、
//!     剩余 225）。`available` 的主口径，字段 `total/used/remaining/unit`；
//!   - `addOnQuota`：官方控制台叫「**个人资源包**」——通过活动/购买获得，
//!     每日签到每签一次发一个 100 credits、30 天有效的包（领 6 次就是 600）。
//!     **上游只在真有资源包时才返回这个键**：没有就整个键缺失（实测 Free 套餐
//!     账号如此），因此要判「有没有」，不能当成恒在的桶去读零；
//!   - `orgResourcePackage`：组织（团队）资源包，字段形状与上面两个不同。
//!
//! 漏掉 `addOnQuota` 正是 issue #26 的直接原因：签到领到的 600 credits 全在那个桶里，
//! 而这里此前只读 `userQuota` 与 `orgResourcePackage`，界面上一个数字都不显示。
//! 三个独立实现都是同一字段名（CLIProxyAPI 的 qoder2api 插件标「个人拓展包」、
//! CPA 的 qoder 插件、10router 的 CHANGELOG 里那条同样的修复）。
//!
//! ── `available` 为什么把资源包算进去 ────────────────────────
//! 账号页那一列回答的是「这个账号还能花多少」，而资源包里的 credits 与套餐额度
//! 一样能花（官方控制台把两者并列显示）。所以
//! `available = userQuota.remaining + addOnQuota.remaining`，钱包明细里再拆开给用户看——
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

/// 个人资源包（`addOnQuota`）→ 钱包行 + 剩余额度。
///
/// 没有这个键、或全零（上游会返回空对象）时给空结果 —— 与参考实现的判据一致：
/// 「全零」意味着没有资源包，而不是「有 0 个」。
fn add_on(raw: &Value, fallback_unit: &str) -> (Vec<Value>, Option<f64>) {
    let Some(bucket) = raw.get("addOnQuota").filter(|value| value.is_object()) else {
        return (Vec::new(), None);
    };
    let total = number(bucket.get("total"));
    let used = number(bucket.get("used"));
    let remaining = number(bucket.get("remaining"));
    if [total, used, remaining].iter().all(|value| value.unwrap_or(0.0) <= 0.0) {
        return (Vec::new(), None);
    }
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
    let (add_on_wallets, add_on_remaining) = add_on(&raw, unit);
    wallets.extend(add_on_wallets);
    if let Some(package) = raw.get("orgResourcePackage") {
        if let Some(total) = number(package.get("total")).filter(|value| *value > 0.0) {
            let package_unit = package.get("unit").and_then(Value::as_str).unwrap_or(unit);
            wallets.push(json!({ "type": "org_total", "displayName": "组织总额",
                "balance": total, "balanceView": format!("{total} {package_unit}") }));
            if let Some(used) = number(package.get("used")) {
                wallets.push(json!({ "type": "org_used", "displayName": "组织已用",
                    "balance": used, "balanceView": format!("{used} {package_unit}") }));
            }
        }
    }
    // 「还能花多少」= 套餐剩余 + 资源包剩余（两者都能花，见模块头）
    let available = match (remaining, add_on_remaining) {
        (Some(base), Some(extra)) => Some(base + extra),
        (Some(base), None) => Some(base),
        (None, Some(extra)) => Some(extra),
        (None, None) => None,
    };
    let mut subscription = Map::new();
    if let Some(name) = plan_name(&credentials, proxy.as_ref()).await {
        subscription.insert("planName".to_string(), Value::String(name));
    }
    if let Some(value) = expiry_of(&raw) {
        subscription.insert("expireAt".to_string(), value);
    }
    Ok(json!({
        "available": available,
        "unit": unit,
        "wallets": wallets,
        "subscription": subscription,
    }))
}
