//! 千问办公的额度 / 余额 / 套餐查询。
//!
//! 结构照 `qoder/balance.rs`（钱包三联：`available` + `wallets` + `credits`），
//! 但**形状按千问办公实测重写** —— 两家上游的字段名完全不同，只能抄结构不能抄字段。
//!
//! ── 三个上游接口，各管一件事（别指望一个接口给全）──────────────
//! ```text
//! GET {gateway}/api/v2/quota/usage                       用量占比（**无信封**）
//! GET {gateway}/api/v1/adapter/user/wallets              余额 / 最近到期 / 最近到期积分
//! GET {gateway}/api/v1/adapter/user/account-context
//!       ?include=user,plan,quota,page                    套餐名 + 有效期 + 余额主口径
//! ```
//!
//! ── ★ `adapter` 族外面包着 `{"code":"ok","data":{...}}` ────────────
//! `quota/usage` 与 `userinfo` 是**裸** JSON，而 `adapter` 族带信封。这是最容易
//! 写错的一处：直接读顶层不会报错，只会把所有字段读成 `null`，界面上表现为
//! 「这家没有余额」—— 与「上游真没给」长得一模一样。所以 [`unwrap`] 是必过的一道。
//!
//! ── 钱包实测形状（2026-10-19，`data` 段）────────────────────────
//! ```json
//! {"active_wallets": {"wallets": [{"balance": 97.43,  "valid_to": "2026-10-20T00:00:00+08:00"},
//!                                 {"balance": 1641.25,"valid_to": "2026-11-28T21:19:24+08:00"}],
//!                     "total": 2, "page_size": 20, "page_number": 1},
//!  "daily_credits":    {"total_balance": 97.43},
//!  "longterm_credits": {"total_balance": 1641.25},
//!  "monthly_credits":  {"total_balance": 0},
//!  "expiring_soon":    {"count": 0, "total_balance": 0, "wallets": []}}
//! ```
//!
//! ── `available` 取谁：`quota.remaining`，不是自己相加 ──────────────
//! 实测 `account-context.data.quota.remaining = 1738.6854`，而
//! `daily_credits.total_balance + longterm_credits.total_balance`
//! `= 97.4313 + 1641.2541 = 1738.6854` —— **两者相等**，说明上游已经把余额算好了。
//! 既然如此就取它（上游口径永远比我们拼的准，比如它以后加新桶我们不必跟），
//! 相加只作为 `quota` 段缺失时的回落。
//!
//! ── 「最近到期」优先用上游算好的 `expiring_soon` ──────────────────
//! 上游自己给了 `expiring_soon` 桶（含 `count` / `total_balance` / `wallets`）——
//! 那是它认为「即将到期」的那一份。自己从 `active_wallets.wallets[]` 挑 `valid_to`
//! 最早的那项只作回落（上游 `expiring_soon` 为空但 `active_wallets` 有数据时）。
//!
//! ── 三个接口互相**不阻塞** ───────────────────────────────────
//! 用量是主链路（进度条），钱包与套餐是补充列。任何一个失败只让对应那几列空着，
//! 不该让「用量明明查到了却整页报错」—— 因此后两个都是 best-effort，失败只记日志。
//!
//! ── 鉴权：用 JWT，不是 MaaS 密钥（别搞混）──────────────────────
//! 三个接口都在 `gateway.qwenwork.cn` 上，认登录态 JWT；MaaS 密钥只能打
//! `maas.xiaoluozi.cn` 的推理接口（实测互不通用，见 `auth.rs` 的模块头）。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{auth, endpoints};

/// 「永不过期」哨兵：上游给 9999-12-31 这类值时别原样透出，
/// 界面上「到期 9999/12/31」不是用户能用的信息。判据取 2100-01-01。
const EXPIRY_SENTINEL_MS: i64 = 4_102_444_800_000;

