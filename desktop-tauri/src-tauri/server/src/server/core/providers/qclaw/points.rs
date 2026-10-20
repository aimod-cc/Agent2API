//! QClaw 云端额度 / 订阅查询（腾讯 jprx 网关）。
//!
//! ── 与推理链路的分工 ─────────────────────────────────────────
//! 推理走**本机** OpenClaw 网关（`127.0.0.1:9434`，见 `local` / `mod`），
//! 而积分、订阅、流水在**云端**：QClaw 客户端自己的积分侧栏打的是
//! `https://jprx.m.qq.com/data/<命令 id>/forward`（下称 jprx）。
//! 两套凭证互不通用（网关令牌 ≠ 云端 JWT），本模块只碰云端这一套。
//!
//! ── 四个端点（2026-10-20 逆向 app.asar + 本机实测）──────────────
//! ```text
//! POST data/4110/forward   getQPointAccount            Q点余额（四桶 + 逐份积分包）
//! POST data/4426/forward   getUserSubscriptionResources 订阅资源（套餐 / 到期时刻）
//! POST data/4421/forward   queryContractStatus          合同状态（签约 / 续费）
//! POST data/4222/forward   queryQPointFlow              积分流水（逐次消耗）
//! ```
//! 余额与订阅是账号页「余额 / 最近到期」两列的数据源；流水备用（排查）。
//!
//! ── 双层信封（最容易写错的一处）────────────────────────────
//! 4110 是 `{ret, data:{resp:{common, data}}}`（多一层 `data`），
//! 其余是 `{ret, resp:{common, data}}`。所以解析顺序是固定的两步：
//! `body.data || body` → `.resp || 自身` → 验 `common.code == 0` → 取 `.data`。
//! 少走一步不会报错，只会把所有字段读成 null —— 界面上表现为「这家没有余额」。
//!
//! ── 请求头六件套（照客户端 `commonFetch` 抄的）────────────────
//! ```text
//! X-Version: 1
//! X-Token:   <leveldb 的 login 键；客户端缺省回落 "m83qdao0AmE5">
//! X-Guid:    <qclaw.json 的 sharedParams.guid>
//! X-Account: <云端 user_id>
//! X-Session: ""
//! X-OpenClaw-Token: <leveldb 的 jwt_token>
//! ```
//! body 恒带 `web_version=1.4.0` + `web_env=release`。
//!
//! ── 为什么没有「签到」端点 ───────────────────────────────────
//! 客户端全量检索（`out/` 下 115 个 chunk）没有任何 checkin / claim /
//! daily-login 类端点：QClaw 的每日积分由服务端按登录/活跃**自动发放**，
//! 客户端只在零点后重新拉一次读数（`getQPointAccount`）。
//!
//! 因此本模块**只读**：只提供余额查询（`query`），不提供任何「领取」动作。
//! 签到中心把它渲染成只读的「只读余额（已停运）」分组行（`extras.autoGrant`），
//! 界面上没有按钮 —— 一颗点了必然报错的按钮比没有更糟
//! （见 `api::checkin_center` 模块头）。
//!
//! ⚠️ QClaw **已公告停运**（2026-09-24 公告、2026-12-24 00:00 停服）：本站不再
//! 发放积分，账号内已有积分有效期不调整、可在原有效期内用完，官网另有
//! 「迁移到 WorkBuddy 领 1000 积分补贴」。这一行存在的意义是让用户**看到
//! 还剩多少、什么时候到期**，然后决定怎么处置 —— 不是让人以为每天还有到账。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本模块零 unwrap/expect/panic。

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::egress;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::local;

/// jprx 网关根（客户端 `qclawBaseUrl` 之外的转发层，实测两者都在用）。
const JPRX_BASE: &str = "https://jprx.m.qq.com/";

/// 客户端恒带的两个 body 字段（`commonFetch` 里写死的）。
const WEB_VERSION: &str = "1.4.0";
const WEB_ENV: &str = "release";

/// `X-Token` 的缺省值：客户端在 leveldb 解不出 `login` 键时用它
/// （`qclaw-api-probe` 实测带上也能过网关，只是拿不到用户态数据）。
const DEFAULT_LOGIN_KEY: &str = "m83qdao0AmE5";

