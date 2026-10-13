//! CodeArts 的余额：两份**互相不知道对方存在**的账，合成一个形状。
//!
//! ── 为什么必须读两处 ────────────────────────────────────────
//! 订阅统计（`/snap-manager/v1/statistics/plugin`）只报**付费套餐**那几个计量表；
//! 限时福利池由**另一个网关**记账（`{benefit_gateway}/api/v1/user/tokens/balance`）。
//! 参考实现里那句注释就是本模块存在的理由：福利模型可以因为额度耗尽报
//! "insufficient quota"，而面板上每一个计量表都还是 0。只读一处 = 显示一个假「还有额度」。
//!
//! ── 三个哨兵值（都是"照实翻"而不是"顺手美化"）──────────────
//!   * `limit <= 0` 表示**该维度无上限**，不是「耗尽」→ 剩余量与百分比都给 -1，
//!     界面上要显示「无上限」而不是「剩 0」。
//!   * 用量超过上限时**如实报大于 100%**：网关在到达上限后仍继续计数，
//!     把超额藏起来就是低估了问题。
//!   * 计量表行的 `show:false` 是上游「这行别显示」的指令 —— 退役的「消息条数」
//!     指标就是带着 `value:-1` 以这种形式回来的。跳过它，别把 -1 当数字画出来。
//!
//! ── 成功判据分两家 ─────────────────────────────────────────
//! 区域 API 看 HTTP 状态；**福利网关把失败塞在 200 的响应体里**，只有
//! envelope 的 `error_code == "0000"` 才算成功。按 HTTP 状态判会让一个
//! `error_code:"9001"` 的错误看起来像成功然后显示空数据。
//!
//! ── benefit 档案从哪来（`claim_benefit` 存在的理由）──────────
//! InferHub 的聊天计费、福利池余额都挂在 opengw 的 **benefit 记录**上，
//! 而这条记录**不是开账号就有的** —— 官方客户端每次初始化都会
//! `POST {benefit_gateway}/api/v1/benefit/claim`（幂等，`initBenefit`），
//! 服务端随之给账号建档；没走过这一步的账号聊天直接被 InferHub 以
//! `InferHub.4004.200: benefit not found` 拒答（2026-10-09 实测：同一批凭据，
//! 官方客户端登录用一下就恢复，我们这边一直 4004）。本模块把同一条
//! claim 纳入余额链路：查到「无档案」（`4004`）时自动补一次再重查，
//! 新账号因此在一次余额查询内自愈，不用再靠官方客户端「渡」一次。

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Value, json};

use crate::server::core::egress;
use crate::server::errors::GatewayError;

use super::chat;
use super::credentials::Credential;
use super::{oauth, redact, signer};

/// 订阅统计（付费套餐计量表）。
pub const STATISTICS_PATH: &str = "/snap-manager/v1/statistics/plugin";
/// 福利池余额（另一个网关）。
pub const BENEFIT_BALANCE_PATH: &str = "/api/v1/user/tokens/balance";
/// benefit 档案的领取/注册（同一网关；官方客户端 `initBenefit` 每次启动都调）。
pub const BENEFIT_CLAIM_PATH: &str = "/api/v1/benefit/claim";

/// 福利网关的「该账号没有福利数据」业务码。
///
/// 实测（Free 套餐账号）：GET 该端点回 HTTP 200 + `{"error_code":"4004",
/// "error_msg":"benefit not found"}` —— 这**不是查询失败**，是上游明说
/// 「这个账号没有福利池」（福利是限时活动下发的，不是每个账号都有；而
/// 每日福利领到的套餐赠送积分进的是另一本账，见模块头）。
/// 把它当失败渲染成「福利网关未读到」会让用户去查一个不存在的问题，
/// 所以解析层把它翻成 `Ok(None)`（第三种状态），与「没读到」分开。
const BENEFIT_ABSENT_CODE: &str = "4004";
/// 同一条的文案兜底（码可能换、文案还没换；两条都命中才算「没有福利」，
/// 避免 4004 将来承载别的语义时被误判成「无福利」而掩盖真错误）。
const BENEFIT_ABSENT_MARKER: &str = "benefit not found";

/// 一个计量表。`credit_*` 三项是「套餐赠送积分」口径，`used/allowance_tokens`
/// 是 token 口径 —— 上游按指标类型给其中一组，另一组留空。
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Meter {
    pub name: String,
    pub label: String,
    pub used_tokens: i64,
    pub allowance_tokens: i64,
    pub credit_total: Option<f64>,
    pub credit_used: Option<f64>,
    pub credit_remaining: Option<f64>,
    /// 上游直接给的百分比；**只有 `>= 0` 才收**（-1 是「不适用」的哨兵）。
    pub used_percent: Option<f64>,
}

impl Meter {
    /// 这一行有没有可显示的东西。全零且没有任何 credit 字段的行是噪声，跳过。
    fn is_empty(&self) -> bool {
        self.used_percent.is_none()
            && self.used_tokens == 0
            && self.allowance_tokens == 0
            && self.credit_total.is_none()
            && self.credit_remaining.is_none()
    }

