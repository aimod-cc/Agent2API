//! MiniMax Code 账户读数：积分余额 + 逐份积分批次（到期两列）+ 签到。
//!
//! ── 本项目「积分制」家的标准口径（照 ChatGLM / Qoder 的形状）────────
//! 账号页的四列分别读这些键（前端 `accounts-panels.tsx` / `accounts-domain.ts`）：
//! ```text
//!   余额          顶层 availableView（优于 balanceView）→ 主读数
//!   明细弹层      wallets[].balanceView（一句人话）
//!   最近到期      credits[].expireAt 里**还有剩余**的最早一笔（creditExpiryOf）
//!   最近到期积分  同一笔的 remain（creditRowsOf）
//! ```
//! 所以 `credits[]` 必须写 `expireAt` + `remain`，且**过期的与已用尽的不要写进去**
//! （`creditExpiryOf` 会跳过 `expire <= now`，但 `creditRowsOf` 会把它们折叠成
//! 「已用完」摞在弹层末尾 —— 本家每天发一批、30 天错峰的阶梯有三十行，
//! 全塞进去会淹掉真正要紧的那几笔）。取舍写在 [`credit_rows`]。
//!
//! ── 三个接口各回答什么 ──────────────────────────────────────
//! ```text
//!   POST /matrix/api/v1/commerce/get_membership_info   → 余额（唯一权威总额）
//!   GET  /minimax-cloud/api/v1/credit/details          → 每批的剩余与到期时刻
//!   GET  /minimax-cloud/api/v1/signin/status           → 七天一签的进度
//!   POST /minimax-cloud/api/v1/signin/claim            → 领今天那份
//! ```
//! 前两个都要「签名 + Bearer」（缺签名回 `400 invalid signature`，见
//! `endpoints.rs` 的模块头）；**余额以 membership 为准、台账只负责时间** ——
//! 过期批次不清零（实测留下 `-0.31` 这类负残留），把台账的 `remaining_amount`
//! 直接相加会与余额对不上。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。
//! 测试**不碰网络、不读本机**：所有解析函数都是纯函数，网络部分只做拼装与转发。

use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::auth_http;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

use super::credentials::{self, MinimaxCredential};
use super::endpoints;
use super::region::Region;

/// 账户类请求的超时（都是小请求；别用 egress 默认的 SSE 长超时）。
const REQUEST_TIMEOUT_MS: u64 = 20_000;

/// 台账翻页上限（每页 100 条，30 天阶梯 + 购买批次远低于这个数；
/// 上限只是防御上游 `total_count` 给错值时的死循环）。
const MAX_LEDGER_PAGES: u32 = 10;

/// 签到接口要求的时区名。上游只认 IANA 名，传偏移量回 `invalid timezone_id`。
pub const SIGNIN_TIMEZONE: &str = "Asia/Shanghai";

/// 一轮签到的天数（上游 `days[]` 恒 7 条）。
pub const SIGNIN_CYCLE_DAYS: usize = 7;

/// 签到领取结果码：1 = 本次领到，2 = 今日已领（**幂等，不是错误**）。
const CLAIM_CREDITED: i64 = 1;
const CLAIM_ALREADY: i64 = 2;

/// 一次签名请求的结果。
struct SignedResponse {
    status: u16,
    payload: Option<Value>,
}

/// 发一次带签名的账户类请求（拼查询串 → 算三头 → 打网关）。
///
/// `region` 决定打哪个网关（国内 / 国际）。**签名本身不分地区**：它是
/// `pathname + search` 的 MD5，域名不在里面（见 `endpoints.rs` 的模块头）。
async fn signed_request(
    region: Region,
    credential: &MinimaxCredential,
    path: &str,
    extra_query: &str,
    body: Option<&Value>,
    proxy: Option<&ResolvedProxy>,
) -> Result<SignedResponse, GatewayError> {
    let now = crate::server::logging::now_ms();
    let body_text = match body {
        Some(value) => value.to_string(),
        None => String::new(),
    };
    let signed = endpoints::sign(
        path,
        extra_query,
        &body_text,
        now,
        endpoints::timezone_offset(),
        &credential.user_id,
    );
    let url = format!("{}{}", region.gateway_url(), signed.path_with_query);
    let mut headers: Vec<(String, String)> = vec![(
        "Authorization".to_string(),
        format!("Bearer {}", credential.access_token),
    )];
    headers.extend(signed.headers);

    // **不跟随 3xx**：国际站对大陆出口 IP 回 307 跳 `agent.minimaxi.com`，
    // 而那个域只认国内令牌 ⇒ 跟随跳转会把「出口不在服务范围」伪装成
    // 「令牌失效」，把用户引向无意义的重新登录（`region.rs` 模块头有实测表）。
    // 关掉跳转后我们能看到真实状态码，下面 `describe_failure` 会据此给出
    // 「换个出口」这种正确建议。客户端自己也是 `redirect: 'error'`。
    let response = auth_http::send_raw_no_redirect(
        if body.is_some() { "POST" } else { "GET" },
        &url,
        body,
        &headers,
        proxy,
        Some(REQUEST_TIMEOUT_MS),
    )
    .await
    .map_err(|err| GatewayError::with_status(502, format!("MiniMax Code 账户接口不可达：{err}")))?;
    Ok(SignedResponse {
        status: response.status,
        payload: response.payload,
    })
}