/// 云端接口总超时。余额查询是用户点出来的转圈，挂住了比失败了更糟
/// （判据同 `ProviderAdapter::query_usage` 的契约第 2 条）。
const POINTS_TIMEOUT: Duration = Duration::from_secs(15);

/// 账号页「余额」列的单位（客户端界面叫「Q点 / 积分」，这里用「积分」——
/// 与别家同一中文口径，前端 `unitLabel` 认这个键）。
const UNIT: &str = "积分";

/// 「永不过期」哨兵：上游给 9999-12-31 这类值别当到期时刻透出
/// （与 `qwenwork::usage` 同一判据）。
const EXPIRY_SENTINEL_MS: i64 = 4_102_444_800_000;

/// 读数收成两位小数（整数去尾），**在服务端就做完**。
///
/// 与 `trae::usage::amount_json` 逐字同款：上游给的是高精度浮点
/// （QClaw 实测 `1984.093776`），照直透出会让每一处渲染都得自己兜底；
/// 在出口收一次，界面上就再也不会有小数尾巴。`f64` 的二进制表示决定了
/// 不能靠 `==` 判整（`1984.0` 这种先 `round` 到分再 `fract() == 0.0`）。
fn amount_json(value: f64) -> Value {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 {
        json!(rounded as i64)
    } else {
        json!(rounded)
    }
}

/// 套餐档位 → 展示名（照客户端 `qh` 映射表：
/// `sv_qclaw_set_sub_lite/pro/max` 三个 item code 分别对应标准/高级/旗舰）。
fn tier_display_name(tier: &str) -> &'static str {
    match tier {
        "lite" => "标准版",
        "pro" => "高级版",
        "max" => "旗舰版",
        _ => "免费版",
    }
}

/// 一个账号的 Q点读数（`data/4110` 归一化后）。
#[derive(Clone, Debug, Default)]
pub struct QPointAccount {
    /// 总余额（上游算好的，不自己拼）
    pub balance: f64,
    /// 每日免费积分桶
    pub daily_free: f64,
    /// 订阅积分桶
    pub subscription_q: f64,
    /// 积分包桶
    pub package_q: f64,
    /// 活动积分桶
    pub activity_q: f64,
    /// 逐份积分包（各带自己的到期时刻）
    pub items: Vec<QPointItem>,
}

/// 一份积分包（`balance_detail.items[]` 的一条）。
#[derive(Clone, Debug)]
pub struct QPointItem {
    /// 包名（如「老用户一次性回馈」）
    pub label: String,
    /// 面值
    pub total_amount: f64,
    /// 剩余
    pub remain_amount: f64,
    /// 到期时刻（毫秒；解析不出为 None）
    pub expire_at: Option<i64>,
}

/// 订阅资源（`data/4426` 的 `resource`；免费版账号上游给 null）。
#[derive(Clone, Debug, Default)]
pub struct Subscription {
    /// 档位（free / lite / pro / max）
    pub tier: String,
    /// 是否生效（上游 `status === 1`）
    pub active: bool,
    /// 到期时刻（毫秒；免费版没有）
    pub expire_at: Option<i64>,
}

/// 查一个账号的额度 / 余额 / 订阅（`ProviderAdapter::query_usage` 的实现）。
///
/// 返回账号页的统一形状：`available` / `unit` / `wallets` / `credits` /
/// `nearestExpiry` / `subscription` + `raw`（排障）。
pub async fn query(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    // 凭证是**机器级**的（本机只有一个 QClaw 登录态），account_id 只用于
    // 「这个账号还在不在」的校验；取不到记录时报错而不是静默用本机状态
    // （与 qwenwork 同一口径：点名了就要么查到要么明说）。
    if !account_id.trim().is_empty()
        && store.get_session_by_id(account_id.trim()).is_none()
    {
        return Err(GatewayError::with_status(
            404,
            "该 QClaw 账号不存在或已删除",
        ));
    }
    let session = require_cloud_session()?;

    // ① 余额：主链路，失败就是失败
    let account = fetch_qpoint_account(&session).await?;

    // ② 订阅：best-effort（免费版账号上游回 resource:null，不是错误）；
    //    挂了只让「套餐」那一格空着。
    let subscription = match fetch_subscription(&session).await {
        Ok(subscription) => Some(subscription),
        Err(error) => {
            logging::log_event(
                "warn",
                "qclaw",
                "订阅资源查询失败（套餐列将留空）",
                Some(json!({ "status": error.status_code, "message": error.message })),
            );
            None
        }
    };

    Ok(normalize(&account, subscription.as_ref()))
}

