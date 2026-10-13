//! ZCode 的**开放平台监控通道**：编码套餐的**窗口限额**读数
//! （`GET {监控平面}/api/monitor/usage/quota/limit`）。
//!
//! ── 这条通道解决什么（本模块存在的理由）─────────────────────
//! `billing/balance`（见 `balance.rs`）认的是**套餐 JWT** —— 只粘了编码套餐
//! API Key 的账号（手工添加那条路，见前端 `add-account-configs.ts` 的说明）
//! 在那里拿不到任何读数，账号页的余额列只能显示「未配置」。而编码套餐的额度
//! 其实还有第二个读法：拿这把 API Key 打开放平台的监控接口，能读到
//! 「每 N 小时 / 每周」两档窗口的**已用比例**、重置时刻与套餐等级。
//! `balance.rs` 的候选令牌链因此把这条通道接成第二候选（JWT 缺失或被拒时的出路）。
//!
//! ── 端点是未公开的（证据与取舍）─────────────────────────────
//! 官方文档没有这条接口；口径来自三个独立第三方项目（CodexBar 的 z.ai 文档、
//! opencode-glm-quota、zai-limits）与社区实测，四点一致：
//!   · 两地各一个域（见 `Region::monitor_base_url`）；
//!   · 认证是 `Authorization` + 密钥本身，**没有**其它客户端身份头
//!     （不需要 `X-Device-Mid`、不需要 `app_version`）；
//!   · 信封 `{code, msg, data}`，`data.limits[]` 是窗口数组，
//!     `data.level` / `data.planName`（或 `plan` / `plan_type` / `packageName`）
//!     是套餐；
//!   · 2026-02 之后**只回百分比**（`percentage`），绝对量（`usage` /
//!     `currentValue` / `remaining`）已经不保证有。因此 [`normalize`] 的读数
//!     以百分比为主、绝对量有才用。
//!
//! ── 两处「不猜」（都写成了显式分支，别当成没做）───────────────
//!   1. **`Authorization` 形态**：第三方实现分成两派 —— 多数明确写「裸令牌、
//!      不带 Bearer」（且都打国内站），`zcode-switch` 与 CodexBar 则发 Bearer。
//!      本家自己的经历是：智谱系两个平面的 `Authorization` 形态本来就不同
//!      （国际 Bearer、国内裸令牌，见 `coding_key.rs` 模块头）。与其猜一个，
//!      不如让上游用 401/403 告诉我们：先发**与转发同一形态**的 Bearer
//!      （那是我们确定能用的），被拒就换裸令牌再试一次 —— 只有鉴权失败才换
//!      形态，业务错误（没有套餐等）一次即返。
//!   2. **窗口单位**：`unit` 是数字（3 = 每 N 小时、4 = 每天、5 = 每月、
//!      6 = 每周，其它 = 每周期），`type` 决定量纲（`TOKENS_LIMIT` 提示次数 /
//!      `TIME_LIMIT` 使用时长）。这几档与 `zcode-switch` 的映射逐条相同；
//!      认不出的 `type`（例如后来新增的 `CREDIT_LIMIT`）**原样显示** ——
//!      编一个中文名等于把我们猜的东西当上游事实给用户看。
//!
//! ── 输出的形状：与 billing 通道同一套（界面不必分家）──────────
//! 归一化的形状与 `balance.rs` 的那份**逐字段同名**（`availableView` /
//! `unit` / `wallets[]` / `subscription` / `raw`），多一个 `source`（排障用，
//! 也让界面将来能说清「这个读数打的是哪台网关」）。读数方向也在这一步统一成
//! **剩余**（上游给的是已用比例）—— 与 billing 通道、进度条同向，理由见
//! [`normalize`] 的文档。
//! 两处**刻意**不同（都是「这条通道没有这个数」的如实表达）：
//!   · **不写 `available`**：这条通道读的是比例，没有「还剩多少 token」这种
//!     绝对量；写了 0 会踩到缺省的余额跳过档（阈值 1）—— 账号会被从选路里
//!     剔掉，而它额度充足。缺失 = 不知道 = 不参与判定，与
//!     `usage_records::extract_remaining` 的口径一致。
//!   · **不写 `subscription.expireAt`**：这个端点没有套餐到期时间（有证据的
//!     字段只有窗口与套餐名/等级），编一个「套餐到期」比空着更糟 ——
//!     到期读数仍由 billing 通道给。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic，取值一律走 Option 链。