/// 查询某账号的额度 / 余额 / 套餐。
///
/// 返回形状对齐账号页的统一契约：`used` / `total` / `remaining` / `percent` /
/// `resetAt`（进度条）+ `available` / `unit` / `wallets` / `credits`（余额面板）
/// + `subscription`（套餐名与有效期）。
pub async fn query(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    // 额度是**账号级**的：按 id 直取，取不到就报错而不是回落队首
    // （否则会把「这个账号查不到」显示成「当前账号的额度」）。
    let session = if account_id.trim().is_empty() {
        store.current_entry_for_provider("qwenwork").map(|e| e.session)
    } else {
        store.get_session_by_id(account_id).map(|e| e.session)
    };
    let session = session.ok_or_else(|| {
        GatewayError::with_status(401, "没有可用的千问办公账号，请先在「账号」页添加")
    })?;
    let jwt = session
        .get("auth")
        .and_then(|a| a.get("accessToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            GatewayError::with_status(401, "千问办公账号缺少登录令牌，请重新从本机导入")
        })?;

    // ── ① 套餐 + 余额主口径（先打这个：它同时给出 plan 与 quota）──
    // best-effort：这一路挂了只让余额/套餐那几列空着。
    let include = endpoints::ACCOUNT_CONTEXT_INCLUDE;
    let context = match get_json(jwt, endpoints::ACCOUNT_CONTEXT_PATH, Some(include)).await {
        Ok(body) => Some(unwrap(&body).clone()),
        Err(error) => {
            logging::log_event(
                "warn",
                "qwenwork",
                "账号上下文查询失败（套餐与余额列将留空）",
                Some(json!({ "status": error.status_code, "message": error.message })),
            );
            None
        }
    };

    // ── ② 用量：主链路，失败就是失败 ──────────────────────────
    let usage_body = get_json(jwt, endpoints::QUOTA_USAGE_PATH, None).await?;
    let mut document = normalize(&usage_body);

    // ── ③ 钱包：最近到期 / 最近到期积分 / 逐桶明细 ─────────────
    let wallet = match get_json(jwt, endpoints::WALLETS_PATH, None).await {
        Ok(body) => Some(unwrap(&body).clone()),
        Err(error) => {
            logging::log_event(
                "warn",
                "qwenwork",
                "钱包查询失败（最近到期列将留空）",
                Some(json!({ "status": error.status_code, "message": error.message })),
            );
            None
        }
    };

    // 钱包与上下文合流（都是 `余额面板` 的数据源，合成一份文档）
    merge_balance(&mut document, wallet.as_ref(), context.as_ref());

    Ok(document)
}

/// 一个带 JWT 的 GET，返回解析后的 JSON（**未解信封**）。
///
/// 非 2xx 一律记下状态码与响应片段再抛 —— 只回「接口返回 HTTP xxx」时
/// 排查要重新抓包，而这条链路上 401（令牌过期）与 404（路径写错）的处置
/// 完全不同，必须分得开。
async fn get_json(jwt: &str, path: &str, include: Option<&str>) -> Result<Value, GatewayError> {
    let mut url = endpoints::gateway_url(path);
    if let Some(include) = include {
        url.push_str(if path.contains('?') { "&include=" } else { "?include=" });
        url.push_str(include);
    }
    let client = crate::server::core::egress::client_for_system_proxy();
    let response = client
        .get(&url)
        .timeout(super::CONTROL_TIMEOUT)
        .header("Authorization", format!("Bearer {jwt}"))
        .header("Accept", "application/json")
        .header("User-Agent", super::user_agent())
        .send()
        .await
        .map_err(|err| {
            GatewayError::with_status(502, format!("千问办公接口不可达（{path}）：{err}"))
        })?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        logging::log_event(
            "warn",
            "qwenwork",
            "上游接口返回非 2xx",
            Some(json!({ "path": path, "status": status, "body": super::truncate(&text, 300) })),
        );
        return Err(GatewayError::with_status(
            if status == 401 { 401 } else { 502 },
            format!("千问办公接口 {path} 返回 HTTP {status}"),
        ));
    }
    serde_json::from_str(&text).map_err(|_| {
        GatewayError::with_status(502, format!("千问办公接口 {path} 返回的不是合法 JSON"))
    })
}