/// 取云端会话；解不开（没登录 / 解密失败）给一条用户能动手的文案。
fn require_cloud_session() -> Result<local::CloudSession, GatewayError> {
    let session = local::read_cloud_session();
    if session.jwt.trim().is_empty() {
        return Err(GatewayError::with_status(
            400,
            "未读到 QClaw 云端登录态：请确认已在 QClaw 客户端登录（积分/订阅查询需要它）",
        ));
    }
    Ok(session)
}

/// 打一次 jprx 并解开双层信封（见模块头）。
async fn jprx_call(session: &local::CloudSession, command: &str, body: Value) -> Result<Value, GatewayError> {
    let mut payload = match body {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    payload.insert("web_version".to_string(), json!(WEB_VERSION));
    payload.insert("web_env".to_string(), json!(WEB_ENV));
    let url = format!("{JPRX_BASE}data/{command}/forward");
    let client = egress::client_for_system_proxy();
    let response = client
        .post(&url)
        .timeout(POINTS_TIMEOUT)
        .header("Content-Type", "application/json")
        .header("X-Version", "1")
        .header("X-Token", non_empty_or(&session.login_key, DEFAULT_LOGIN_KEY))
        .header("X-Guid", session.guid.trim())
        .header("X-Account", session.user_id.trim())
        .header("X-Session", "")
        .header("X-OpenClaw-Token", session.jwt.trim())
        .json(&Value::Object(payload))
        .send()
        .await
        .map_err(|err| {
            GatewayError::with_status(
                502,
                format!("QClaw 云端接口不可达（{command}）：{}", egress::describe_error_detail(&err)),
            )
        })?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        logging::log_event(
            "warn",
            "qclaw",
            "云端接口返回非 2xx",
            Some(json!({ "command": command, "status": status, "body": text.chars().take(300).collect::<String>() })),
        );
        // 401/403 原样透出：调用方（批量查询）据此走「刷新后重试」
        return Err(GatewayError::with_status(
            if status == 401 || status == 403 {
                401
            } else {
                502
            },
            format!("QClaw 云端接口 {command} 返回 HTTP {status}"),
        ));
    }
    let body: Value = serde_json::from_str(&text).map_err(|_| {
        GatewayError::with_status(502, format!("QClaw 云端接口 {command} 返回的不是合法 JSON"))
    })?;
    unwrap_envelope(command, &body)
}

fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    }
}

/// 解 `{ret, data?:{resp?:{common, data}}}` 双层信封（见模块头）。
///
/// 判 `ret == 0` 与 `common.code == 0` 两道：前者是网关层（路由 / 限流），
/// 后者是业务层（登录态失效在这层报）。任一层非零都带上游文案报错。
fn unwrap_envelope(command: &str, body: &Value) -> Result<Value, GatewayError> {
    let ret = body.get("ret").and_then(Value::as_i64).unwrap_or(0);
    if ret != 0 {
        return Err(GatewayError::with_status(
            502,
            format!("QClaw 云端接口 {command} 被网关拒绝（ret={ret}）"),
        ));
    }
    let inner = body.get("data").filter(|value| value.is_object()).unwrap_or(body);
    let outer = inner.get("resp").filter(|value| value.is_object()).unwrap_or(inner);
    let code = outer.pointer("/common/code").and_then(Value::as_i64);
    if code != Some(0) {
        let message = outer
            .pointer("/common/message")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or("未知错误");
        return Err(GatewayError::with_status(
            502,
            format!("QClaw 云端接口 {command} 业务失败（code={code:?}）：{message}"),
        ));
    }
    Ok(outer.get("data").cloned().unwrap_or(Value::Null))
}