use chrono::TimeZone;
use serde_json::{json, Map, Value};

use crate::server::core::auth_http::{send_raw, ApiResponse};
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

use super::balance::{compact_number, number_of};
use super::region::Region;

/// 单次请求超时（与领取 / 余额查询同一档：同一个上游上的轻量只读调用，
/// 而 `egress` 的默认 read_timeout 是 600 秒 —— 不设总超时会让前端的
/// 「查询余额」转圈十分钟）
const REQUEST_TIMEOUT_MS: u64 = 15_000;

/// 监控端点路径（第三方实现逐字一致）
const QUOTA_PATH: &str = "/api/monitor/usage/quota/limit";

/// 取比例上限用（上游个别窗口会给 >100，钳住免得进度条越界）
const PERCENT_MAX: f64 = 100.0;

/// `Authorization` 的两种形态（顺序 = 尝试顺序，理由见模块头第 1 条）
#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthForm {
    /// `Bearer {key}` —— 与转发同一形态（本家确定能用的那个）
    Bearer,
    /// 裸令牌 —— 多数第三方实现打国内站时用的形态
    Raw,
}

impl AuthForm {
    /// 尝试顺序（Bearer 在前：它是本家转发展与凭证换取链上已经在用的形态）
    const ALL: [AuthForm; 2] = [AuthForm::Bearer, AuthForm::Raw];

    /// 本形态的 `Authorization` 头取值
    fn header(self, token: &str) -> String {
        match self {
            Self::Bearer => format!("Bearer {token}"),
            Self::Raw => token.to_string(),
        }
    }
}