/// 把一次失败翻译成给人看的话（401 点名「重新登录」，缺签名单列一句）。
fn describe_failure(status: u16, payload: Option<&Value>, what: &str) -> GatewayError {
    let text = payload.map(|value| value.to_string()).unwrap_or_default();
    let lower = text.to_ascii_lowercase();
    let message = if lower.contains("invalid signature") {
        format!("{what}被上游拒绝：请求签名不合法（本仓签名实现与客户端不一致，请反馈）")
    } else if (300..400).contains(&status) {
        // **3xx 必须单独一支**：国际站对大陆出口 IP 回 307 跳国内域，
        // 跟过去只会得到 401（见 `region.rs` 模块头）。不单独说的话，
        // 用户会看到「登录态已过期」并被引向无意义的重新登录。
        //
        // 文案要**点名这个账号自己**：余额/签到走的是账号级出口
        // （`account_proxy` → `session.proxy`），没配就是**真的直连**
        // （`build_client_with(None)` 显式 `no_proxy()`，不会回退系统代理）。
        // 只说「去网络设置配代理」会让人以为是全局开关，配完还不生效。
        //
        // ⚠️ 这句是**纯文本**（前端按文本渲染，不解析 Markdown）：别写
        // `**强调**`，否则用户会看到一对原样的星号。
        format!(
            "{what}失败：上游返回 {status} 重定向 —— 本机出口 IP 不在国际站的服务范围。\
             请在「这个账号」的代理设置里指定一个境外出口，配好后重试；\
             这不是登录态问题，重新登录不会改变结果"
        )
    } else if lower.contains("invalid access token") || status == 401 {
        format!("{what}失败：登录态已过期，请在账号页重新登录（或在「刷新 Token」处续期）")
    } else if text.trim().is_empty() {
        format!("{what}失败：上游返回 HTTP {status}（空响应体）")
    } else {
        format!("{what}失败：上游返回 HTTP {status}: {text}")
    };
    GatewayError::with_status(if status == 401 { 401 } else { 502 }, message)
}

/// 上游金额字段可能是字符串（`"11459.454"`）也可能是数字 —— 两种都认。
fn to_number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        return number.is_finite().then_some(number);
    }
    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    text.parse::<f64>().ok().filter(|number| number.is_finite())
}

/// 紧凑数字（`12345.6` → `1.2万`；`998` → `998`）。
///
/// 与别家的 `compact` 同一取向：余额列只有百来像素宽，写全 `11,459.454`
/// 会被截断成 `11,4…`。
fn compact(value: f64) -> String {
    let amount = value.abs();
    if amount >= 100_000_000.0 {
        return format!("{:.2}亿", value / 100_000_000.0);
    }
    if amount >= 10_000.0 {
        return format!("{:.1}万", value / 10_000.0);
    }
    if amount >= 100.0 {
        return format!("{:.0}", value);
    }
    if amount >= 10.0 {
        return format!("{:.1}", value);
    }
    format!("{:.2}", value)
}

/// 余额摘要（`op_credit_summary`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreditSummary {
    pub total: f64,
    pub purchased: f64,
    pub free: f64,
}

/// 从 `get_membership_info` 的响应里取余额摘要。
///
/// ── 字段链照抄客户端（同一条兜底链，少一级就会在某种账号上读成 0）──
/// ```js
/// op_credit_summary.total_remaining_amount
///   ?? opcredit_balance ?? total_remains_credit ?? 0
/// ```
/// ⚠️ `total_remains_credit` / `opcredit_balance` 是**旧账**：在已迁到积分池的
/// 账号上恒为 0（`dsh-connect-minimaxcode` README §六「注意别取错字段」）。
/// 所以它们只在 `op_credit_summary` **整块缺失**时兜底，而不是并列候选。
pub fn parse_credit_summary(payload: &Value) -> Option<CreditSummary> {
    // 账户类接口的信封：`{base_resp:{status_code,…}, data:{…}}`；也兼容直接平铺
    let body = payload.get("data").unwrap_or(payload);
    if let Some(summary) = body.get("op_credit_summary").filter(|value| value.is_object()) {
        let total = to_number(summary.get("total_remaining_amount"));
        if let Some(total) = total {
            let purchased = to_number(summary.get("purchased_remaining_amount")).unwrap_or(0.0);
            let free = to_number(summary.get("free_remaining_amount")).unwrap_or(total - purchased);
            return Some(CreditSummary {
                total,
                purchased,
                free,
            });
        }
    }
    let flat = to_number(body.get("opcredit_balance"))
        .or_else(|| to_number(body.get("total_remains_credit")))?;
    Some(CreditSummary {
        total: flat,
        purchased: 0.0,
        free: flat,
    })
}

/// 一份积分批次。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreditLot {
    /// 发放面额
    pub granted: f64,
    /// 剩余
    pub remaining: f64,
    /// 发放时刻（毫秒；0 = 上游没给）
    pub granted_at_ms: i64,
    /// 到期时刻（毫秒；缺这个字段的批次**直接丢弃** —— 见 [`parse_lots`]）
    pub expire_at_ms: i64,
    /// 上游的批次类型（1 = 购买，2 = 签到）
    pub credit_type: Option<i64>,
}