/// 拉 Q点账户（`data/4110`）。
pub async fn fetch_qpoint_account(session: &local::CloudSession) -> Result<QPointAccount, GatewayError> {
    let data = jprx_call(session, "4110", json!({})).await?;
    let number = |pointer: &str| -> f64 {
        data.pointer(pointer).and_then(Value::as_f64).unwrap_or(0.0)
    };
    let mut items = Vec::new();
    if let Some(entries) = data.pointer("/balance_detail/items").and_then(Value::as_array) {
        for entry in entries {
            let label = entry
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .unwrap_or("积分")
                .to_string();
            items.push(QPointItem {
                label,
                total_amount: entry.get("total_amount").and_then(Value::as_f64).unwrap_or(0.0),
                remain_amount: entry.get("remain_amount").and_then(Value::as_f64).unwrap_or(0.0),
                expire_at: entry
                    .get("expire_time")
                    .and_then(Value::as_str)
                    .and_then(parse_datetime_ms),
            });
        }
    }
    Ok(QPointAccount {
        balance: number("/balance"),
        daily_free: number("/balance_detail/daily_free"),
        subscription_q: number("/balance_detail/subscription_q"),
        package_q: number("/balance_detail/package_q"),
        activity_q: number("/balance_detail/activity_q"),
        items,
    })
}

/// 拉订阅资源（`data/4426`）。免费版账号上游给 `resource: null` →
/// `Ok(Subscription{tier:"free", active:false, ..})`（不是错误）。
pub async fn fetch_subscription(session: &local::CloudSession) -> Result<Subscription, GatewayError> {
    let data = jprx_call(session, "4426", json!({})).await?;
    let Some(resource) = data.get("resource").filter(|value| value.is_object()) else {
        return Ok(Subscription::default());
    };
    let tier = resource
        .get("sub_billing_item_code")
        .and_then(Value::as_str)
        .map(|code| match code {
            "sv_qclaw_set_sub_lite" => "lite",
            "sv_qclaw_set_sub_pro" => "pro",
            "sv_qclaw_set_sub_max" => "max",
            _ => "free",
        })
        .unwrap_or("free")
        .to_string();
    Ok(Subscription {
        tier,
        active: resource.get("status").and_then(Value::as_i64) == Some(1),
        // `end_effect_time` 实测是毫秒时间戳（数字）；字符串形态也收
        expire_at: resource
            .get("end_effect_time")
            .and_then(|value| value.as_i64().or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok())))
            .filter(|ms| *ms > 0),
    })
}