    /// 一行计量表 → 界面上那一段读数（`balanceView`）。
    ///
    /// ── 为什么要提供商自己给展示串 ───────────────────────────
    /// 账号页的余额明细只认 `balanceView` / `balance` 两个键（别家的钱包都带
    /// `balance`），而本家的订阅统计给的是**两组互不相干的口径**：token 类指标
    /// 只有 used/allowance，积分类指标只有 credit_*。不给一个展示串，那一列就会
    /// 整排显示成「—」—— 明明读到了数，看着却像没读到，正是本模块要避免的那类误读
    /// （与「无上限显示成 0」同一个病，只是方向相反）。
    /// 展示口径放在这里是合适的：什么指标该说「已用 x / y token」、什么该说
    /// 「剩 x / y 积分」，是上游这张表的知识，不是界面该猜的。
    fn view(&self) -> Option<String> {
        if let (Some(remaining), Some(total)) = (self.credit_remaining, self.credit_total) {
            return Some(format!("剩 {} / {} 积分", compact(remaining), compact(total)));
        }
        // 总量 >1 才当真配额：上游给过 `package_token_amount=1`（2026-10-09 实测，
        // 试用版账号周期内没下发过对话 token 包时的占位形状）—— 1 个 token 的
        // 「包」没有任何信息量，还会把「没额度」显示成「剩 0 / 1」的荒谬读数；
        // 退回「已用 N token」（官方同款取向：它的 `calculateTokenPercent` 对
        // `amount <= 0` 返回空，不拿小占位值当真）。
        if self.allowance_tokens > 1 {
            let percent = self.used_percent.map(|value| format!("（{}%）", compact(value))).unwrap_or_default();
            return Some(format!("已用 {} / {} token{}", self.used_tokens, self.allowance_tokens, percent));
        }
        if self.used_tokens > 0 {
            return Some(format!("已用 {} token", self.used_tokens));
        }
        // 只有百分比没有量的行（上游确实给过这种）：0% 是**真信息**，不能又变回「—」
        self.used_percent.map(|value| format!("已用 {}%", compact(value)))
    }
}

/// 数值 → 去掉尾随的 `.0`（积分常是 `300.0`，界面上一眼看成像是一个不精确的量）。
fn compact(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    format!("{value}")
}

/// 订阅统计文档里本模块要用的部分。
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Statistics {
    pub reset_date: String,
    pub plan: String,
    pub plan_name: String,
    pub plan_url: String,
    pub meters: Vec<Meter>,
    pub features: HashMap<String, bool>,
}

/// 福利池的一份额度。
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BenefitBalance {
    pub channel: String,
    pub daily_token_limit: i64,
    pub daily_tokens_used: i64,
    pub monthly_token_limit: i64,
    pub monthly_tokens_used: i64,
    pub total_balance: i64,
}

/// 无上限的返回值（与参考实现同值，界面上显示成「无上限」）。
pub const UNCAPPED: i64 = -1;

impl BenefitBalance {
    /// 日池剩余；**无上限回 [`UNCAPPED`]**（不是 0）。
    pub fn remaining_daily(&self) -> i64 {
        if self.daily_token_limit <= 0 {
            return UNCAPPED;
        }
        (self.daily_token_limit - self.daily_tokens_used).max(0)
    }

    /// 日池已用百分比；无上限回 `-1.0`。超额**如实报 >100**。
    pub fn daily_percent(&self) -> f64 {
        if self.daily_token_limit <= 0 {
            return -1.0;
        }
        self.daily_tokens_used as f64 / self.daily_token_limit as f64 * 100.0
    }
}

/// 错误体先脱敏再截断（顺序不能反：截断可能把秘密切成半截，那时前缀也得清掉）。
/// 上游的错误体可能是整页 HTML，原样进日志等于把凭据与代理诊断信息抄进库。
pub(crate) fn excerpt(text: &str, credential: &Credential) -> String {
    let cleaned = redact::redact(text, credential, false);
    let count = cleaned.chars().count();
    if count <= 300 {
        return cleaned;
    }
    cleaned.chars().take(300).collect::<String>() + "…"
}

fn trim(base: &str) -> String {
    base.trim_end_matches('/').to_string()
}