/// 解 `{"code":"ok","data":{...}}` 信封。
///
/// ── 为什么只有 `adapter` 族需要它 ──────────────────────────
/// `quota/usage` 与 `userinfo` 是裸 JSON，`adapter` 族带信封（见模块头）。
/// 这里**不判 `code`** —— 非 `ok` 时上游同时也回非 2xx，`get_json` 已经拦下了；
/// 如果哪天它回 200 + 非 ok，那是上游语义变化，届时该显式报错而不是静默吞掉。
/// 现在只做「有 `data` 就用 `data`，没有就当裸 JSON 用」这一件事，
/// 于是同一个函数对两种接口都成立。
fn unwrap(body: &Value) -> &Value {
    body.get("data").filter(|data| data.is_object()).unwrap_or(body)
}

/// 把上游额度响应归一成账号页认的形状。
///
/// ── 两种上游形态都认 ───────────────────────────────────────
///   1. **百分比形态**（免费版实测）：`total_usage_percentage` 有值、
///      `user_quota` 为 null → 只报百分比；
///   2. **结构化形态**（付费版预期）：`user_quota` 是对象，从里面取
///      `total` / `used` / `remaining` / `reset_at`。
fn normalize(body: &Value) -> Value {
    let exceeded = body
        .get("is_quota_exceeded")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let percent = body
        .get("total_usage_percentage")
        .and_then(Value::as_f64)
        .map(|p| p.clamp(0.0, 1000.0));

    let quota = body.get("user_quota").filter(|v| v.is_object());
    let pick = |keys: &[&str]| -> Option<f64> {
        let quota = quota?;
        for key in keys {
            if let Some(n) = quota.get(*key).and_then(Value::as_f64) {
                return Some(n);
            }
        }
        None
    };
    let total = pick(&["total", "total_quota", "limit", "quota"]);
    let used = pick(&["used", "used_quota", "usage"]);
    let remaining = pick(&["remaining", "remaining_quota", "left"]);
    let reset_at = quota
        .and_then(|q| q.get("reset_at").or_else(|| q.get("resetAt")))
        .and_then(Value::as_i64)
        .filter(|n| *n > 0);

    // 结构化缺失时用百分比反推 used/total（拿 100 当总量，只为了画进度条）
    let (total, used, remaining) = match (total, used, remaining) {
        (Some(t), u, r) => (Some(t), u, r),
        _ => match percent {
            Some(p) => (Some(100.0), Some(p), Some((100.0 - p).max(0.0))),
            None => (None, None, None),
        },
    };

    json!({
        "provider": "qwenwork",
        "userType": body.get("user_type").cloned().unwrap_or(Value::Null),
        "used": used,
        "total": total,
        "remaining": remaining,
        "percent": percent,
        "exceeded": exceeded,
        "resetAt": reset_at,
        // 原样保留上游响应，便于账号页展示细节与排查
        "raw": body,
    })
}