/// 余额 + 订阅 → 账号页统一形状（字段口径见 `ProviderAdapter::query_usage`）。
fn normalize(account: &QPointAccount, subscription: Option<&Subscription>) -> Value {
    // ── 钱包四行：行名照客户端官方弹窗的四个分类 ──────────────
    let mut wallets: Vec<Value> = Vec::new();
    let buckets = [
        ("daily_free", "每日免费积分", account.daily_free),
        ("subscription_q", "订阅积分", account.subscription_q),
        ("package_q", "积分包", account.package_q),
        ("activity_q", "活动积分", account.activity_q),
    ];
    for (kind, name, value) in buckets {
        // 数值先收两位再拼串：`format!("{value}")` 会把 f64 的原始尾巴铺开
        // （`1984.093776 积分`）。`balanceView` 是上游给的展示串的先例用法
        // （见 trae 的 `剩 N 积分`），这里让它与 `balance` 用同一个收干净的数。
        let shown = amount_json(value);
        let text = match shown.as_f64() {
            Some(number) => format!("{number} {UNIT}"),
            None => format!("0 {UNIT}"),
        };
        wallets.push(json!({
            "type": kind,
            "displayName": name,
            "balance": shown,
            "balanceView": text,
        }));
    }

    // ── 逐份积分包（「积分明细」弹层）：各带自己的到期时刻 ─────
    // 前端 `creditRowsOf` 认 `name` / `remain` / `unit` / `expireAt`。
    //
    // ── 两个与本家相关的取值纪律（跟 trae/usage.rs:816 的 `credit_rows` 同款）──
    // ① **行内不写 `unit`**：这一份的单位就是文档级 `unit`（都是「积分」），
    //    再写一遍会让「最近到期积分」列拼出「1984.09 积分」—— 而各家该列只有数字
    //    （trae 的规则是「`unit != doc_unit` 才写行内 unit」，见其函数头）。
    //    行内 `unit` 只为**混单位**的家保留（Trae 的套餐包是「额度」那次）。
    // ② **数值就地收成两位小数**：上游给的是高精度浮点（实测 1984.093776），
    //    明细弹层与签到中心的读数都走 `numberText` 兜底，但真值是 `f64`，
    //    界面上任何一处漏了格式化就会把小数尾巴铺开。trae 用 `amount_json`
    //    在**服务端**就四舍五入（`(value * 100).round() / 100` 且整数去尾），
    //    这里照同一纪律把读数收干净 —— 与 `unit` 一起构成「行形状逐键一致」。
    let mut credits: Vec<Value> = Vec::new();
    for item in &account.items {
        let mut row = Map::new();
        row.insert("name".to_string(), json!(item.label));
        row.insert("remain".to_string(), amount_json(item.remain_amount));
        row.insert("total".to_string(), amount_json(item.total_amount));
        if let Some(expire_at) = item.expire_at {
            row.insert("expireAt".to_string(), json!(expire_at));
        }
        credits.push(Value::Object(row));
    }

    // ── 最近到期：items[] 里最早作废的那一份（与「最近到期积分」两列同源）
    let nearest = account
        .items
        .iter()
        .filter_map(|item| item.expire_at.map(|at| (at, item.remain_amount)))
        .min_by_key(|(at, _)| *at);

    let mut document = Map::new();
    document.insert("provider".to_string(), json!("qclaw"));
    // 三个主读数都在出口收成两位（与 wallets / credits 同一纪律）：上游给高精度
    // 浮点，任何一处漏了格式化都会把小数尾巴带进界面。
    document.insert("available".to_string(), amount_json(account.balance));
    document.insert("unit".to_string(), json!(UNIT));
    document.insert("wallets".to_string(), Value::Array(wallets));
    if !credits.is_empty() {
        document.insert("credits".to_string(), Value::Array(credits));
    }
    if let Some((at, remain)) = nearest {
        document.insert("nearestExpiry".to_string(), json!(at));
        document.insert("nearestExpiryCredits".to_string(), amount_json(remain));
    }
    // 套餐：免费版也给一行（planName=免费版），免得界面把「没有订阅」
    // 显示成「查不到套餐」—— 两者对用户是同一个状态，但成因不同。
    let tier = subscription.map(|s| s.tier.as_str()).unwrap_or("free");
    let mut plan = Map::new();
    plan.insert("planName".to_string(), json!(tier_display_name(tier)));
    if let Some(subscription) = subscription {
        if let Some(expire_at) = subscription.expire_at.filter(|ms| *ms < EXPIRY_SENTINEL_MS) {
            plan.insert("expireAt".to_string(), json!(expire_at));
        }
    }
    document.insert("subscription".to_string(), Value::Object(plan));
    // 原始读数（排障用；前端默认不展示）。手写而不 `#[derive(Serialize)]`：
    // 这两个结构体是内部形态，序列化一次读数的代价不值得为它开 derives。
    let items: Vec<Value> = account
        .items
        .iter()
        .map(|item| {
            json!({
                "label": item.label,
                "total_amount": item.total_amount,
                "remain_amount": item.remain_amount,
                "expire_at": item.expire_at,
            })
        })
        .collect();
    document.insert(
        "raw".to_string(),
        json!({
            "balance": account.balance,
            "daily_free": account.daily_free,
            "subscription_q": account.subscription_q,
            "package_q": account.package_q,
            "activity_q": account.activity_q,
            "items": items,
            "tier": tier,
        }),
    );
    Value::Object(document)
}