/// 一次签名请求（GET / POST；本模块三处读数的头集合**各不相同**，别顺手统一）。
///
/// ── 头集合是契约的一部分 ───────────────────────────────────
/// 订阅统计只带 `Accept` / `X-Language` / `plugin-name` / `plugin-version` 四个
/// （没有 `Content-Type`、没有 `Agent-Type`）；福利余额与 claim 走
/// [`benefit_headers`]（官方 BenefitService 的集合）。签名覆盖的是头集合，
/// 多一项少一项签出来的串就不同 —— 与参考实现逐字一致才有「上游会接受」的证据。
async fn signed_request(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    payload: &[u8],
    credential: &Credential,
    host_signed: bool,
    domainless: bool,
) -> Result<(u16, String), GatewayError> {
    let mut signing = oauth::signer_credential(credential);
    if domainless {
        signing.domain_id = String::new();
    }
    let signed = signer::sign(method, url, headers, payload, &signing, host_signed)
        .map_err(|reason| GatewayError::with_status(500, format!("CodeArts 余额请求签名失败：{reason}")))?;
    let is_post = method.eq_ignore_ascii_case("post");
    let verb = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut request = egress::client_for(None)
        .request(verb, url)
        .timeout(std::time::Duration::from_secs(20));
    if is_post {
        request = request.body(payload.to_vec());
    }
    for (name, value) in signed {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request.send().await.map_err(|error| {
        GatewayError::with_status(
            502,
            format!("CodeArts 余额请求失败：{}", egress::describe_error_detail(&error)),
        )
    })?;
    let status = response.status().as_u16();
    Ok((status, response.text().await.unwrap_or_default()))
}

/// 订阅统计的四件套（参考实现逐字一致：多一项签名串就不同）。
fn statistics_headers(language: &str, plugin_version: &str) -> Vec<(String, String)> {
    vec![
        ("Accept".to_string(), "application/json".to_string()),
        ("X-Language".to_string(), language.to_string()),
        ("plugin-name".to_string(), chat::DEFAULT_PLUGIN_NAME.to_string()),
        ("plugin-version".to_string(), plugin_version.to_string()),
    ]
}

/// 福利 claim 的头集合 —— **照官方 BenefitService 的 `claimBenefit` 逐字抄**：
/// `X-Security-Token`（临时凭证的 STS）、`Agent-Type: PromptCenter`、
/// `Content-Type`、`X-Language`。只有 claim 用它（官方只在这条上给过实证）；
/// 余额那条维持实测通过的统计四件套，两套头各按各的证据，别顺手统一。
fn benefit_headers(credential: &Credential) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Agent-Type".to_string(), "PromptCenter".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("X-Language".to_string(), chat::DEFAULT_LANGUAGE.to_string()),
    ];
    if !credential.security_token.is_empty() {
        headers.insert(0, ("X-Security-Token".to_string(), credential.security_token.clone()));
    }
    headers
}

/// 读订阅统计（付费套餐计量表）。
pub async fn fetch_statistics(
    base: &str,
    credential: &Credential,
    language: &str,
    plugin_version: &str,
) -> Result<Statistics, GatewayError> {
    let url = format!("{}{STATISTICS_PATH}", trim(base));
    let (status, body) = signed_request(
        "GET",
        &url,
        &statistics_headers(language, plugin_version),
        b"",
        credential,
        false,
        false,
    )
    .await?;
    if status != 200 {
        return Err(GatewayError::with_status(
            i32::from(status),
            format!("CodeArts 统计接口返回 HTTP {status}：{}", excerpt(&body, credential)),
        ));
    }
    parse_statistics(&body)
}

/// 解析统计文档。字段名逐个照上游（`metrics[].usage_token_num` 这类拼写不能改）。
pub fn parse_statistics(body: &str) -> Result<Statistics, GatewayError> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|error| GatewayError::with_status(502, format!("CodeArts 统计响应不是合法 JSON：{error}")))?;
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_i64).unwrap_or(0);
    let float = |value: &Value, key: &str| value.get(key).and_then(Value::as_f64);
    let mut meters = Vec::new();
    for metric in parsed.get("metrics").and_then(Value::as_array).into_iter().flatten() {
        let name = text(metric, "name");
        // `show:false` 是上游的「这行别显示」指令（退役指标带着 value:-1 走这条路）
        if name.is_empty() || !metric.get("show").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let mut meter = Meter {
            name: name.clone(),
            label: quota_meter_label(&name).unwrap_or_else(|| name.clone()),
            used_tokens: number(metric, "usage_token_num"),
            allowance_tokens: number(metric, "package_token_amount"),
            credit_total: float(metric, "package_credit_amount"),
            credit_used: float(metric, "package_credit_used"),
            credit_remaining: float(metric, "package_credit_remain"),
            used_percent: None,
        };
        if let Some(value) = float(metric, "value") {
            if value >= 0.0 {
                meter.used_percent = Some(value);
            }
        }
        if meter.is_empty() {
            continue;
        }
        meters.push(meter);
    }
    let package = parsed.get("package").filter(|value| !value.is_null());
    let mut features = HashMap::new();
    if let Some(package) = &package {
        for feature in package.get("features").and_then(Value::as_array).into_iter().flatten() {
            let name = text(feature, "name");
            if !name.is_empty() {
                features.insert(name, feature.get("enable").and_then(Value::as_bool).unwrap_or(false));
            }
        }
    }
    Ok(Statistics {
        reset_date: text(&parsed, "end_date"),
        plan: package.as_ref().map(|value| text(value, "spec_code")).unwrap_or_default(),
        plan_name: package
            .as_ref()
            .map(|value| {
                let en = text(value, "package_name_en");
                if en.is_empty() { text(value, "package_name_cn") } else { en }
            })
            .unwrap_or_default(),
        plan_url: package.as_ref().map(|value| text(value, "package_url")).unwrap_or_default(),
        meters,
        features,
    })
}