/// 查询一个账号的窗口限额。
///
/// `token` 是账号里那把**编码套餐 API Key**（`accessToken`，与转发用的是同一把）。
/// 失败语义按调用方契约：鉴权被拒 → 401（调用方据此提示重新登录），
/// 其余上游错误 → 502（原文进 message）。
pub(super) async fn query(
    region: Region,
    token: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<Value, GatewayError> {
    let token = token.trim();
    if token.is_empty() {
        // 调用方只在非空时才会走到这里，这条只是防御（空令牌发出去必 401）
        return Err(GatewayError::with_status(
            401,
            "ZCode 账号缺少编码套餐 API Key，请重新登录或补填凭证",
        ));
    }
    let base = region
        .env_override("MONITOR_BASE_URL")
        .unwrap_or_else(|| region.monitor_base_url().to_string());
    let url = format!("{base}{QUOTA_PATH}");
    // 展示用的来源串（环境变量覆盖过基址时也如实反映）
    let source = format!("{}/api/monitor", base.trim_start_matches("https://"));
    let mut auth_error: Option<GatewayError> = None;
    for (index, form) in AuthForm::ALL.into_iter().enumerate() {
        let headers = vec![("Authorization".to_string(), form.header(token))];
        let response = send_raw("GET", &url, None, &headers, proxy, Some(REQUEST_TIMEOUT_MS))
            .await
            .map_err(|error| transport_error(&error))?;
        // 鉴权失败才换形态重试一次（理由见模块头第 1 条）：记住这次的事实，
        // 万一第二种形态也被拒，透出去的就是它
        if matches!(response.status, 401 | 403) {
            auth_error = Some(GatewayError::with_status(
                401,
                "ZCode 编码套餐 API Key 被上游拒绝（已失效，或不是这一地的密钥），\
                 请重新登录或更换凭证",
            ));
            if index + 1 < AuthForm::ALL.len() {
                continue;
            }
            break;
        }
        return unpack(&source, response);
    }
    Err(auth_error.unwrap_or_else(|| {
        GatewayError::with_status(401, "ZCode 窗口限额查询被上游拒绝")
    }))
}

/// 传输层错误 → 可读错误（超时与其它失败分开，与其它各家同一口径）
fn transport_error(error: &reqwest::Error) -> GatewayError {
    if error.is_timeout() {
        GatewayError::with_status(504, "ZCode 窗口限额查询超时")
    } else {
        GatewayError::with_status(502, format!("ZCode 窗口限额查询失败: {error}"))
    }
}

/// 信封解包：`{code, msg, data}` → 归一化后的余额文档。
///
/// `code` 缺失视为成功（有的网关在成功时干脆不回这个键），非 0 才是失败 ——
/// 与 `coding_key.rs` 的信封口径一致。
fn unpack(source: &str, response: ApiResponse) -> Result<Value, GatewayError> {
    let payload = response.payload.unwrap_or(Value::Null);
    let code = payload.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code == 401 {
        return Err(GatewayError::with_status(
            401,
            "ZCode 编码套餐 API Key 已失效，请重新登录该账号",
        ));
    }
    if !(200..300).contains(&response.status) || code != 0 {
        let detail = payload
            .get("msg")
            .or_else(|| payload.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {}", response.status));
        // 业务码为 0 表示「响应体里没有 code」（HTTP 层错误）—— 那种情况别拼一个
        // 读起来莫名其妙的「（0）」
        let label = if code == 0 { String::new() } else { format!("（{code}）") };
        return Err(GatewayError::with_status(
            502,
            format!("ZCode 窗口限额查询失败{label}：{detail}"),
        ));
    }
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    Ok(normalize(source, &data))
}

/// 上游 `data` → 账号页的统一形状（纯函数：不碰网络、不碰时钟以外的东西）。
///
/// ── 读数的方向一律是「剩余」─────────────────────────────────
/// 上游这几个字段是**已用**口径（`percentage` = 已用百分比），而账号页的余额列
/// 与它旁边的进度条都是**剩余**口径（条画的是「还剩多少」，见 billing 通道那份
/// `remainingPercent` 的说明）。两者混在一列里会让人读反，所以这里在归一化的
/// 第一步就换算成剩余：进度条、`balanceView`、表头读数三处同向。
///
/// 表头读数（`availableView`）取**最紧张的那一档**（剩余比例最低的窗口）——
/// 它回答的是「现在离被拦还有多远」；窗口全都没有比例时退回绝对量读数，
/// 一个都读不出时给中性文案（见 [`headline_of`]）。
fn normalize(source: &str, data: &Value) -> Value {
    let plan_name = plan_label(data);
    let limits: Vec<&Value> = data
        .get("limits")
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    let mut wallets: Vec<Value> = Vec::new();
    // (剩余百分比, 窗口期文案) —— 表头用；取剩余**最少**（最紧张）的那个
    let mut tightest: Option<(f64, String)> = None;
    // 没有比例时的兜底读数（第一份带绝对量的窗口）
    let mut absolute: Option<String> = None;
    for limit in &limits {
        let kind = limit
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        let unit = limit.get("unit").and_then(Value::as_i64);
        let period = period_label(unit, limit.get("number").and_then(Value::as_i64));
        let kind_label = kind_label(&kind);
        let unit_text = kind_unit(&kind);
        let total = limit.get("usage").and_then(number_of);
        let used = limit.get("currentValue").and_then(number_of);
        // 剩余量的取值链：`remaining` → 总额 − 已用（与 billing 通道同一条口径）
        let remaining = limit.get("remaining").and_then(number_of).or_else(|| match (total, used) {
            (Some(total), Some(used)) => Some(total - used),
            _ => None,
        });
        // 已用百分比：`percentage` → 总额与已用现算（2026-02 之后上游只回比例，
        // 因此绝对量这条路只是兜底）
        let percent_used = limit
            .get("percentage")
            .and_then(number_of)
            .map(|value| value.clamp(0.0, PERCENT_MAX))
            .or_else(|| match (total, used) {
                (Some(total), Some(used)) if total > 0.0 => {
                    Some((used / total * PERCENT_MAX).clamp(0.0, PERCENT_MAX))
                }
                _ => None,
            });
        // 剩余占比（0~100）：换算是**在这里一次做完**的（读数方向见本函数的文档）
        let remaining_percent = percent_used.map(|value| (PERCENT_MAX - value).clamp(0.0, PERCENT_MAX));
        let view = wallet_view(total, used, remaining, remaining_percent, &unit_text, limit);
        // 窗口期标识（type 字段）：单位决定档位，认不出落 cycle
        let window = window_code(unit);
        // 表头读数的候选（见 `headline_of`）：有比例的窗口比「剩余最少」，
        // 没有比例的窗口才参与绝对量兜底
        if let Some(value) = remaining_percent {
            if tightest
                .as_ref()
                .map(|(current, _)| value < *current)
                .unwrap_or(true)
            {
                tightest = Some((value, period.clone()));
            }
        } else if absolute.is_none() {
            if let Some(remaining) = remaining {
                absolute = Some(join_number(remaining, &unit_text, "剩 "));
            }
        }
        wallets.push(json!({
            "type": format!("zcode_window_{window}"),
            "displayName": format!("{kind_label}（{period}）"),
            "balance": remaining,
            "balanceView": view,
            // 结构化读数（billing 通道同名字段）：缺失一律 null —— 界面按
            // 「不知道」处理，不拿 0 冒充
            "total": total,
            "used": used,
            "remainingPercent": remaining_percent,
            "planName": plan_name,
        }));
    }
    let available_view = headline_of(tightest, absolute, wallets.len())
        .unwrap_or_else(|| "无额度窗口".to_string());
    let mut subscription = Map::new();
    if let Some(name) = plan_name {
        subscription.insert("planName".to_string(), Value::String(name));
    }
    json!({
        // `available` **不写**（见模块头：写了 0 会让缺省的余额跳过档剔掉这个账号）
        "availableView": available_view,
        // 形状与 billing 通道对齐的占位：这条通道的量纲是「次 / 分钟」，
        // 而读数以比例为主，因此它只在绝对量那一档有意义
        "unit": "",
        "wallets": wallets,
        "subscription": subscription,
        // 排障用：上游原文（前端默认不展示）
        "raw": data,
        "source": source,
    })
}

/// 表头读数的取值链（见 [`normalize`] 的说明）：剩余最少的那个窗口 → 绝对量 →
/// 「读到了几项」。
fn headline_of(
    tightest: Option<(f64, String)>,
    absolute: Option<String>,
    window_count: usize,
) -> Option<String> {
    if let Some((percent, period)) = tightest {
        return Some(format!("{period}剩 {}%", percent_text(percent)));
    }
    if let Some(text) = absolute {
        return Some(text);
    }
    // 窗口在但没有可读的数字（上游改了字段名之类）：如实说「读到了几项」，
    // 不编一个 0 出来
    (window_count > 0).then(|| format!("已读取 {window_count} 项额度窗口"))
}

/// 一个窗口的展示串：有绝对量给「剩 x / y 单位」，只有比例给「剩 p%」，
/// 两者都带上重置时刻（有才带）。
///
/// `remaining_percent` 是**剩余**比例（换算在 [`normalize`] 里做完），
/// 因此这里的两个分支方向一致（都是「还剩多少」）。
fn wallet_view(
    total: Option<f64>,
    used: Option<f64>,
    remaining: Option<f64>,
    remaining_percent: Option<f64>,
    unit_text: &str,
    limit: &Value,
) -> String {
    let mut view = match (total, used) {
        (Some(total), Some(used)) => {
            let left = remaining.unwrap_or((total - used).max(0.0));
            format!(
                "剩 {} / {} {}",
                compact_number(left),
                compact_number(total),
                unit_text
            )
            .trim_end()
            .to_string()
        }
        _ => match remaining_percent {
            Some(value) => format!("剩 {}%", percent_text(value)),
            None => "已读取".to_string(),
        },
    };
    if let Some(text) = reset_text(limit.get("nextResetTime").and_then(Value::as_i64)) {
        view.push_str(" · ");
        view.push_str(&text);
        view.push_str(" 重置");
    }
    view
}

/// 重置时刻 → 本地时区 `MM-DD HH:MM`。
///
/// 上游给的是**绝对时刻**（epoch 毫秒），因此按操作者本机时区渲染 ——
/// 与账号页其它时间列（前端 `formatTime`）同一口径。本机时区在 `chrono::Local`，
/// 取不到（时间戳越界）时返回 None：宁可不写这一段，也不编一个时刻。
fn reset_text(ms: Option<i64>) -> Option<String> {
    let ms = ms.filter(|value| *value > 0)?;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|at| at.format("%m-%d %H:%M").to_string())
}

/// 窗口期文案（`unit` 的数字语义：3 = 每 N 小时、4 = 每天、5 = 每月、6 = 每周）
fn period_label(unit: Option<i64>, number: Option<i64>) -> String {
    match unit {
        // 每 N 小时：`number` 缺失时按 5（与第三方实现的缺省一致）
        Some(3) => format!("每 {} 小时", number.unwrap_or(5)),
        Some(4) => "每天".to_string(),
        Some(5) => "每月".to_string(),
        Some(6) => "每周".to_string(),
        _ => "每周期".to_string(),
    }
}

/// 窗口期标识（`type` 字段用，机器可读）
fn window_code(unit: Option<i64>) -> String {
    match unit {
        Some(3) => "hours".to_string(),
        Some(4) => "daily".to_string(),
        Some(5) => "monthly".to_string(),
        Some(6) => "weekly".to_string(),
        _ => "cycle".to_string(),
    }
}

/// `type` → 量纲名。只认有证据的两档，认不出的**原样显示**（见模块头第 2 条）。
///
/// 措辞对齐官方客户端（`StatusCards.tsx` 的额度卡标题）：`TOKENS_LIMIT` 那两行官方
/// 叫「5 小时 / 每周」，`TIME_LIMIT` 那行官方叫「工具调用」
/// （`settings.usage.entitlementMonthlyMcpUsage`）—— 早先按参考实现写成「使用时长」，
/// 与用户在官方界面里看到的名字对不上（同一个面板、两套名字，最容易被当成两回事）。
fn kind_label(kind: &str) -> String {
    match kind {
        "TOKENS_LIMIT" => "提示次数".to_string(),
        "TIME_LIMIT" => "工具调用".to_string(),
        "" => "额度".to_string(),
        other => other.to_string(),
    }
}

/// 量纲的计量单位（绝对量读数用；认不出的 type 与「没有绝对量」都给空串）。
/// `TIME_LIMIT` 是**工具调用次数**（官方同款语义），不是时长。
fn kind_unit(kind: &str) -> String {
    match kind {
        "TOKENS_LIMIT" => "次".to_string(),
        "TIME_LIMIT" => "次".to_string(),
        _ => String::new(),
    }
}

/// 套餐名：`planName` → `plan` → `plan_type` → `packageName` → `level` 归一。
///
/// `level`（`pro` / `max` / `lite` 这类）走一次大小写归一，与
/// `zcode-switch` 的 `tier_from_level` 同口径；认不出的原样返回。
fn plan_label(data: &Value) -> Option<String> {
    for key in ["planName", "plan", "plan_type", "planType", "packageName"] {
        if let Some(text) = data
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            return Some(text.to_string());
        }
    }
    let level = data
        .get("level")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())?;
    let lower = level.to_lowercase();
    if lower.contains("max") {
        Some("Max".to_string())
    } else if lower.contains("pro") {
        Some("Pro".to_string())
    } else if lower.contains("lite") {
        Some("Lite".to_string())
    } else {
        Some(level.to_string())
    }
}

/// 百分比 → 紧凑串（`40.5` / `52`；一位小数，读的是量级）
fn percent_text(value: f64) -> String {
    format!("{value:.1}").trim_end_matches(".0").to_string()
}

/// 「剩 N 单位」——单位为空时不拼出多余的空格
fn join_number(value: f64, unit_text: &str, prefix: &str) -> String {
    format!("{prefix}{} {unit_text}", compact_number(value))
        .trim_end()
        .to_string()
}