/// 读一批台账条目（`details[]`）。
///
/// **没有 `expire_at_ms` 的条目一律丢弃**：没有到期时刻的批次如果当成
/// 「永不过期」写进 `credits[]`，前端 `creditExpiryOf` 会跳过它（`expire<=0`），
/// 于是它既不进到期列也不进明细 —— 不如不写。
pub fn parse_lots(payload: &Value) -> Vec<CreditLot> {
    let body = payload.get("data").unwrap_or(payload);
    let Some(details) = body.get("details").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut lots: Vec<CreditLot> = Vec::with_capacity(details.len());
    for entry in details {
        let expire_at_ms = to_number(entry.get("expire_at_ms")).unwrap_or(0.0);
        if expire_at_ms <= 0.0 {
            continue;
        }
        let granted = to_number(entry.get("granted_amount")).unwrap_or(0.0);
        let remaining = to_number(entry.get("remaining_amount")).unwrap_or(0.0);
        lots.push(CreditLot {
            granted,
            remaining,
            granted_at_ms: to_number(entry.get("granted_at_ms")).unwrap_or(0.0) as i64,
            expire_at_ms: expire_at_ms as i64,
            credit_type: entry.get("credit_type").and_then(Value::as_i64),
        });
    }
    lots.sort_by_key(|lot| lot.expire_at_ms);
    lots
}

/// 台账响应里的总条数（翻页用；缺失返回 None）。
pub fn total_count(payload: &Value) -> Option<i64> {
    let body = payload.get("data").unwrap_or(payload);
    body.get("total_count").and_then(Value::as_i64)
}

/// 一批台账条目是否为空（翻页终止判据）。
pub fn raw_lot_count(payload: &Value) -> usize {
    payload
        .get("data")
        .unwrap_or(payload)
        .get("details")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

/// 「还有剩余且未过期」的批次，按到期升序。
fn alive_lots(lots: &[CreditLot], now_ms: i64) -> Vec<&CreditLot> {
    let mut alive: Vec<&CreditLot> = lots
        .iter()
        .filter(|lot| lot.remaining > 0.0 && lot.expire_at_ms > now_ms)
        .collect();
    alive.sort_by_key(|lot| lot.expire_at_ms);
    alive
}

/// 批次 → `credits[]` 行（前端读 `expireAt` / `remain` / `name`）。
///
/// ── 三个取舍（都在前端有对应的读数方）──────────────────────────
///   1. **只写还有剩余的批次**：用光的那批到期时没有东西可损失，
///      写进去会让「最近到期积分」显示成 0（`dsh-connect-minimaxcode` README
///      §七「只统计仍有余额的批次」同一结论）；
///   2. **不写 `daily` / `record` / `unlimited` 三个标记**：三个都是
///      `creditExpiryOf` 的跳过键，本家的批次是**真到期**（发放日 + 30 天），
///      不是每日重置；
///   3. `name` 按 `credit_type` 给中文（1 = 购买积分，2 = 签到积分），
///      缺失时给中性名 —— 明细弹层里「这是哪来的」比金额次要，但有比没有好。
fn credit_rows(lots: &[CreditLot], now_ms: i64) -> Vec<Value> {
    alive_lots(lots, now_ms)
        .into_iter()
        .map(|lot| {
            let mut row = Map::new();
            row.insert(
                "name".to_string(),
                json!(match lot.credit_type {
                    Some(1) => "购买积分",
                    Some(2) => "签到积分",
                    _ => "积分",
                }),
            );
            row.insert("unit".to_string(), json!("积分"));
            row.insert("granted".to_string(), json!(lot.granted));
            row.insert("remain".to_string(), json!(lot.remaining));
            row.insert("expireAt".to_string(), json!(lot.expire_at_ms));
            if lot.granted_at_ms > 0 {
                row.insert("grantAt".to_string(), json!(lot.granted_at_ms));
            }
            Value::Object(row)
        })
        .collect()
}

/// 一次签到的面板（`days[]` 里的一天）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SigninDay {
    pub day_no: i64,
    pub points: f64,
    pub status: i64,
    pub is_today: bool,
}

/// 签到面板。
#[derive(Clone, Debug, PartialEq)]
pub struct SigninPanel {
    pub scene: i64,
    pub days: Vec<SigninDay>,
}

impl SigninPanel {
    /// 今天这一天（可能没有）。
    pub fn today(&self) -> Option<&SigninDay> {
        self.days.iter().find(|day| day.is_today)
    }

    /// 今天能不能领（上游只把可领那一天标 `status == 2`）。
    pub fn claimable(&self) -> bool {
        self.today().map(|day| day.status == 2).unwrap_or(false)
    }

    /// 今天是否已领。
    pub fn claimed(&self) -> bool {
        self.today().map(|day| day.status == 3).unwrap_or(false)
    }

    /// 一轮的总积分（界面上「一轮共 N 积分」）。
    pub fn cycle_points(&self) -> f64 {
        self.days.iter().map(|day| day.points).sum()
    }
}