/// 解 QClaw 的日期文案 → 毫秒时间戳。
///
/// 两种形态都实测存在：积分包 `expire_time` 是 `2026-11-17 11:10:29`
/// （空格分隔、无时区 —— 按北京时间 UTC+8 算），`updated_at` 一类是
/// RFC3339（`2026-05-10T10:44:39+08:00`，带时区）。
/// 判据与 `qwenwork::auth::parse_iso8601_ms` 同源：只认前缀、手算不做
/// 第三方日期库依赖；无时区时按 +08:00（客户端是腾讯国内产品）。
fn parse_datetime_ms(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, rest) = if let Some((date, rest)) = text.split_once('T') {
        (date, rest)
    } else {
        text.split_once(' ')?
    };
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // 时区：显式带了（Z / ±HH:MM）就按它算；没带按北京时间
    let (mut hms, mut tz_offset_sec) = (rest, 8 * 3600i64);
    if let Some(idx) = rest.find(['Z', 'z']) {
        hms = &rest[..idx];
        tz_offset_sec = 0;
    } else if rest.len() > 6 {
        // 形如 `10:44:39+08:00`：最后一个 +/- 之后是时区
        if let Some(idx) = rest.rfind(['+', '-']) {
            let (time, tz) = rest.split_at(idx);
            hms = time;
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let mut parts = tz[1..].split(':');
            let hours: i64 = parts.next().and_then(|value| value.parse().ok()).unwrap_or(0);
            let minutes: i64 = parts.next().and_then(|value| value.parse().ok()).unwrap_or(0);
            tz_offset_sec = sign * (hours * 3600 + minutes * 60);
        }
    }
    let mut time_parts = hms.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next().unwrap_or("0").parse().ok()?;
    let second_text = time_parts.next().unwrap_or("0");
    let second: i64 = second_text.split('.').next()?.parse().ok()?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=60).contains(&second) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let secs = days * 86400 + hour * 3600 + minute * 60 + second - tz_offset_sec;
    Some(secs * 1000)
}