/// 计量表的中文标签 —— **逐字照参考实现那张表**（`quota.go` 的 `quotaMeterLabels`）。
///
/// 认不出来就回 None、由调用方回落上游原名（参考实现同一句注释：留着原名比
/// 藏起来有用）。这里刻意**不猜**：第一版我按「指标名大概长什么样」写了
/// `chat_token` / `code_completion` 三条，真跑一次统计接口才发现上游给的是
/// `usageTokenChatMessages` / `usageTotalPackageCredit` 那一族名字 —— 三条全是死码，
/// 而界面上每一行都显示成英文原名。教训：这类对照表只能抄，不能推。
fn quota_meter_label(name: &str) -> Option<String> {
    Some(match name {
        "usageDataCodeCompletions" => "代码补全额度",
        "usageDataChatMessages" => "对话消息额度",
        "usageTokenChatMessages" => "对话 token 额度",
        "usageTotalPackageCredit" => "套餐积分",
        "usageBasicPackageCredit" => "基础包积分",
        "usageOnDemandPackageCredit" => "按需付费积分",
        "usageBonusPackageCredit" => "赠送积分",
        _ => return None,
    }
    .to_string())
}

/// 读福利池余额（第二个网关）。
///
/// 签名口径与区域 API **不同**：带 Host、**不带 `X-Domain-Id`**（参考实现把
/// `DomainID` 清空后再签）。开发者网关不是区域活动服务，两套契约不能混用。
///
/// 返回 `Ok(None)` = 上游明说「该账号没有福利数据」（见 `BENEFIT_ABSENT_CODE`），
/// 与「读取失败」（`Err`）分开 —— 界面才能把「没有福利」和「没读到」分得开。
pub async fn fetch_benefit_balance(gateway: &str, credential: &Credential) -> Result<Option<BenefitBalance>, GatewayError> {
    let url = format!("{}{BENEFIT_BALANCE_PATH}", trim(gateway));
    // 头集合维持**实测通过**的那一套（统计四件套；2026-09-27 现网 trial/Free 账号
    // 都拿回过 envelope）—— claim 才用官方 BenefitService 的头集合，两个端点各按
    // 各自的证据走，别顺手统一（统一 = 让其中一条失去实证）。
    let headers = statistics_headers(chat::DEFAULT_LANGUAGE, chat::DEFAULT_PLUGIN_VERSION);
    let (status, body) = signed_request("GET", &url, &headers, b"", credential, true, true).await?;
    if status != 200 {
        return Err(GatewayError::with_status(i32::from(status), format!("CodeArts 福利余额返回 HTTP {status}")));
    }
    parse_benefit_balance(&body, credential)
}

/// benefit 档案的领取/注册（`POST {gateway}/api/v1/benefit/claim`，空 body）。
///
/// 与官方 BenefitService 的 `claimBenefit` 逐字同源（头集合见 [`benefit_headers`]，
/// 签名口径与 [`fetch_benefit_balance`] 相同：带 Host、不带 `X-Domain-Id` —— 官方
/// 签这条时本就不放 DomainId）。**幂等**：官方每次启动、每次选中免费模型都调它，
/// 重复调用的返回只有「已存在」与「新建档」两种措辞，语义上不会多拿一次权益。
///
/// 成功判据与余额同一家：看 envelope 的 `error_code == "0000"`（HTTP 一律 200）。
pub async fn claim_benefit(gateway: &str, credential: &Credential) -> Result<String, GatewayError> {
    let url = format!("{}{BENEFIT_CLAIM_PATH}", trim(gateway));
    let headers = benefit_headers(credential);
    let (status, body) = signed_request("POST", &url, &headers, b"{}", credential, true, true).await?;
    if status != 200 {
        return Err(GatewayError::with_status(i32::from(status), format!("CodeArts benefit 领取返回 HTTP {status}")));
    }
    parse_benefit_claim(&body, credential)
}

/// claim 的解析：`0000` 成功（`result` 可能是空对象或一句说明，转成人话返回），
/// 其余码如实报错。独立于 `parse_benefit_claim` 的**网络**半边以便单测。
pub fn parse_benefit_claim(body: &str, credential: &Credential) -> Result<String, GatewayError> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|error| GatewayError::with_status(502, format!("CodeArts benefit 领取响应不是合法 JSON：{error}")))?;
    let code = parsed.get("error_code").and_then(Value::as_str).unwrap_or("");
    if code != "0000" {
        let message = parsed.get("error_msg").and_then(Value::as_str).unwrap_or("");
        return Err(GatewayError::with_status(
            502,
            format!("CodeArts benefit 领取返回 {code}：{}", excerpt(message, credential)),
        ));
    }
    let result = parsed.get("result").cloned().unwrap_or(Value::Null);
    let text = result.as_str().map(str::to_string).unwrap_or_else(|| {
        // result 常见是 `{"claimed":true,...}` 一类；取不到字段就照实给一句中性的
        result.get("msg").or_else(|| result.get("message"))
            .and_then(Value::as_str).map(str::to_string)
            .unwrap_or_else(|| "官方已确认 benefit 档案".to_string())
    });
    Ok(text)
}