/// 钱包 + 账号上下文 → `available` / `unit` / `wallets` / `credits` /
/// `nearestExpiry` / `subscription`。
///
/// 两个来源都可能是 `None`（对应接口失败），逐项缺席地合 —— 谁到了就写谁。
fn merge_balance(document: &mut Value, wallet: Option<&Value>, context: Option<&Value>) {
    // 单位用中文「积分」（不是上游的 `credits`）：这一串会直接进界面读数
    // （「可用 1,741.25 积分」与明细里每一行的「剩 x 积分」），中文界面里
    // 冒出一个英文口径名是要用户在两套词汇之间翻译。前端 `unitLabel`
    // 认的就是 `积分` / `额度` 两个中文键（其余取值原样透出）。
    let unit = "积分";

    // ── 余额主口径：`quota.remaining`（见模块头），缺失时用两个桶相加回落
    let quota_remaining = context
        .and_then(|ctx| ctx.pointer("/quota/remaining"))
        .and_then(Value::as_f64);
    let wallet_daily = wallet
        .and_then(|w| w.pointer("/daily_credits/total_balance"))
        .and_then(Value::as_f64);
    let wallet_longterm = wallet
        .and_then(|w| w.pointer("/longterm_credits/total_balance"))
        .and_then(Value::as_f64);
    let available = quota_remaining.or_else(|| {
        match (wallet_daily, wallet_longterm) {
            (Some(d), Some(l)) => Some(d + l),
            (Some(d), None) => Some(d),
            (None, Some(l)) => Some(l),
            _ => None,
        }
    });

    // ── 钱包行（账号页余额面板按 type + displayName + balance 渲染）──
    // 行名照**上游官方口径**（客户端余额弹窗里就叫「每日刷新积分 / 其他积分」）：
    // 界面用词与官方页面对不上时，用户要自己在两套词汇之间翻译 —— 没有这个必要。
    let mut wallets: Vec<Value> = Vec::new();
    if let Some(value) = wallet_daily {
        wallets.push(wallet_row("daily", "每日刷新积分", value, unit));
    }
    if let Some(value) = wallet_longterm {
        wallets.push(wallet_row("longterm", "其他积分", value, unit));
    }
    if let Some(value) = wallet
        .and_then(|w| w.pointer("/monthly_credits/total_balance"))
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0)
    {
        wallets.push(wallet_row("monthly", "包月积分", value, unit));
    }
    if let Some(used) = context
        .and_then(|ctx| ctx.pointer("/quota/used"))
        .and_then(Value::as_f64)
    {
        wallets.push(wallet_row("used", "已用", used, unit));
    }

    // ── 逐桶明细（「积分明细」弹层）：两行各带到期时刻，且标 daily ──
    // 前端 `creditRowsOf` 认 `name` / `remain` / `expireAt`（毫秒）/ `daily`；
    // 行内**不写 `unit`**（理由见 `credit_row` 的说明）。
    // 行名照上游官方弹窗的两个分类（「每日刷新积分」每天 0 点回满、「其他积分」
    // 通过充值或活动获取），与 `wallets[]` 的 displayName 同一套词汇。
    let mut credits: Vec<Value> = Vec::new();
    if let Some(row) = credit_row("每日刷新积分", wallet_daily, daily_expiry(wallet), true) {
        credits.push(row);
    }
    if let Some(row) = credit_row("其他积分", wallet_longterm, longterm_expiry(wallet), false) {
        credits.push(row);
    }

    // ── 最近到期：优先上游算好的 `expiring_soon`，回落自己挑 ──────
    let (nearest_at, nearest_amount) = nearest_expiry(wallet);

    let Some(object) = document.as_object_mut() else {
        return;
    };
    if let Some(value) = available {
        // 主读数在出口收两位（与 wallets / credits 同一纪律）：上游给高精度浮点
        object.insert("available".to_string(), amount_json(value));
    }
    object.insert("unit".to_string(), json!(unit));
    if !wallets.is_empty() {
        object.insert("wallets".to_string(), Value::Array(wallets));
    }
    if !credits.is_empty() {
        object.insert("credits".to_string(), Value::Array(credits));
    }
    if let Some(at) = nearest_at {
        object.insert("nearestExpiry".to_string(), json!(at));
        // 与小结里其余字段同一条规矩：**缺失不写键**，不写 `null`
        // （前端按「有没有这个键」判空，写 null 会被画成 0）。
        // `nearest_expiry` 两值成对出现，走到这里 amount 必然有值；
        // 用 `if let` 而非打平，是给将来上游只给时刻的那种形态留正确行为。
        if let Some(amount) = nearest_amount {
            object.insert("nearestExpiryCredits".to_string(), amount_json(amount));
        }
    }
    // 套餐名与到期（`plan.name` / `plan.next_due_date`）
    object.insert("subscription".to_string(), subscription_of(context));
    // 原始响应：界面上「最近到期」的备选口径与排查都靠它
    if let Some(wallet) = wallet {
        object.insert("walletRaw".to_string(), wallet.clone());
    }
    if let Some(context) = context {
        object.insert("contextRaw".to_string(), context.clone());
    }
}

/// 一行钱包（账号页余额面板按 `type` + `displayName` + `balance` 渲染）。
///
/// 数值走 `amount_json` 收两位再拼串：上游给高精度浮点（`97.43000030517578`），
/// 直接 `format!("{value}")` 会把尾巴铺进展示串。
fn wallet_row(kind: &str, name: &str, value: f64, unit: &str) -> Value {
    let shown = amount_json(value);
    let text = match shown.as_f64() {
        Some(number) => format!("{number} {unit}"),
        None => format!("0 {unit}"),
    };
    json!({
        "type": kind,
        "displayName": name,
        "balance": shown,
        "balanceView": text,
    })
}