/// 读签到面板；形状不合规返回 None（`validateSigninPanel` 的同款校验）。
///
/// 校验三条（照客户端 `@mavis/shared/dist/daily-signin.js`）：
/// 恰好 7 天、`day_no` 唯一且落在 1..=7、可领天与今天各至多一个。
/// **半信半疑的响应不如不认** —— 读错一天会让按钮状态与事实相反。
pub fn parse_panel(payload: &Value) -> Option<SigninPanel> {
    // 业务失败被包在 HTTP 200 里：`base_resp.status_code != 0` 就是失败
    if let Some(code) = payload
        .pointer("/base_resp/status_code")
        .and_then(Value::as_i64)
    {
        if code != 0 {
            return None;
        }
    }
    let data = payload.get("data")?;
    let scene = data.get("scene").and_then(Value::as_i64).unwrap_or(0);
    let raw_days = data.get("days")?.as_array()?;
    let mut days: Vec<SigninDay> = Vec::with_capacity(raw_days.len());
    let mut seen: Vec<i64> = Vec::with_capacity(SIGNIN_CYCLE_DAYS);
    let (mut claimable, mut todays) = (0u32, 0u32);
    for entry in raw_days {
        let Some(day_no) = entry.get("day_no").and_then(Value::as_i64) else {
            continue;
        };
        if !(1..=7).contains(&day_no) || seen.contains(&day_no) {
            return None;
        }
        let Some(points) = to_number(entry.get("points")) else {
            return None;
        };
        let Some(status) = entry.get("status").and_then(Value::as_i64) else {
            return None;
        };
        if !(1..=4).contains(&status) {
            return None;
        }
        let is_today = entry.get("is_today").and_then(Value::as_bool).unwrap_or(false);
        if status == 2 {
            claimable += 1;
        }
        if is_today {
            todays += 1;
        }
        seen.push(day_no);
        days.push(SigninDay {
            day_no,
            points,
            status,
            is_today,
        });
    }
    if days.len() != SIGNIN_CYCLE_DAYS || claimable > 1 || todays > 1 {
        return None;
    }
    days.sort_by_key(|day| day.day_no);
    Some(SigninPanel { scene, days })
}

/// 一次签到的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct SigninClaim {
    pub claim_id: String,
    /// 1 = 本次领到，2 = 今日已领
    pub result: i64,
    pub day_no: i64,
    pub points: f64,
    pub expire_at_ms: i64,
    pub panel: Option<SigninPanel>,
}

/// 读签到结果；形状不合规返回 None。
pub fn parse_claim(payload: &Value) -> Option<SigninClaim> {
    if let Some(code) = payload
        .pointer("/base_resp/status_code")
        .and_then(Value::as_i64)
    {
        if code != 0 {
            return None;
        }
    }
    let data = payload.get("data")?;
    let claim_id = data.get("claim_id")?.as_str()?.trim().to_string();
    if claim_id.is_empty() {
        return None;
    }
    let result = data.get("claim_result").and_then(Value::as_i64)?;
    if result != CLAIM_CREDITED && result != CLAIM_ALREADY {
        return None;
    }
    let day_no = data.get("day_no").and_then(Value::as_i64)?;
    if !(1..=7).contains(&day_no) {
        return None;
    }
    let points = to_number(data.get("points")).unwrap_or(0.0);
    let expire_at_ms = to_number(data.get("expire_at_ms")).unwrap_or(0.0) as i64;
    Some(SigninClaim {
        claim_id,
        result,
        day_no,
        points,
        expire_at_ms,
        panel: data.get("panel").and_then(|panel| parse_panel(&json!({ "data": panel }))),
    })
}