/// 福利网关的解析：**成功看 envelope，不看 HTTP 状态**。
///
/// 三种结局：`Ok(Some)` 读到福利；`Ok(None)` 上游明说没有该账号的福利数据
/// （`4004 benefit not found`，正常状态）；`Err` 才是真失败。
pub fn parse_benefit_balance(body: &str, credential: &Credential) -> Result<Option<BenefitBalance>, GatewayError> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|error| GatewayError::with_status(502, format!("CodeArts 福利余额响应不是合法 JSON：{error}")))?;
    let code = parsed.get("error_code").and_then(Value::as_str).unwrap_or("");
    if code != "0000" {
        let message = parsed.get("error_msg").and_then(Value::as_str).unwrap_or("");
        // 「没有福利数据」与「查询失败」是两回事：前者是账号的正常状态
        // （福利按活动下发，Free 账号常常没有），后者才该冒到界面当警告。
        // 码与文案**都**命中才算 —— 只认码的话，4004 将来承载别的语义时
        // 会被静默吞成「无福利」；只认文案的话，换码就失效。
        if code == BENEFIT_ABSENT_CODE && message.to_lowercase().contains(BENEFIT_ABSENT_MARKER) {
            return Ok(None);
        }
        return Err(GatewayError::with_status(
            502,
            format!("CodeArts 福利余额返回 {code}：{}", excerpt(message, credential)),
        ));
    }
    let result = parsed
        .get("result")
        .filter(|value| !value.is_null())
        .ok_or_else(|| GatewayError::with_status(502, "CodeArts 福利余额响应没有 result"))?;
    let number = |key: &str| result.get(key).and_then(Value::as_i64).unwrap_or(0);
    Ok(Some(BenefitBalance {
        channel: result.get("channel").and_then(Value::as_str).unwrap_or("").to_string(),
        daily_token_limit: number("daily_token_limit"),
        daily_tokens_used: number("daily_tokens_used"),
        monthly_token_limit: number("monthly_token_limit"),
        monthly_tokens_used: number("monthly_tokens_used"),
        total_balance: number("total_balance"),
    }))
}

/// 一次余额查询 = **并发**打两个网关。
///
/// 为什么并发（而不是先订阅后福利）：参考实现把这条写得很硬 ——
/// 「自动额度刷新绝不能等另一台网关」。顺序写会把两家的往返时间相加，
/// 而批量查询时每个账号都付一遍。返回的是两个 `Result`：
/// **一边失败不抹掉另一边**（界面上「读到 0」与「没读到」必须分得开）。
///
/// 福利那一侧的 `Ok(None)` 是「上游明说没有该账号的福利数据」，不是失败也不是
/// 读到 0 —— 三种状态在 `query_usage` 里分别落成 wallets / benefitAbsent / benefitError。
pub async fn fetch_both(
    base: &str,
    gateway: &str,
    credential: &Credential,
    language: &str,
    plugin_version: &str,
) -> (Result<Statistics, GatewayError>, Result<Option<BenefitBalance>, GatewayError>) {
    futures::future::join(
        fetch_statistics(base, credential, language, plugin_version),
        fetch_benefit_balance(gateway, credential),
    )
    .await
}

/// 两份账合成 `query_usage` 的归一化形状（契约见 `adapter.rs` 的 `query_usage`）。
///
/// `available` 只在**有积分数值**时给数：福利池是 token 不是积分，把两家的数
/// 加在一起是一个谁也对不上的值 —— 宁可 null，让界面按 wallets 逐项显示。
pub fn usage_document(statistics: Option<&Statistics>, benefit: Option<&BenefitBalance>) -> Value {
    let mut wallets = Vec::new();
    let mut credit_left: Option<f64> = None;
    if let Some(statistics) = statistics {
        for meter in &statistics.meters {
            wallets.push(json!({
                "type": format!("meter:{}", meter.name),
                "displayName": meter.label,
                "balanceView": meter.view(),
                "usedTokens": meter.used_tokens,
                "allowanceTokens": meter.allowance_tokens,
                "usedPercent": meter.used_percent,
                "creditTotal": meter.credit_total,
                "creditUsed": meter.credit_used,
                "creditRemaining": meter.credit_remaining,
            }));
            if let Some(left) = meter.credit_remaining {
                credit_left = Some(credit_left.unwrap_or(0.0) + left);
            }
        }
    }
    if let Some(benefit) = benefit {
        // 无上限不是「剩 0」：balance 给 null + unlimited:true，并**同时给一句
        // 人话**（`balanceView`）—— 界面的明细行只认这两个键之一，光有
        // `unlimited: true` 它仍然会画成一个「—」，与「没读到」长得一样。
        let remaining = benefit.remaining_daily();
        let uncapped = remaining == UNCAPPED;
        let view = if uncapped {
            format!("无上限（已用 {} token）", benefit.daily_tokens_used)
        } else {
            format!(
                "剩 {} / {} token",
                remaining, benefit.daily_token_limit
            )
        };
        wallets.push(json!({
            "type": "benefit_daily_tokens",
            "displayName": "福利模型日额度",
            "balance": if uncapped { Value::Null } else { Value::from(remaining) },
            "balanceView": view,
            "unlimited": uncapped,
            "usedTokens": benefit.daily_tokens_used,
            "allowanceTokens": benefit.daily_token_limit,
            "usedPercent": if benefit.daily_percent() < 0.0 { Value::Null } else { Value::from(benefit.daily_percent()) },
            "note": "超额如实显示（上游到达上限后仍继续计数）",
        }));
    }
    json!({
        "available": credit_left,
        "unit": "积分",
        "wallets": wallets,
        "subscription": statistics.map(|statistics| json!({
            "planName": statistics.plan_name,
            "planCode": statistics.plan,
            "planUrl": statistics.plan_url,
            "resetDate": statistics.reset_date,
        })).unwrap_or(Value::Null),
        // 两份原始文档都留着：这个面板最常见的排障问题是「显示 0 到底是没额度
        // 还是没读到」，只有原始响应能回答
        "raw": {
            "statistics": statistics.map(|statistics| serde_json::to_value(statistics).unwrap_or(Value::Null)),
            "benefit": benefit.map(|benefit| serde_json::to_value(benefit).unwrap_or(Value::Null)),
        },
        "note": "福利领取到账的是**套餐赠送积分**，不增加福利模型 token 池（两个账户各记各的）",
    })
}