/// 一行积分明细；`remain` 读不到就整行不出（界面只能画「—」）。
///
/// ── 为什么不写行内 `unit`（踩过）─────────────────────────────
/// 本家只有一种单位（`DOC_UNIT`，见下），而账号表的「最近到期积分」列会把
/// 行内 `unit` 直接拼在数字后面 —— 写了就变成「1984.09 积分」，而别家这一列
/// 是纯数字。trae 的 `credit_rows` 早就把这个约定写进契约了：
/// **只有 `unit != doc_unit` 才写行内 `unit`**（那一条是为混单位的套餐包留的）。
/// 本家不属于那一档，因此不写 —— 界面上的单位由余额列 / 明细弹层统一交代。
fn credit_row(
    name: &str,
    remain: Option<f64>,
    expire_at: Option<i64>,
    daily: bool,
) -> Option<Value> {
    let remain = remain?;
    let mut row = Map::new();
    row.insert("name".to_string(), json!(name));
    // 读数收成两位小数（整数去尾），与 `trae::usage::amount_json` 同款：
    // 上游给的是高精度浮点（`97.43000030517578` 这种），照直透出会让每一处
    // 渲染都得自己兜底。在出口收一次，界面上就再也不会有小数尾巴。
    row.insert("remain".to_string(), amount_json(remain));
    // 前端 `creditRowsOf` 的 `expiryMillis` 同时收秒与毫秒，这里统一发毫秒
    if let Some(expire_at) = expire_at {
        row.insert("expireAt".to_string(), json!(expire_at));
    }
    if daily {
        row.insert("daily".to_string(), json!(true));
    }
    Some(Value::Object(row))
}

/// 读数收成两位小数（整数去尾），**在服务端就做完**。
///
/// 与 `trae::usage::amount_json` 逐字同款（两处是同一份实现，下游同步上游时
/// 若上游合并其一，另一处可随之收敛）。`f64` 不能靠 `==` 判整：先把分位
/// `round` 上来再 `fract() == 0.0` 才是稳的。
fn amount_json(value: f64) -> Value {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 {
        json!(rounded as i64)
    } else {
        json!(rounded)
    }
}

/// 「每日刷新积分」那份的到期时刻 —— `active_wallets.wallets[]` 里最早的一项。
///
/// 上游没给每日刷新积分单独一个到期字段（`daily_credits` 只有 `total_balance`），
/// 而实测里额度分两份：面值 97.43 的那份 `valid_to` 是今天 24 点，正是每日那份。
/// 所以按「金额相等」认领 —— 比按顺序取第 0 项稳（顺序没有契约保证）。
fn daily_expiry(wallet: Option<&Value>) -> Option<i64> {
    let daily = wallet
        .and_then(|w| w.pointer("/daily_credits/total_balance"))
        .and_then(Value::as_f64)?;
    let entries = wallet_entries(wallet);
    entries
        .iter()
        .find(|(_, balance)| (*balance - daily).abs() < 1e-6)
        .and_then(|(at, _)| *at)
}

/// 「其他积分」那份的到期时刻 —— 剩下的那一项（金额与其他积分相等）。
fn longterm_expiry(wallet: Option<&Value>) -> Option<i64> {
    let longterm = wallet
        .and_then(|w| w.pointer("/longterm_credits/total_balance"))
        .and_then(Value::as_f64)?;
    let entries = wallet_entries(wallet);
    entries
        .iter()
        .find(|(_, balance)| (*balance - longterm).abs() < 1e-6)
        .and_then(|(at, _)| *at)
}