/// 公历 → 天数（Howard Hinnant 的 days_from_civil，与 qwenwork 同一实现）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 双层信封：4110 形态（多一层 data）与其余端点形态都要认。
    #[test]
    fn envelope_unwraps_both_shapes() {
        let with_data_layer = json!({
            "ret": 0,
            "data": { "resp": { "common": { "code": 0, "message": "Success" }, "data": { "balance": 12.5 } } }
        });
        assert_eq!(unwrap_envelope("4110", &with_data_layer).unwrap(), json!({ "balance": 12.5 }));
        let flat = json!({
            "ret": 0,
            "resp": { "common": { "code": 0 }, "data": { "resource": null } }
        });
        assert_eq!(unwrap_envelope("4426", &flat).unwrap(), json!({ "resource": null }));
    }

    /// 网关层 ret 非零与业务层 code 非零都要报错（带上游文案）。
    #[test]
    fn envelope_rejects_nonzero_codes() {
        let gateway_error = json!({ "ret": 1001 });
        assert!(unwrap_envelope("4110", &gateway_error).is_err());
        let business_error = json!({ "ret": 0, "resp": { "common": { "code": 10001, "message": "登录态失效" }, "data": null } });
        let error = unwrap_envelope("4110", &business_error).err().expect("must fail");
        assert!(error.message.contains("登录态失效"), "文案要带上游的：{}", error.message);
    }

    /// 无时区按北京时间、带时区按声明算（两种形态都实测存在）。
    #[test]
    fn datetime_parses_both_forms() {
        // 2026-11-17 11:10:29（+08:00）= 1794885029000
        assert_eq!(
            parse_datetime_ms("2026-11-17 11:10:29"),
            Some(1_794_885_029_000)
        );
        assert_eq!(
            parse_datetime_ms("2026-05-10T10:44:39+08:00"),
            Some(1_778_381_079_000)
        );
        // 同一时刻的 UTC 形态（Z）必须得到同一个数
        assert_eq!(
            parse_datetime_ms("2026-05-10T02:44:39Z"),
            Some(1_778_381_079_000)
        );
        assert_eq!(parse_datetime_ms("not a date"), None);
    }

    /// 四桶 + 逐份包 → 统一形状：available 取上游 balance（不自己拼），
    /// 最近到期取 items 里最早的那一份。
    #[test]
    fn normalize_emits_unified_shape() {
        let account = QPointAccount {
            balance: 1984.093776,
            daily_free: 0.0,
            subscription_q: 0.0,
            package_q: 0.0,
            activity_q: 1984.093776,
            items: vec![QPointItem {
                label: "老用户一次性回馈".to_string(),
                total_amount: 2000.0,
                remain_amount: 1984.093776,
                expire_at: Some(1_794_885_029_000),
            }],
        };
        let document = normalize(&account, None);
        // 三个主读数都在出口收成两位（上游给的是高精度浮点）
        assert_eq!(document["available"], json!(1984.09));
        assert_eq!(document["unit"], json!("积分"));
        let wallets = document["wallets"].as_array().expect("wallets");
        assert_eq!(wallets.len(), 4);
        assert_eq!(wallets[0]["displayName"], json!("每日免费积分"));
        assert_eq!(wallets[3]["displayName"], json!("活动积分"));
        // 钱包读数与展示串都收干净（`balanceView` 不铺小数尾巴）
        assert_eq!(wallets[3]["balance"], json!(1984.09));
        assert_eq!(wallets[3]["balanceView"], json!("1984.09 积分"));
        assert_eq!(wallets[0]["balanceView"], json!("0 积分"));
        let credits = document["credits"].as_array().expect("credits");
        assert_eq!(credits.len(), 1);
        assert_eq!(credits[0]["name"], json!("老用户一次性回馈"));
        assert_eq!(credits[0]["expireAt"], json!(1_794_885_029_000i64));
        // 行内**不带** unit（单位在文档级，见 normalize 的说明）：带了就会让
        // 账号表「最近到期积分」列拼出「1984.09 积分」，而各家该列只有数字
        assert!(credits[0].get("unit").is_none(), "行内不该写 unit：{credits:?}");
        // 读数在服务端就收成两位（与 trae::usage::amount_json 同款）
        assert_eq!(credits[0]["remain"], json!(1984.09));
        assert_eq!(credits[0]["total"], json!(2000));
        assert_eq!(document["nearestExpiry"], json!(1_794_885_029_000i64));
        assert_eq!(document["nearestExpiryCredits"], json!(1984.09));
        assert_eq!(document["subscription"]["planName"], json!("免费版"));
    }

    /// 读数收两位：整数去尾、小数留到分（与 `trae::usage::amount_json` 同一判据）。
    #[test]
    fn amount_json_rounds_to_cents() {
        assert_eq!(amount_json(1984.093776), json!(1984.09));
        assert_eq!(amount_json(2000.0), json!(2000));
        assert_eq!(amount_json(0.0), json!(0));
        // 四舍五入不是截断
        assert_eq!(amount_json(1.239), json!(1.24));
        assert_eq!(amount_json(97.43000030517578), json!(97.43));
    }

    /// 订阅档位的 item code → tier → 展示名（照客户端映射表）。
    #[test]
    fn tier_maps_item_codes() {
        let resource = json!({
            "sub_billing_item_code": "sv_qclaw_set_sub_pro",
            "status": 1,
            "end_effect_time": 1799702400000i64,
        });
        let data = json!({ "resource": resource });
        // 直接喂解析逻辑（避免为它再打一次网络）
        let tier = data
            .pointer("/resource/sub_billing_item_code")
            .and_then(Value::as_str)
            .map(|code| match code {
                "sv_qclaw_set_sub_lite" => "lite",
                "sv_qclaw_set_sub_pro" => "pro",
                "sv_qclaw_set_sub_max" => "max",
                _ => "free",
            })
            .unwrap_or("free");
        assert_eq!(tier, "pro");
        assert_eq!(tier_display_name("pro"), "高级版");
        assert_eq!(tier_display_name("max"), "旗舰版");
        assert_eq!(tier_display_name("free"), "免费版");
    }
}