/// 上游信封里的失败文案（`base_resp.status_msg` / `error` / `message`）。
fn envelope_message(payload: &Value) -> String {
    payload
        .pointer("/base_resp/status_msg")
        .and_then(Value::as_str)
        .or_else(|| payload.get("error").and_then(Value::as_str))
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 接口成功判定：`base_resp.status_code == 0`，或没有信封时看 `code`。
fn envelope_ok(payload: &Value) -> bool {
    if let Some(code) = payload
        .pointer("/base_resp/status_code")
        .and_then(Value::as_i64)
    {
        return code == 0;
    }
    match payload.get("code").and_then(Value::as_i64) {
        Some(code) => code == 0 || code == 200,
        None => true,
    }
}

// ────────────────────────────────────────────────────────────────
// 网络部分
// ────────────────────────────────────────────────────────────────

/// 取一条记录并还原凭证（`account_id` 为空时用**本地区**的队首账号）。
fn credential_for(
    region: Region,
    store: &AccountStore,
    account_id: &str,
) -> Result<MinimaxCredential, GatewayError> {
    let record = store.minimax_account_record(region, account_id).ok_or_else(|| {
        GatewayError::with_status(
            404i32,
            if account_id.is_empty() {
                format!(
                    "还没有 MiniMax Code {}账号，请先在账号页添加",
                    region.label()
                )
            } else {
                format!(
                    "MiniMax Code {}账号 {account_id} 不存在或不可用",
                    region.label()
                )
            },
        )
    })?;
    credentials::from_record(Some(&record))
}

/// 查一次余额（归一化形状见模块头）。
pub async fn fetch_usage(
    region: Region,
    credential: &MinimaxCredential,
    proxy: Option<&ResolvedProxy>,
) -> Result<Value, GatewayError> {
    // ① 余额（唯一权威总额）
    let membership = signed_request(
        region,
        credential,
        endpoints::MEMBERSHIP_PATH,
        "",
        Some(&json!({})),
        proxy,
    )
    .await?;
    if !(200..300).contains(&membership.status) {
        return Err(describe_failure(membership.status, membership.payload.as_ref(), "查询积分余额"));
    }
    let membership_payload = membership.payload.unwrap_or(Value::Null);
    if !envelope_ok(&membership_payload) {
        let message = envelope_message(&membership_payload);
        return Err(GatewayError::with_status(
            502i32,
            format!("查询积分余额失败：{}", if message.is_empty() { "上游返回业务失败".to_string() } else { message }),
        ));
    }
    let Some(summary) = parse_credit_summary(&membership_payload) else {
        return Err(GatewayError::with_status(
            502i32,
            "积分余额读取失败：上游响应里没有 op_credit_summary（账号可能尚未开通积分池）",
        ));
    };

    // ② 台账（到期两列）。**失败不拖垮余额** —— 两个不同接口，只有余额是必需的
    let (lots, ledger_note) = match fetch_ledger(region, credential, proxy).await {
        Ok(lots) => (lots, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let now = crate::server::logging::now_ms();
    Ok(normalize(region, summary, &lots, now, ledger_note))
}

/// 拉全量台账（翻页；`page_number` 才是生效参数，`page` 会被上游忽略）。
async fn fetch_ledger(
    region: Region,
    credential: &MinimaxCredential,
    proxy: Option<&ResolvedProxy>,
) -> Result<Vec<CreditLot>, GatewayError> {
    let mut all: Vec<CreditLot> = Vec::new();
    let mut page = 1u32;
    let mut total = i64::MAX;
    while page <= MAX_LEDGER_PAGES && (all.len() as i64) < total {
        let query = format!("page_number={page}&page_size=100");
        let response =
            signed_request(
                region,
                credential,
                endpoints::CREDIT_DETAILS_PATH,
                &query,
                None,
                proxy,
            )
            .await?;
        if !(200..300).contains(&response.status) {
            return Err(describe_failure(response.status, response.payload.as_ref(), "查询积分明细"));
        }
        let payload = response.payload.unwrap_or(Value::Null);
        if !envelope_ok(&payload) {
            return Err(GatewayError::with_status(502i32, "查询积分明细失败：上游返回业务失败"));
        }
        let batch = parse_lots(&payload);
        let raw_count = raw_lot_count(&payload);
        all.extend(batch);
        if let Some(count) = total_count(&payload) {
            total = count;
        }
        // 一整页都没条目 = 到底了（`total_count` 缺失或给错时也能收敛）
        if raw_count < 100 {
            break;
        }
        page += 1;
    }
    Ok(all)
}

/// 把余额与台账归一成 `query_usage` 的契约形状。
///
/// `region` 只用来在钱包 / 套餐名上标出地区（`MiniMax Code 国际版积分`）——
/// 同一页里两家的账号分组相邻，光看「MiniMax Code 积分」分不出是哪一个。
fn normalize(
    region: Region,
    summary: CreditSummary,
    lots: &[CreditLot],
    now_ms: i64,
    ledger_note: Option<String>,
) -> Value {
    let alive = alive_lots(lots, now_ms);
    let next_expire = alive.first().map(|lot| lot.expire_at_ms);
    let expiring_soon: f64 = alive
        .iter()
        .filter(|lot| lot.expire_at_ms - now_ms <= 7 * 86_400_000)
        .map(|lot| lot.remaining)
        .sum();
    // 三十天窗口内到期的积分（含前七天）—— 与 `dsh-connect-minimaxcode` 的
    // 「30 天内到期」同口径，给界面的细节描述用
    let expiring_month: f64 = alive
        .iter()
        .filter(|lot| lot.expire_at_ms - now_ms <= 30 * 86_400_000)
        .map(|lot| lot.remaining)
        .sum();

    let view = format!("剩 {} 积分", compact(summary.total));
    let plan_name = format!("MiniMax Code {}积分", region.label());

    let mut wallet = Map::new();
    wallet.insert("type".to_string(), json!("credits"));
    wallet.insert("displayName".to_string(), json!(plan_name.clone()));
    wallet.insert("unit".to_string(), json!("积分"));
    wallet.insert("balance".to_string(), json!(summary.total));
    wallet.insert("free".to_string(), json!(summary.free));
    wallet.insert("purchased".to_string(), json!(summary.purchased));
    wallet.insert("balanceView".to_string(), json!(view.clone()));

    let mut document = json!({
        "available": summary.total,
        "availableView": view,
        "unit": "积分",
        "wallets": [Value::Object(wallet)],
        "subscription": {
            "planName": plan_name,
            "status": "credits",
            // 积分是逐批到期的，没有「订阅到期日」这一个时刻 —— 写 0 而不是编
            "expireAt": 0,
            "remainQuota": summary.total,
        },
        "minimax": {
            "region": region.edition_id(),
            "nextExpireAt": next_expire.unwrap_or(0),
            "expiringIn7Days": expiring_soon,
            "expiringIn30Days": expiring_month,
            "lotCount": alive.len(),
            "expireNote": "积分按批次发放：每批在发放日 + 30 个自然日到期（账户时区当日 00:00 失效）",
            "ledgerNote": ledger_note,
        },
        "raw": {
            "membership": {
                "total": summary.total,
                "free": summary.free,
                "purchased": summary.purchased,
            },
            "lotCount": lots.len(),
        },
    });
    // `credits[]` 只在非空时挂：给一个空数组会让明细入口点出一个空框
    let rows = credit_rows(lots, now_ms);
    if !rows.is_empty() {
        if let Some(object) = document.as_object_mut() {
            object.insert("credits".to_string(), Value::Array(rows));
        }
    }
    document
}

/// 适配器入口：查某账号（`region` 地区的）余额。
pub async fn query_usage(
    region: Region,
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let credential = credential_for(region, store, account_id)?;
    let proxy = account_proxy(store, account_id);
    fetch_usage(region, &credential, proxy.as_ref()).await
}

/// 取该账号配的出网出口（`session.proxy`）。
///
/// ── 为什么余额必须跟着账号的代理走 ──────────────────────────
/// 国际站 `agent.minimax.io` 对**大陆出口 IP** 回 307，跟过去就是国内集群的
/// 401（见 `region.rs` 模块头）—— 也就是「余额能不能查」本身就取决于出口。
/// 账号记录上那个代理正是用户为这个账号挑的，不拿它等于白配。
///
/// 与 ZCode 同一取法（`proxies::session_proxy`）；拿不到就直连（`None`）——
/// 那是国内版的正常形态，也是「没配代理」的唯一含义。
fn account_proxy(store: &AccountStore, account_id: &str) -> Option<ResolvedProxy> {
    store
        .get_session_by_id(account_id)
        .and_then(|entry| crate::server::core::proxies::session_proxy(&entry.session))
}

/// 适配器入口：查某账号的签到面板（签到中心的自述用）。
pub async fn query_signin(
    region: Region,
    store: &AccountStore,
    account_id: &str,
) -> Result<SigninPanel, GatewayError> {
    let credential = credential_for(region, store, account_id)?;
    let query = format!(
        "timezone_id={}",
        endpoints::encode_uri_component(SIGNIN_TIMEZONE)
    );
    let proxy = account_proxy(store, account_id);
    let response = signed_request(
        region,
        &credential,
        endpoints::SIGNIN_STATUS_PATH,
        &query,
        None,
        proxy.as_ref(),
    )
    .await?;
    if !(200..300).contains(&response.status) {
        return Err(describe_failure(response.status, response.payload.as_ref(), "查询签到状态"));
    }
    let payload = response.payload.unwrap_or(Value::Null);
    parse_panel(&payload).ok_or_else(|| {
        GatewayError::with_status(
            502i32,
            "签到状态读取失败：上游响应不是合法的七天面板（可能接口已变更）",
        )
    })
}

/// 领今天那份积分（自动签到框架的 claim）。
///
/// 返回 `{success, msg}` —— 与别家 claim 同一形状（`billing::checkin` 的
/// `claim_result` 只认这两个字段）。**今天已领不算失败**：`claim_result == 2`
/// 如实报成功并说明是重复调用（幂等是上游保证的）。
pub async fn claim_signin(
    region: Region,
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let credential = credential_for(region, store, account_id)?;
    let query = format!(
        "timezone_id={}",
        endpoints::encode_uri_component(SIGNIN_TIMEZONE)
    );
    let proxy = account_proxy(store, account_id);
    let response = signed_request(
        region,
        &credential,
        endpoints::SIGNIN_CLAIM_PATH,
        &query,
        Some(&json!({})),
        proxy.as_ref(),
    )
    .await?;
    if !(200..300).contains(&response.status) {
        return Err(describe_failure(response.status, response.payload.as_ref(), "签到"));
    }
    let payload = response.payload.unwrap_or(Value::Null);
    let Some(claim) = parse_claim(&payload) else {
        let message = envelope_message(&payload);
        return Ok(json!({
            "success": false,
            "msg": if message.is_empty() {
                "签到失败：上游响应不是合法的领取结果".to_string()
            } else {
                format!("签到失败：{message}")
            },
        }));
    };
    if claim.result == CLAIM_ALREADY {
        return Ok(json!({
            "success": true,
            "alreadyCompleted": true,
            "msg": format!("今日已签到（第 {} 天，本轮无需重复领取）", claim.day_no),
        }));
    }
    Ok(json!({
        "success": true,
        "msg": format!(
            "签到成功：第 {} 天 +{} 积分（{} 到期）",
            claim.day_no,
            compact(claim.points),
            if claim.expire_at_ms > 0 { "30 天后" } else { "按上游规则" },
        ),
        "points": claim.points,
        "expireAt": claim.expire_at_ms,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 线上真实响应（2026-10-21 抓到的形状，数值脱敏）。
    fn membership_payload() -> Value {
        json!({
            "data": {
                "op_credit_summary": {
                    "total_remaining_amount": "11459.454",
                    "purchased_remaining_amount": "0",
                    "free_remaining_amount": "11459.454"
                },
                "total_remains_credit": 0,
                "opcredit_balance": 0,
                "has_token_plan": false
            },
            "base_resp": {"status_code": 0, "status_msg": "success"}
        })
    }

    #[test]
    fn reads_the_credit_summary_not_the_legacy_fields() {
        // 旧账字段在这里是 0，取错就会显示「0 积分」
        let summary = parse_credit_summary(&membership_payload()).expect("应能解析");
        assert!((summary.total - 11459.454).abs() < 1e-6);
        assert!((summary.purchased - 0.0).abs() < 1e-9);
        assert!((summary.free - 11459.454).abs() < 1e-6);
    }

    #[test]
    fn falls_back_to_legacy_fields_only_when_the_summary_is_absent() {
        let payload = json!({"data": {"opcredit_balance": 500}});
        let summary = parse_credit_summary(&payload).expect("应能解析");
        assert!((summary.total - 500.0).abs() < 1e-9);
        // 摘要整块缺失时才用它，且不假装有「购买/赠送」的拆分
        assert!((summary.free - 500.0).abs() < 1e-9);
        assert!((summary.purchased - 0.0).abs() < 1e-9);
    }

    #[test]
    fn summary_absent_and_no_legacy_fields_yields_none() {
        assert!(parse_credit_summary(&json!({"data": {}})).is_none());
        assert!(parse_credit_summary(&Value::Null).is_none());
    }

    fn lot(remaining: f64, expire_at_ms: i64, credit_type: i64) -> Value {
        json!({
            "granted_amount": "400",
            "remaining_amount": remaining,
            "consumed_amount": "0",
            "granted_at_ms": expire_at_ms - 30 * 86_400_000i64,
            "expire_at_ms": expire_at_ms,
            "credit_type": credit_type
        })
    }

    #[test]
    fn lots_without_expiry_are_dropped() {
        let payload = json!({"data": {"details": [
            lot(400.0, 1_800_000_000_000, 2),
            {"granted_amount": 100, "remaining_amount": 100}
        ], "total_count": 2}});
        let lots = parse_lots(&payload);
        assert_eq!(lots.len(), 1, "没有 expire_at_ms 的条目必须丢弃");
    }

    #[test]
    fn lots_are_sorted_by_expiry() {
        let payload = json!({"data": {"details": [
            lot(10.0, 1_900_000_000_000, 2),
            lot(20.0, 1_800_000_000_000, 1)
        ]}});
        let lots = parse_lots(&payload);
        assert_eq!(lots[0].expire_at_ms, 1_800_000_000_000);
        assert_eq!(lots[0].credit_type, Some(1));
    }

    /// **四列口径的回归护栏**：只算还有剩余的批次、只写未过期的，
    /// 且 `credits[].expireAt` 必须是毫秒（秒会被当成 1970 年）。
    #[test]
    fn credits_rows_cover_only_live_lots() {
        let now = 1_800_000_000_000i64;
        let payload = json!({"data": {"details": [
            lot(0.0, now + 86_400_000, 2),                 // 用光了 → 不写
            lot(400.0, now - 86_400_000, 2),               // 过期了 → 不写
            lot(400.0, now + 2 * 86_400_000, 2),           // 活着 → 写
            lot(-0.31, now + 3 * 86_400_000, 2)            // 负残留 → 不写
        ]}});
        let lots = parse_lots(&payload);
        let rows = credit_rows(&lots, now);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["remain"], json!(400.0));
        assert_eq!(rows[0]["expireAt"], json!(now + 2 * 86_400_000));
        assert_eq!(rows[0]["name"], json!("签到积分"));
        // 三个会被前端跳过/误会的标记一律不写
        assert!(rows[0].get("daily").is_none());
        assert!(rows[0].get("record").is_none());
        assert!(rows[0].get("unlimited").is_none());
    }

    #[test]
    fn normalize_reports_the_next_expiry_and_the_windows() {
        let now = 1_800_000_000_000i64;
        let lots = vec![
            CreditLot {
                granted: 400.0,
                remaining: 100.0,
                granted_at_ms: now - 86_400_000,
                expire_at_ms: now + 3 * 86_400_000,
                credit_type: Some(2),
            },
            CreditLot {
                granted: 1000.0,
                remaining: 500.0,
                granted_at_ms: now - 86_400_000,
                expire_at_ms: now + 20 * 86_400_000,
                credit_type: Some(2),
            },
        ];
        let summary = CreditSummary {
            total: 600.0,
            purchased: 0.0,
            free: 600.0,
        };
        let document = normalize(Region::Cn, summary, &lots, now, None);
        assert_eq!(document["available"], json!(600.0));
        assert_eq!(document["minimax"]["nextExpireAt"], json!(now + 3 * 86_400_000));
        // 7 天内只有第一批（100），30 天内两批（600）
        assert_eq!(document["minimax"]["expiringIn7Days"], json!(100.0));
        assert_eq!(document["minimax"]["expiringIn30Days"], json!(600.0));
        assert_eq!(document["credits"].as_array().map(Vec::len), Some(2));
        assert_eq!(document["minimax"]["region"], json!("cn"));
    }

    /// 归一化结果必须标出地区：两家的账号在同一页相邻，钱包名一样就分不清。
    #[test]
    fn normalize_labels_the_region() {
        let summary = CreditSummary {
            total: 1.0,
            purchased: 0.0,
            free: 1.0,
        };
        let intl = normalize(Region::Intl, summary, &[], crate::server::logging::now_ms(), None);
        assert_eq!(intl["minimax"]["region"], json!("intl"));
        assert_eq!(intl["wallets"][0]["displayName"], json!("MiniMax Code 国际版积分"));
        let cn = normalize(Region::Cn, summary, &[], crate::server::logging::now_ms(), None);
        assert_eq!(cn["subscription"]["planName"], json!("MiniMax Code 国内版积分"));
    }

    #[test]
    fn normalize_without_ledger_keeps_the_balance_and_explains() {
        let summary = CreditSummary {
            total: 1234.5,
            purchased: 0.0,
            free: 1234.5,
        };
        let document = normalize(Region::Cn, summary, &[], crate::server::logging::now_ms(), Some("台账不可用".into()));
        assert_eq!(document["available"], json!(1234.5));
        assert!(
            document.get("credits").is_none(),
            "没有批次就不写空数组（否则明细入口点出一个空框）"
        );
        assert_eq!(document["minimax"]["ledgerNote"], json!("台账不可用"));
        assert_eq!(document["availableView"], json!("剩 1234 积分"));
    }

    fn panel_payload() -> Value {
        json!({
            "base_resp": {"status_code": 0, "status_msg": "success"},
            "data": {
                "scene": 2,
                "days": [
                    {"day_no": 1, "points": 400, "status": 3, "is_today": false},
                    {"day_no": 2, "points": 400, "status": 3, "is_today": false},
                    {"day_no": 3, "points": 400, "status": 2, "is_today": true},
                    {"day_no": 4, "points": 1000, "status": 1, "is_today": false},
                    {"day_no": 5, "points": 400, "status": 1, "is_today": false},
                    {"day_no": 6, "points": 400, "status": 1, "is_today": false},
                    {"day_no": 7, "points": 1000, "status": 1, "is_today": false}
                ]
            }
        })
    }

    #[test]
    fn parses_a_valid_seven_day_panel() {
        let panel = parse_panel(&panel_payload()).expect("应能解析");
        assert_eq!(panel.days.len(), 7);
        assert_eq!(panel.scene, 2);
        assert!(panel.claimable());
        assert!(!panel.claimed());
        assert_eq!(panel.today().map(|day| day.day_no), Some(3));
        // 400/400/400/1000/400/400/1000
        assert_eq!(panel.cycle_points(), 4000.0);
    }

    #[test]
    fn panel_with_wrong_day_count_is_rejected() {
        let mut payload = panel_payload();
        if let Some(days) = payload.pointer_mut("/data/days").and_then(Value::as_array_mut) {
            days.pop();
        }
        assert!(parse_panel(&payload).is_none(), "不足 7 天必须整份拒绝");
    }

    #[test]
    fn panel_with_two_todays_is_rejected() {
        let mut payload = panel_payload();
        if let Some(days) = payload.pointer_mut("/data/days").and_then(Value::as_array_mut) {
            days[0]["is_today"] = json!(true);
        }
        assert!(parse_panel(&payload).is_none());
    }

    #[test]
    fn panel_with_two_claimable_is_rejected() {
        let mut payload = panel_payload();
        if let Some(days) = payload.pointer_mut("/data/days").and_then(Value::as_array_mut) {
            days[3]["status"] = json!(2);
        }
        assert!(parse_panel(&payload).is_none());
    }

    #[test]
    fn panel_with_business_error_is_rejected() {
        let payload = json!({
            "base_resp": {"status_code": 1406010011, "status_msg": "invalid timezone_id"},
            "data": json!(panel_payload()["data"])
        });
        assert!(parse_panel(&payload).is_none());
    }

    #[test]
    fn parses_a_fresh_claim() {
        let payload = json!({
            "base_resp": {"status_code": 0},
            "data": {
                "claim_id": "c-1",
                "claim_result": 1,
                "day_no": 3,
                "points": 400,
                "expire_at_ms": 1_802_592_000_000i64,
                "panel": panel_payload()["data"]
            }
        });
        let claim = parse_claim(&payload).expect("应能解析");
        assert_eq!(claim.result, CLAIM_CREDITED);
        assert_eq!(claim.points, 400.0);
        assert!(claim.panel.is_some());
    }

    #[test]
    fn parses_an_already_claimed_result() {
        let payload = json!({
            "base_resp": {"status_code": 0},
            "data": {"claim_id": "c-2", "claim_result": 2, "day_no": 3, "points": 0, "expire_at_ms": 1}
        });
        let claim = parse_claim(&payload).expect("应能解析");
        assert_eq!(claim.result, CLAIM_ALREADY, "今日已领是合法结果，不是失败");
    }

    #[test]
    fn unknown_claim_result_is_rejected() {
        let payload = json!({
            "base_resp": {"status_code": 0},
            "data": {"claim_id": "c-3", "claim_result": 9, "day_no": 3, "points": 0, "expire_at_ms": 1}
        });
        assert!(parse_claim(&payload).is_none());
    }

    #[test]
    fn compact_keeps_the_column_readable() {
        assert_eq!(compact(998.0), "998");
        assert_eq!(compact(11459.454), "1.1万");
        assert_eq!(compact(123_456_789.0), "1.23亿");
    }
}