/// `active_wallets.wallets[]` → `[(到期毫秒, 余额)]`（日期解析不了的项跳过）。
fn wallet_entries(wallet: Option<&Value>) -> Vec<(Option<i64>, f64)> {
    let Some(entries) = wallet
        .and_then(|w| w.pointer("/active_wallets/wallets"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|item| {
            let balance = item.get("balance").and_then(Value::as_f64)?;
            let at = item
                .get("valid_to")
                .and_then(auth::parse_iso8601_ms_value)
                .filter(|ms| *ms > 0 && *ms < EXPIRY_SENTINEL_MS);
            Some((at, balance))
        })
        .collect()
}

/// 「最近到期」那一份：优先上游的 `expiring_soon`，回落自己挑 `valid_to` 最早的。
///
/// 上游 `expiring_soon.wallets[]` 已经是它筛过的「即将到期」（实测免费版为空数组
/// —— 那是真的没有快到期的东西，不是读法不对）。它为空时才自己从
/// `active_wallets` 挑，两条路都取不到就给 `(None, None)`，界面显示 `—`。
fn nearest_expiry(wallet: Option<&Value>) -> (Option<i64>, Option<f64>) {
    let soon = wallet
        .and_then(|w| w.pointer("/expiring_soon/wallets"))
        .and_then(Value::as_array)
        .filter(|rows| !rows.is_empty());
    let pool: Vec<(Option<i64>, f64)> = match soon {
        Some(rows) => rows
            .iter()
            .filter_map(|item| {
                let balance = item.get("balance").and_then(Value::as_f64)?;
                let at = item
                    .get("valid_to")
                    .and_then(auth::parse_iso8601_ms_value)
                    .filter(|ms| *ms > 0 && *ms < EXPIRY_SENTINEL_MS);
                Some((at, balance))
            })
            .collect(),
        None => wallet_entries(wallet),
    };
    pool.into_iter()
        .filter_map(|(at, balance)| at.map(|at| (at, balance)))
        .min_by_key(|(at, _)| *at)
        .map(|(at, balance)| (Some(at), Some(balance)))
        .unwrap_or((None, None))
}

/// `data.plan` → `subscription{planName, expireAt}`。
///
/// 实测免费版：`{pid: "subscription-cn-free", name: "个人免费版",
/// next_due_date: null, is_subscribed: false}` —— **`next_due_date` 是 null**，
/// 免费版没有到期日，如实留空让界面显示 `—`（与 Qoder 那边对哨兵值的处置一致）。
fn subscription_of(context: Option<&Value>) -> Value {
    let mut subscription = Map::new();
    if let Some(plan) = context.and_then(|ctx| ctx.get("plan")).filter(|v| v.is_object()) {
        for key in ["name", "plan_name", "title"] {
            if let Some(name) = plan
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                subscription.insert("planName".to_string(), json!(name));
                break;
            }
        }
        for key in ["next_due_date", "nextDueDate", "expire_at", "expires_at"] {
            if let Some(value) = plan.get(key).filter(|v| !v.is_null()) {
                let normalized = normalize_expiry(value);
                if !normalized.is_null() {
                    subscription.insert("expireAt".to_string(), normalized);
                }
                break;
            }
        }
    }
    Value::Object(subscription)
}