#[cfg(test)]
mod tests {
    //! 两处账的解析与三个哨兵值。这里的错误全是「显示成 0 但其实没读到」那一类，
    //! 所以断言都钉在**边界值**上，不钉文案。
    use super::*;

    fn credential() -> Credential {
        Credential {
            access_key_id: "AK".into(),
            secret_access_key: "SK".into(),
            security_token: "sts".into(),
            ..Default::default()
        }
    }

    /// 真响应形状（字段名逐字照上游，别顺手"规范化"成驼峰）。
    /// 五行各代表一种处置：正常行 / `show:false` 的退役指标 / 全空行 /
    /// 只有 `value:0` 的行（0% 是**真信息**，不是空）/ 只有积分的行。
    const STATISTICS: &str = r#"{
      "end_date": "2026-10-01",
      "show": {"metrics": true, "package": true},
      "metrics": [
        {"name": "chat_token", "value": 42.5, "usage_token_num": 85000, "package_token_amount": 200000, "show": true},
        {"name": "chat_message", "value": -1, "usage_token_num": 0, "package_token_amount": 0, "show": false},
        {"name": "unknown_meter", "usage_token_num": 0, "package_token_amount": 0, "show": true},
        {"name": "zero_percent", "value": 0, "usage_token_num": 0, "package_token_amount": 0, "show": true},
        {"name": "code_completion", "usage_token_num": 10, "package_token_amount": 0,
         "package_credit_remain": 300.0, "package_credit_amount": 500.0, "show": true}
      ],
      "package": {"spec_code": "PRO", "package_name_en": "专业版", "package_name_cn": "CN Name",
                  "package_url": "https://example.invalid/p",
                  "features": [{"name": "agent_mode", "enable": true}, {"name": "", "enable": true}]}
    }"#;

    #[test]
    fn hidden_and_empty_rows_are_dropped_but_real_ones_kept() {
        let parsed = parse_statistics(STATISTICS).expect("真形状应当能解");
        let names: Vec<&str> = parsed.meters.iter().map(|meter| meter.name.as_str()).collect();
        assert_eq!(
            vec!["chat_token", "zero_percent", "code_completion"],
            names,
            "show:false 的退役指标与「什么都没有」的行不该出现，但 value:0 的行要留"
        );
        assert_eq!("2026-10-01", parsed.reset_date);
        assert_eq!("专业版", parsed.plan_name, "英文名优先");
        assert_eq!(Some(&true), parsed.features.get("agent_mode"));
        assert_eq!(1, parsed.features.len(), "空名的 feature 不进表");
    }

    #[test]
    fn a_negative_value_is_a_sentinel_while_zero_is_a_real_percentage() {
        let parsed = parse_statistics(STATISTICS).unwrap();
        let percent = |name: &str| parsed.meters.iter().find(|meter| meter.name == name).map(|meter| meter.used_percent);
        assert_eq!(Some(Some(42.5)), percent("chat_token"));
        assert_eq!(Some(Some(0.0)), percent("zero_percent"), "0% 是「一点没用」，要显示");
        // 那行 `value:-1` 同时带着 show:false 被丢掉了；单独再验一次
        // 「即便 show:true，-1 也不能进 used_percent」
        let negative_only = r#"{"metrics":[{"name":"m","value":-1,"usage_token_num":7,"package_token_amount":9,"show":true}]}"#;
        let row = &parse_statistics(negative_only).unwrap().meters[0];
        assert_eq!(None, row.used_percent, "-1 不进 used_percent");
        assert_eq!(7, row.used_tokens, "但这一行本身还有内容，不能整行丢");
    }

    #[test]
    fn an_unknown_meter_label_falls_back_to_the_upstream_name() {
        let parsed = parse_statistics(&STATISTICS.replace("code_completion", "brand_new_thing")).unwrap();
        let row = parsed.meters.iter().find(|meter| meter.name == "brand_new_thing").expect("行还在");
        assert_eq!("brand_new_thing", row.label, "认不出就照上游原名，别编一个中文标签");
    }

    /// `package_token_amount=1` 是**占位**不是配额（2026-10-09 实测于试用版账号
    /// 周期内没下发对话 token 包的形状）：1 个 token 的「包」画成「已用 0 / 1」
    /// 是荒谬读数，还会被误读成「马上用完」。≤1 的总量退回「已用 N / 百分比」。
    #[test]
    fn a_placeholder_token_amount_of_one_is_not_a_real_quota() {
        let body = r#"{"metrics":[{"name":"usageTokenChatMessages","value":0,"usage_token_num":0,"package_token_amount":1,"show":true}]}"#;
        let row = &parse_statistics(body).unwrap().meters[0];
        assert_eq!(Some("已用 0%".to_string()), row.view(), "占位包不该拼出「0 / 1 token」");
        let real = r#"{"metrics":[{"name":"usageTokenChatMessages","value":42.5,"usage_token_num":85000,"package_token_amount":200000,"show":true}]}"#;
        assert_eq!(
            Some("已用 85000 / 200000 token（42.5%）".to_string()),
            parse_statistics(real).unwrap().meters[0].view(),
            "真总量（>1）照旧拼「已用 x / y token」"
        );
    }

    /// 上游**真实**给回的指标名（2026-09-27 从现网统计接口抓的，一个 trial 账号）。
    /// 这张表只能钉住，不能"以后再说"：名字对不上时界面每行都显示英文原名，
    /// 而没有人会报错 —— 典型的静默降级。
    const REAL_STATISTICS: &str = r#"{
      "end_date": "2026-10-20",
      "metrics": [
        {"name":"usageTokenChatMessages","value":0,"usage_token_num":3541691,"package_token_amount":0,"show":true},
        {"name":"usageTotalPackageCredit","value":5,"package_credit_amount":10917.43,"package_credit_remain":5172.57,"show":true},
        {"name":"usageBasicPackageCredit","value":65,"package_credit_amount":493.0,"package_credit_remain":172.57,"show":true},
        {"name":"usageOnDemandPackageCredit","value":0,"package_credit_remain":0,"show":true},
        {"name":"usageBonusPackageCredit","value":0,"package_credit_remain":5000.0,"show":true}
      ],
      "package": {"spec_code":"codearts.agent.individual.trial","package_name_en":"Trial"}
    }"#;

    #[test]
    fn the_real_metric_names_all_get_a_localised_label() {
        let parsed = parse_statistics(REAL_STATISTICS).expect("现网形状");
        assert_eq!(5, parsed.meters.len(), "五行都该留下");
        for meter in &parsed.meters {
            assert!(
                meter.label.chars().next().map_or(false, |c| !c.is_ascii_alphabetic()),
                "{} 没拿到中文标签（label={:?}）—— 对照表漏了这一条",
                meter.name,
                meter.label
            );
        }
        let total = &parsed.meters[1];
        assert_eq!("套餐积分", total.label);
        assert_eq!(Some(5172.57), total.credit_remaining);
        assert_eq!(Some(5.0), total.used_percent);
        // available = 各行剩余积分之和（按需那行是 0，不是「没读到」）
        let document = usage_document(Some(&parsed), None);
        assert_eq!(Some(10_345.14), document["available"].as_f64().map(|v| (v * 100.0).round() / 100.0));
    }

    #[test]
    fn benefit_success_is_judged_by_the_envelope_not_the_transport() {
        let ok = r#"{"error_code":"0000","error_msg":"success","result":{"channel":"codearts","daily_token_limit":10000000,"daily_tokens_used":10231428,"monthly_token_limit":0,"monthly_tokens_used":10313740,"total_balance":0}}"#;
        let balance = parse_benefit_balance(ok, &credential())
            .expect("0000 是成功")
            .expect("有 result 就该有读数");
        assert_eq!(10_000_000, balance.daily_token_limit);
        // 超额：如实报，不藏
        assert_eq!(0, balance.remaining_daily(), "用超了剩余就是 0");
        assert!(balance.daily_percent() > 100.0, "超额要显示成 >100%，实际 {}", balance.daily_percent());

        let failed = r#"{"error_code":"9001","error_msg":"channel not found"}"#;
        let error = parse_benefit_balance(failed, &credential()).expect_err("200 + 非 0000 是失败");
        assert!(error.message.contains("9001"), "错误里要带上游的码：{}", error.message);
        assert!(parse_benefit_balance(r#"{"error_code":"0000"}"#, &credential()).is_err(), "缺 result 不能当成功");
    }

    /// claim 的 envelope 判据与余额同一家（`0000` 才是成功，HTTP 一律 200）。
    /// `result` 的形状上游给过空对象与字符串两种，取不到字段时给中性确认句 ——
    /// 「已建档」这个事实本身就是返回值，别让解析对形状挑剔。
    #[test]
    fn benefit_claim_successes_all_read_as_confirmed() {
        let credential = credential();
        let confirmed = parse_benefit_claim(r#"{"error_code":"0000","error_msg":"success","result":{}}"#, &credential)
            .expect("0000 + 空 result 也是成功");
        assert!(confirmed.contains("benefit"), "取不到字段要有中性确认句：{confirmed}");
        let with_msg = parse_benefit_claim(r#"{"error_code":"0000","result":"already claimed"}"#, &credential)
            .expect("字符串 result 原样转述");
        assert_eq!("already claimed", with_msg);
        let failed = parse_benefit_claim(r#"{"error_code":"9001","error_msg":"auth failed"}"#, &credential)
            .expect_err("非 0000 如实报错");
        assert!(failed.message.contains("9001") && failed.message.contains("auth failed"), "{}", failed.message);
    }

    /// 「该账号没有福利数据」是**正常状态**，不是错误：上游回
    /// `4004 benefit not found`（实测于 Free 套餐账号），解析层要把它翻成
    /// `Ok(None)` —— 若翻成 Err，界面会把一个正常状态渲染成「福利网关未读到」
    /// 的警告，用户去查一个不存在的问题。
    #[test]
    fn a_missing_benefit_is_an_absence_not_a_failure() {
        let absent = r#"{"error_code":"4004","error_msg":"benefit not found"}"#;
        assert!(matches!(parse_benefit_balance(absent, &credential()), Ok(None)), "4004 + benefit not found 是「没有福利」");

        // 码与文案**都**要命中：只认码会让 4004 将来承载别的语义时被静默吞掉，
        // 只认文案会让换码（比如 4005）失效 —— 两条负向都钉住。
        let other_message = r#"{"error_code":"4004","error_msg":"something else"}"#;
        assert!(parse_benefit_balance(other_message, &credential()).is_err(), "4004 配别的文案要如实报错");
        let other_code = r#"{"error_code":"4005","error_msg":"benefit not found"}"#;
        assert!(parse_benefit_balance(other_code, &credential()).is_err(), "别的码配同一文案也要如实报错");
    }

    #[test]
    fn an_uncapped_pool_reports_uncapped_not_zero() {
        let balance = BenefitBalance { daily_token_limit: 0, daily_tokens_used: 5, ..Default::default() };
        assert_eq!(UNCAPPED, balance.remaining_daily(), "无上限 ≠ 剩 0");
        assert_eq!(-1.0, balance.daily_percent());
        let negative = BenefitBalance { daily_token_limit: -1, ..Default::default() };
        assert_eq!(UNCAPPED, negative.remaining_daily());
    }

    #[test]
    fn the_usage_document_keeps_uncapped_distinguishable_from_exhausted() {
        let uncapped = BenefitBalance { daily_token_limit: 0, daily_tokens_used: 5, ..Default::default() };
        let document = usage_document(None, Some(&uncapped));
        let wallet = &document["wallets"][0];
        assert_eq!(Value::Null, wallet["balance"], "无上限不能显示成一个数");
        assert_eq!(Some(true), wallet["unlimited"].as_bool());
        assert_eq!(
            Some("无上限（已用 5 token）"),
            wallet["balanceView"].as_str(),
            "无上限也得给一句展示串：界面明细只认 balanceView / balance，光有 unlimited 标记会画成「—」"
        );

        let exhausted = BenefitBalance { daily_token_limit: 100, daily_tokens_used: 100, ..Default::default() };
        let wallet = usage_document(None, Some(&exhausted))["wallets"][0].clone();
        assert_eq!(Some(false), wallet["unlimited"].as_bool());
        assert_eq!(Some(0), wallet["balance"].as_i64(), "真的用完了才显示 0");
        assert_eq!(Some("剩 0 / 100 token"), wallet["balanceView"].as_str());

        // 两边都没读到 → available 是 null，不是 0
        assert!(usage_document(None, None)["available"].is_null());
        let statistics = Statistics {
            meters: vec![Meter { name: "code_completion".into(), label: "代码补全".into(), credit_remaining: Some(300.0), ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(Some(300.0), usage_document(Some(&statistics), None)["available"].as_f64());
    }

    /// 订阅统计的每一行都要带一句展示串。
    ///
    /// 三种行各代表一种口径，缺任何一种都会退回「—」：token 量（已用/上限）、
    /// 积分量（剩/总）、只有百分比。这条与上面那条是同一个毛病的两面 ——
    /// 「读到了却显示成没读到」。
    #[test]
    fn every_meter_row_carries_a_readable_view() {
        let rows = vec![
            Meter {
                name: "chat_token".into(),
                label: "对话 token 额度".into(),
                used_tokens: 85_000,
                allowance_tokens: 200_000,
                used_percent: Some(42.5),
                ..Default::default()
            },
            Meter {
                name: "code_completion".into(),
                label: "代码补全".into(),
                credit_total: Some(500.0),
                credit_remaining: Some(300.0),
                ..Default::default()
            },
            Meter {
                name: "zero_percent".into(),
                label: "只有百分比".into(),
                used_percent: Some(0.0),
                ..Default::default()
            },
        ];
        let document = usage_document(Some(&Statistics { meters: rows, ..Default::default() }), None);
        let views: Vec<&str> = document["wallets"]
            .as_array()
            .expect("wallets 是数组")
            .iter()
            .map(|wallet| wallet["balanceView"].as_str().unwrap_or("<空>"))
            .collect();
        assert_eq!(vec!["已用 85000 / 200000 token（42.5%）", "剩 300 / 500 积分", "已用 0%"], views,
            "300.0 要写成 300（尾随的 .0 看着像不精确的量），只有百分比时也不能回落到「—」");
    }
}