/// 到期值归一：ISO 字符串 → 毫秒；哨兵/0/空 → null。
fn normalize_expiry(value: &Value) -> Value {
    let Some(ms) = auth::parse_iso8601_ms_value(value) else {
        return Value::Null;
    };
    if ms <= 0 || ms >= EXPIRY_SENTINEL_MS {
        return Value::Null;
    }
    json!(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 免费版实测形态：只有百分比，没有结构化额度。
    #[test]
    fn normalizes_percentage_only_shape() {
        let body = json!({
            "user_id": "u", "user_type": "personal_standard",
            "total_usage_percentage": 0, "is_quota_exceeded": false,
            "user_quota": null, "add_on_quota": null
        });
        let out = normalize(&body);
        assert_eq!(out.get("percent").and_then(Value::as_f64), Some(0.0));
        assert_eq!(out.get("total").and_then(Value::as_f64), Some(100.0));
        assert_eq!(out.get("remaining").and_then(Value::as_f64), Some(100.0));
        assert_eq!(out.get("exceeded").and_then(Value::as_bool), Some(false));
    }

    #[test]
    fn normalizes_structured_shape() {
        let body = json!({
            "is_quota_exceeded": true,
            "user_quota": {"total": 1000, "used": 1000, "remaining": 0, "reset_at": 1792425431921_i64}
        });
        let out = normalize(&body);
        assert_eq!(out.get("total").and_then(Value::as_f64), Some(1000.0));
        assert_eq!(out.get("remaining").and_then(Value::as_f64), Some(0.0));
        assert_eq!(out.get("exceeded").and_then(Value::as_bool), Some(true));
        assert_eq!(out.get("resetAt").and_then(Value::as_i64), Some(1792425431921));
    }

    #[test]
    fn missing_everything_is_null_not_panic() {
        let out = normalize(&json!({}));
        assert!(out.get("total").map(Value::is_null).unwrap_or(true));
        assert_eq!(out.get("exceeded").and_then(Value::as_bool), Some(false));
    }

    /// ★ `adapter` 族带 `{"code":"ok","data":{...}}` 信封；不解开就全是 null，
    /// 而 null 在界面上表现为「这家没有余额」—— 与「上游真没给」无法区分。
    #[test]
    fn unwrap_peels_the_code_data_envelope() {
        let body = json!({"code": "ok", "data": {"total_balance": 1738.69}});
        assert_eq!(unwrap(&body).get("total_balance").and_then(Value::as_f64), Some(1738.69));
        // 裸 JSON（`quota/usage` 与 `userinfo` 就是）原样返回，同一个函数两种都成立
        let bare = json!({"user_id": "u"});
        assert_eq!(unwrap(&bare).get("user_id").and_then(Value::as_str), Some("u"));
        // `code` 在但 `data` 不是对象（脏响应）时不 panic，原样返回交由上层判空
        let dirty = json!({"code": "ok", "data": null});
        assert_eq!(unwrap(&dirty).get("code").and_then(Value::as_str), Some("ok"));
    }

    /// 实测样本：余额取 `quota.remaining`（上游算好的 1738.6854），
    /// 不要自己把 daily 与 longterm 相加 —— 虽然此刻两者相等，
    /// 但那是上游的口径，它以后加新桶时我们不该跟着改。
    #[test]
    fn available_prefers_quota_remaining_over_summing_buckets() {
        let wallet = json!({
            "daily_credits": {"total_balance": 97.4313},
            "longterm_credits": {"total_balance": 1641.2541},
            "active_wallets": {"wallets": [
                {"balance": 97.4313,  "valid_to": "2026-10-20T00:00:00+08:00"},
                {"balance": 1641.2541,"valid_to": "2026-11-28T21:19:24.496+08:00"}
            ]},
            "expiring_soon": {"count": 0, "total_balance": 0, "wallets": []}
        });
        // `quota.remaining` 给一个**与两桶之和不同**的值，确保取的是它
        let context = json!({"quota": {"total": null, "used": null, "remaining": 9999.0, "exceeded": false}});
        let mut document = json!({});
        merge_balance(&mut document, Some(&wallet), Some(&context));
        assert_eq!(document.get("available").and_then(Value::as_f64), Some(9999.0));
    }

    /// `quota` 段缺失时回落成两桶相加（这正是实测里两者相等的那个数）。
    #[test]
    fn available_falls_back_to_bucket_sum_without_quota() {
        let wallet = json!({
            "daily_credits": {"total_balance": 97.4313},
            "longterm_credits": {"total_balance": 1641.2541}
        });
        let mut document = json!({});
        merge_balance(&mut document, Some(&wallet), None);
        // 和值 1738.6854 在出口被 `amount_json` 收成两位（97.43 + 1641.25）
        let available = document.get("available").and_then(Value::as_f64).unwrap_or(0.0);
        assert!((available - 1738.69).abs() < 1e-6, "实得 {available}");
        // 明细行仍分两项给用户看（行名 = 上游官方弹窗的两个分类）
        let credits = document.get("credits").and_then(Value::as_array).cloned().unwrap_or_default();
        assert_eq!(credits.len(), 2);
        assert_eq!(credits[0].get("name").and_then(Value::as_str), Some("每日刷新积分"));
        assert_eq!(credits[1].get("name").and_then(Value::as_str), Some("其他积分"));
        assert_eq!(credits[0].get("daily").and_then(Value::as_bool), Some(true));
        // 读数收两位、行内不写 unit（单位在文档级，见 `credit_row` 的说明）
        assert_eq!(credits[0].get("remain").and_then(Value::as_f64), Some(97.43));
        assert!(credits[0].get("unit").is_none(), "行内不该写 unit");
    }

    /// 「最近到期」优先用上游算好的 `expiring_soon`；
    /// 它为**空数组**（实测免费版如此）时才自己挑 `valid_to` 最早的那项。
    #[test]
    fn nearest_expiry_prefers_expiring_soon_then_falls_back() {
        let empty_soon = json!({
            "daily_credits": {"total_balance": 97.4313},
            "longterm_credits": {"total_balance": 1641.2541},
            "active_wallets": {"wallets": [
                {"balance": 1641.2541, "valid_to": "2026-11-28T21:19:24.496+08:00"},
                {"balance": 97.4313,   "valid_to": "2026-10-20T00:00:00+08:00"}
            ]},
            "expiring_soon": {"count": 0, "total_balance": 0, "wallets": []}
        });
        let (at, amount) = nearest_expiry(Some(&empty_soon));
        // 全空时自己挑 → 10-20 那笔（97.43），不是第一项（11-28 的 1641）
        let expected = auth::parse_iso8601_ms_value(&json!("2026-10-20T00:00:00+08:00")).unwrap();
        assert_eq!(at, Some(expected));
        assert_eq!(amount, Some(97.4313));

        // 上游给了 expiring_soon → 直接用它，不信自己挑的
        let with_soon = json!({
            "active_wallets": {"wallets": [{"balance": 100.0, "valid_to": "2026-10-20T00:00:00+08:00"}]},
            "expiring_soon": {"count": 1, "total_balance": 5.0,
                "wallets": [{"balance": 5.0, "valid_to": "2026-10-19T23:00:00+08:00"}]}
        });
        let (at, amount) = nearest_expiry(Some(&with_soon));
        let expected = auth::parse_iso8601_ms_value(&json!("2026-10-19T23:00:00+08:00")).unwrap();
        assert_eq!(at, Some(expected));
        assert_eq!(amount, Some(5.0));
    }

    /// 没有 `valid_to` 的项不参与「最近到期」比较 —— 拿一个无期限余额
    /// 冒充到期日，比留空更糟。
    #[test]
    fn wallets_without_valid_to_do_not_fake_an_expiry() {
        let wallet = json!({
            "active_wallets": {"wallets": [{"balance": 100.0}]}
        });
        let mut document = json!({});
        merge_balance(&mut document, Some(&wallet), None);
        assert!(document.get("nearestExpiry").is_none());
    }

    /// 套餐名与有效期从 `plan` 段读；免费版 `next_due_date` 是 null，
    /// 如实留空（界面显示 `—`），不要伪造一个日期。
    #[test]
    fn subscription_reads_plan_name_and_optional_expiry() {
        let free = json!({"plan": {"pid": "subscription-cn-free", "name": "个人免费版",
                                   "next_due_date": null, "is_subscribed": false}});
        let mut document = json!({});
        merge_balance(&mut document, None, Some(&free));
        assert_eq!(
            document.pointer("/subscription/planName").and_then(Value::as_str),
            Some("个人免费版")
        );
        assert!(
            document.pointer("/subscription/expireAt").is_none(),
            "免费版没有到期日，不该造一个出来"
        );

        let paid = json!({"plan": {"name": "Pro", "next_due_date": "2026-11-19T00:00:00Z"}});
        let mut document = json!({});
        merge_balance(&mut document, None, Some(&paid));
        assert_eq!(
            document.pointer("/subscription/planName").and_then(Value::as_str),
            Some("Pro")
        );
        assert!(document.pointer("/subscription/expireAt").and_then(Value::as_i64).is_some());

        // 哨兵日期不透出
        let sentinel = json!({"plan": {"name": "X", "next_due_date": "9999-12-31T00:00:00Z"}});
        let mut document = json!({});
        merge_balance(&mut document, None, Some(&sentinel));
        assert!(document.pointer("/subscription/expireAt").is_none(), "哨兵日期不该透出");
    }

    /// 两个来源全失败（都 None）时不能 panic，只是「余额面板没有内容」。
    #[test]
    fn both_sources_missing_still_yields_a_document() {
        let mut document = json!({"provider": "qwenwork"});
        merge_balance(&mut document, None, None);
        assert!(document.get("available").is_none());
        assert_eq!(document.get("unit").and_then(Value::as_str), Some("积分"));
        assert!(document.get("subscription").and_then(Value::as_object).map(|s| s.is_empty()).unwrap_or(false));
    }
}
