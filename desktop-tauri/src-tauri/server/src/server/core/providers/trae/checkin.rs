//! Trae 的每日签到（`checkin_credits/status` + `.../claim`）。
//!
//! ── 协议来源 ──────────────────────────────────────────────
//! 参考实现 `cpa-multi-plugins/plugins/trae`：协议层 `upstream/client.go`
//! （CheckinStatus / CheckinClaim / 探测体表 / 设备号生成）、节奏层
//! `scheduler/scheduler.go`（一轮流程与「不自动重试」的定案）、面板层
//! `management.go`（无 deviceId 硬拦、回查未翻转不算成功）。这一条链上
//! 每一步看着都可以"顺手简化"，而每一处简化的后果都是官方风控。
//!
//! ── 为什么现在做签到（usage.rs 旧注释里写过「不做」，那半句已收回）──
//! SOLO 已转积分制：模型调用消耗的就是签到钱包那份积分（参考实现
//! `panel.html:304`，调度器给池子打分也算它）。不做签到等于每天白丢一笔额度。
//! 那条链真正的前科不是"签到会被封"，而是**签到姿势**会被风控：
//!   · 设备号跨轮复用（风控画像里的可疑项）—— 这里每轮全新（见 `checkin_device_id`）；
//!   · 同一账号混用多套客户端身份 —— 这里只发一套 ug 画像，
//!     换鉴权方案时也只动 `Authorization` 一个头；
//!   · 9074 之后反复重试（参考实现有当日指数退避定时器）—— 这里不搬。
//! 因此态度是「按定稿姿势碰，且每天只发该发的几次请求」。
//!
//! ── 一轮到底发几个请求（这是本模块的安全边界）──────────────────
//! 状态 1 发 + （未签时）领取 1 发 + 同设备号回查 1 发 = **常态 3 发**；
//! 请求体恒 `{}`（见 `probe_bodies`），最坏情况因 9074 不换方案而止于一发。
//! 撞上 `1001`（服务端作废这张 JWT）时驱动会换发新令牌后再走一轮，封顶两轮。
//! **不搬**参考实现的当日指数退避定时器（1m→2m→…→2h、每日 10 次）：那套是
//! 为「0 点整官方重置瞬间的洪峰」调的，而本仓的定时签到时刻由用户在设置页决定，
//! 再叠一层后台重试会把「一天三次」变成「一天几十次」——恰好是前科的成因。
//! 撞 9074 时如实回报、当日不再自动重试；想再试就点面板按钮，一次点击一轮。
//!
//! ── claim 的 `code:0` 是**假成功**陷阱 ──────────────────────
//! 当日已签过的账号对**任何** device_id 都回 `code:0`（参考实现实测）。
//! 所以判"这次真领到了"不看 claim 的码，只看**同一 device_id 回查**之后
//! `checked_in` / `did_checked_in` 有没有翻转。回查没确认就报 `success:false`，
//! 绝不报「签到成功」——那会让界面亮绿、当日台账落库，而实际什么也没到账。
//!
//! ── claim 形状与其他几家对齐 ───────────────────────────────
//! 返回 `{success, msg, ...}`，`billing::checkin` 的汇总与 `mark_checkin` 只认
//! 这两个字段，本家不另立口径：今日已签 → `success:false` + `alreadyCompleted:true`
//! + msg 带「今日已签到」；真领到 → `success:true` + msg 带奖励数额。
//!
//! ⚠️ 数额是上游**报出的当日奖励**，不是到账余额 —— 实测一个 SOLO 账号官方报
//!   200（100+100）、积分池只长 100。两个分量原样透出（`checkinCredits` /
//!   `checkinExtraCredits`），文案不替上游的名义数做到账背书。

use std::time::Duration;

use serde_json::{Value, json};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::adapter::{account_proxy, read_record, renew_forced, renew_if_due};
use super::credentials::Credential;
use super::errors::classify;
use super::http::post_json;
use super::usage::{UG_HOST, ug_headers};

/// 签到状态查询（参考实现 `EpCheckinStatus`）。
const EP_CHECKIN_STATUS: &str = "/trae/api/v2/ug/checkin_credits/status";
/// 领取（参考实现 `EpCheckinClaim`）。
const EP_CHECKIN_CLAIM: &str = "/trae/api/v2/ug/checkin_credits/claim";

/// 业务码：成功。
const CODE_OK: i64 = 0;
/// 业务码：`当前参与用户太多，请稍后再试`。
///
/// 这一码有两家相反的说法：参考实现反编译官方后判「`req_source` 与令牌的产品
/// 谱系错配同样回 9074」；同族端点的另一家公开实现实测判「device 维度限流，
/// 换张派生号就领得到」。本仓的一手记录偏向后者（空体——没有 `req_source`
/// 可错配——撞到 9074，换一轮新 device id 十一秒后就领到了），但 11 秒的间隔
/// 同样能解释成"名额松开"，不足以下定论。维持现状：一发即回报、当日不自动
/// 重试。真要判别：下次撞 9074 时**复用同一个 device id** 再打一发，复用还能
/// 过就是时间窗，复用必败才是 device 维度。
const CODE_REJECTED: i64 = 9074;

/// 业务码：**服务端已作废这张 JWT**，而本地 `expiredAt` 看不出来。
///
/// 出处是同一族端点（`ug/checkin_credits/*` + `pay/ide_user_ent_usage`）上的
/// 第三方判定：`code == 1001` 与文案 `not able to authenticate` 一并当作鉴权
/// 失效，动作是换发新 JWT 后重放一次。参考实现的码表里没有这一码；
/// 本仓一手验证过语义：连拒 1001 的账号刷新令牌后当天就签到成功 ——
/// 不是"名额"、不是"没资格"，就是令牌被服务端作废而 `expiredAt` 没反映。
const CODE_TOKEN_DEAD: i64 = 1001;

/// 单次请求超时。与 `usage.rs` 同一个数：这条族没有流式，`egress` 默认的
/// 600 秒读超时（留给 SSE 的）会把一次批量签到拖成整页转圈。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// 错误体进消息的截断长度（与 `usage.rs` 同一口径）。
const ERROR_BODY_HEAD: usize = 200;

/// 鉴权方案。参考实现：`Cloud-IDE-JWT` 优先，`Bearer` 兜底；非 9074 的业务码
/// 才换方案（9074 走"换探测体"那一支 —— 而探测体现在只有一发）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scheme {
    Jwt,
    Bearer,
}

impl Scheme {
    fn next(self) -> Option<Scheme> {
        match self {
            Self::Jwt => Some(Self::Bearer),
            Self::Bearer => None,
        }
    }

    fn authorization(self, access_token: &str) -> String {
        match self {
            Self::Jwt => format!("Cloud-IDE-JWT {access_token}"),
            Self::Bearer => format!("Bearer {access_token}"),
        }
    }
}

/// 签到这一发的请求体：**恒为空对象**。
///
/// 这里以前照参考实现轮询三种 `req_source`，依据是「谱系错配会 9074」。收成
/// 一条的依据有两条，都不是猜的：同族端点的另一家实现恒发 `{}`（`req_source`
/// 在它那儿出现在另一条接口上，本来就不是签到在读的字段）；本仓第一次撞到的
/// 9074 就是拿空体发出来的 —— 没有 `req_source` 可错配，"谱系探测不对"这条
/// 解释在那一发上直接不成立。`round_trip` 仍按"体用尽则收"分派，所以现在的
/// 形状是一发即回报；真在自己日志里攒出一次 9074 时，这里就是加回序列的挂点
/// （`variant` 参数保留，调用点不必再翻一遍）。
pub fn probe_bodies(_variant: &str) -> Vec<Value> {
    vec![json!({})]
}

/// 一轮签到的**设备号**：16 位随机数字串。
///
/// 三个约束都来自参考实现的实测：登录那套 hex32 设备号或 `machineId` 去打签到
/// **必败 9074**；空值 → 9004；**复用**上一轮的号 → 风控画像里的可疑项。
/// 所以每轮新生成，并在 status → claim → 回查这**三发之间保持不变**
/// （回查要按同一个号问「今天这个设备签过没有」，换号回查等于自证失败）。
pub fn checkin_device_id() -> String {
    let mut digits = String::with_capacity(16);
    let mut buffer = [0u8; 32];
    // 逐字节取模会偏向 0-5（256 不是 10 的整数倍），因此丢掉 250-255 这六个值
    // —— 设备号的空间是 10^16，分布歪了就是可被统计出来的画像特征。
    while digits.len() < 16 {
        if getrandom::getrandom(&mut buffer).is_err() {
            // 系统随机源不可用：退回时间戳数字（仍是一个一次性号）。
            // 宁可用一个熵低的号签一次，也不能复用旧号 —— 后者才是可疑项。
            let stamp = crate::server::logging::now_ms().to_string();
            return stamp
                .chars()
                .filter(|ch| ch.is_ascii_digit())
                .take(16)
                .collect();
        }
        for byte in buffer {
            if byte < 250 {
                digits.push(char::from(b'0' + byte % 10));
                if digits.len() == 16 {
                    break;
                }
            }
        }
    }
    digits
}

/// `checkin_credits/status` 响应里本模块要用的字段。
///
/// `credits` / `extra_credits` 是**本次奖励数额**（不是钱包余额、签到后不增长），
/// 界面与日志都按"今日奖励"来写，别把它当余额显示。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub checked_in: bool,
    pub did_checked_in: bool,
    pub enable: bool,
    pub credits: i64,
    pub extra_credits: i64,
    pub code: i64,
    pub message: String,
}

impl Status {
    /// 「今天这个账号已经算签过了」——两个维度任一为真即真。
    ///
    /// `did_checked_in` 是**设备维度**（这个号今天签过），`checked_in` 是账号维度。
    pub fn done_today(&self) -> bool {
        self.checked_in || self.did_checked_in
    }

    /// 今日奖励的两个分量（合计见 [`Award::total`]）。
    pub fn award(&self) -> Award {
        Award { credits: self.credits, extra_credits: self.extra_credits }
    }
}

/// 响应体 → `Status`。缺字段一律按 `false` / `0` 收（**不猜**：`enable` 缺省
/// 按"没开"处理最保守，宁可少签一次也不多打一发请求）。
pub fn status_of(payload: &Value) -> Status {
    let number = |key: &str| payload.get(key).and_then(Value::as_i64).unwrap_or(0);
    Status {
        checked_in: flag(payload, "checked_in"),
        did_checked_in: flag(payload, "did_checked_in"),
        enable: flag(payload, "enable"),
        credits: number("credits"),
        extra_credits: number("extra_credits"),
        code: payload.get("code").and_then(Value::as_i64).unwrap_or(CODE_OK),
        message: payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
    }
}

fn flag(payload: &Value, key: &str) -> bool {
    payload.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// 一次往返后的业务码判定。
///
/// 参考实现分三路：码 0 → 收；码 9074 → 换下一个探测体（同方案）；其它非零
/// → 换下一个鉴权方案。比它多一档 `GiveUp`（1001）：既然这一码判为「服务端
/// 已作废这张 JWT」，换 `Bearer` 重试还是拿同一张死令牌去打 —— 那一发不会
/// 改变结果，只会把一次点击变成两发配额。换发在驱动那一层做。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// 采纳这一发
    Accept,
    /// 9074：换下一个探测体（体用尽则整体失败）
    NextBody,
    /// 其它非零码：换下一个鉴权方案（方案用尽则整体失败）
    NextScheme,
    /// 1001：这一发就是终点 —— 真正的处置在驱动那一层（换发新令牌后重放整轮）
    GiveUp,
}

pub fn probe_for(code: i64) -> Probe {
    match code {
        CODE_OK => Probe::Accept,
        CODE_REJECTED => Probe::NextBody,
        CODE_TOKEN_DEAD => Probe::GiveUp,
        _ => Probe::NextScheme,
    }
}

/// 状态 → 领取 → 回查 的**结论**（与网络层解耦）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 上游没给这个账号开签到活动（中性，不算失败也不算成功）
    NotEnabled,
    /// 今天已经签过了（`already` 带的是当日奖励数额，可能为 0）
    AlreadyCheckedIn(Award),
    /// 本次真领到了（回查已确认），参数为奖励数额
    Claimed(Award),
    /// 领取被拒（`code` 是上游业务码，`message` 是上游原话）
    Rejected(i64, String),
    /// claim 回了 code:0 但同设备号回查没翻转 —— 服务端静默拒签，**不算成功**
    Unconfirmed,
}

/// 今日奖励的**两部分**（基础 + 加成）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Award {
    pub credits: i64,
    pub extra_credits: i64,
}

impl Award {
    /// 参考实现给界面看的合计数（两者相加）。
    pub fn total(&self) -> i64 {
        self.credits + self.extra_credits
    }
}

/// 拿到状态与（必要时）领取+回查后的判定。
///
/// `before` 是本轮第一发状态；`after` 是领取后用**同一个设备号**的回查；
/// `claim` 是领取那一发的响应（只读它的 code 与 message，数额归回查读）。
pub fn decide(before: &Status, claim: &Status, after: Option<&Status>) -> Outcome {
    if !before.enable {
        return Outcome::NotEnabled;
    }
    if before.done_today() {
        return Outcome::AlreadyCheckedIn(before.award());
    }
    if claim.code != CODE_OK {
        return Outcome::Rejected(claim.code, claim.message.clone());
    }
    match after {
        // 回查确认：奖励数按回查那份读（上游在 claim 里不保证给数额）
        Some(confirmed) if confirmed.done_today() => Outcome::Claimed(confirmed.award()),
        Some(_) | None => Outcome::Unconfirmed,
    }
}

/// 这一行结果是不是「服务端把这张 JWT 作废了」。
///
/// 判据两条并列（码、文案），因为 1001 有时**不带文案**：只认码会在文案到位时
/// 仍走一遍，只认文案会在文案缺失时整条失效。文案两种写法都要认 —— 上游与
/// 参考实现各自转述过 `not able to authenticate` 和 `unable to authenticate`，
/// 差一个词就漏判。
pub fn needs_reauth(row: &Value) -> bool {
    if row.get("code").and_then(Value::as_i64) == Some(CODE_TOKEN_DEAD) {
        return true;
    }
    let message = row.get("msg").and_then(Value::as_str).unwrap_or_default();
    message.contains("not able to authenticate") || message.contains("unable to authenticate")
}

/// Trae 账号的每日签到。
///
/// 与 `query_usage` 同一条凭证链：读记录 → 组凭据 → 解代理 → **临期先续期**
/// （参考实现的"签到前保鲜"：token 过期时签到必撞会话类错误，而那一发错误
/// 看起来像"账号不行了"，会把健康账号拖进冷却）。
///
/// 之上多一条 **1001 的换发重放**：`renew_if_due` 是按本地 `expiredAt` 判临期的，
/// 而 `CODE_TOKEN_DEAD` 的意义恰恰是"本地那张表看不出来"。所以这一码单独走
/// `renew_forced`（无视临期直接换发），再打一轮，总共两轮封顶 —— 一次点击不该
/// 变成六发以上。换发也失败就叫用户重新登录，两种结果都比一句报错有用。
pub async fn claim_daily_checkin(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let record = read_record(store, account_id)?;
    let credential = Credential::from_payload(&record).map_err(GatewayError::new)?;
    let proxy = account_proxy(&record)?;
    let credential = renew_if_due(store, &record, &credential, proxy.as_ref()).await?;
    let first = run(&credential, proxy.as_ref(), UG_HOST).await?;
    if !needs_reauth(&first) {
        return Ok(first);
    }
    let renewed = match renew_forced(store, &record, &credential, proxy.as_ref()).await {
        Ok(renewed) => renewed,
        Err(error) => {
            // 连 refreshToken 都换不出新令牌：这才是真的"要重新登录"。
            // 原行留着（码、文案都在），只在后面接一句我们做了什么的实话说。
            // ⚠️ 追加的句子不许出现「已签到」「已领取」，理由见 `complete`。
            let mut row = first;
            if let Some(map) = row.as_object_mut() {
                map.insert("reauthAttempted".to_string(), json!(true));
                map.insert("reauthFailed".to_string(), json!(true));
                let head = map.get("msg").and_then(Value::as_str).unwrap_or_default().to_string();
                map.insert(
                    "msg".to_string(),
                    json!(format!("{head}；换发新令牌也失败（{}），请在「账号」页重新登录", error.message)),
                );
            }
            return Ok(row);
        }
    };
    let mut row = run(&renewed, proxy.as_ref(), UG_HOST).await?;
    if let Some(map) = row.as_object_mut() {
        map.insert("reauthAttempted".to_string(), json!(true));
    }
    Ok(row)
}

/// 一轮签到（凭证 → 结果行）。`base` 让测试夹具能把这三发指向假上游；
/// 生产上它是 `UG_HOST` 常量，而**这条链最容易写错的就是 host**（打错 host 的
/// 失败形态是 401「unable to authenticate」而不是 404，看不出是 host 问题）。
async fn run(
    credential: &Credential,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
    base: &str,
) -> Result<Value, GatewayError> {
    let variant = credential.variant().to_string();
    if variant == "intl" || variant == "solo-intl" {
        // 参考实现里国际版根本没有签到（上游无此活动）。走到这里只可能是
        // 手工粘进来的谱系，如实说明而不是打一发注定失败的请求。
        return Ok(json!({
            "success": false,
            "msg": "国际版 Trae 没有签到通道（上游无此活动）",
        }));
    }
    if credential.access_token.trim().is_empty() {
        return Err(GatewayError::with_status(401, "该 Trae 账号没有可用凭证，无法签到"));
    }
    if credential.device_id.trim().is_empty() {
        // 参考实现对缺 deviceId 的凭据是**硬拦**：这类凭据多半是手工粘贴的，
        // 服务端侧没有设备绑定，签到正好落在「绑定不一致 = 9074 高危画像」。
        // 与其打一发去撞风控，不如把话说明白：重新走一次网页登录再签。
        return Ok(json!({
            "success": false,
            "msg": "该账号没有 deviceId（多半是手工粘贴的凭据），官方按设备画像风控，请先重新登录再签到",
        }));
    }

    // 一轮一个号，三发之间不变（见 `checkin_device_id` 的说明）
    let device_id = checkin_device_id();
    let before = status(base, credential, &variant, &device_id, proxy).await?;
    // 状态那一发自己回**非零业务码**时必须原样报成拒绝，不许退化成"活动没开"。
    // 退化是真的会发生的形状：`status_of` 对缺字段一律按 `enable=false` 收，
    // 于是 1001（这张 JWT 已被服务端作废）会被翻成「当前没有可领取的签到活动」
    // —— 把一个鉴权失败报成一件中性事，用户与定时链都会据此放弃这个账号。
    if before.code != CODE_OK {
        return Ok(complete(
            Outcome::Rejected(before.code, before.message.clone()),
            &device_id,
        ));
    }
    if !before.enable || before.done_today() {
        // 这一条分支根本没打领取；`decide` 在读 claim 之前就返回了，给一个空响应占位。
        return Ok(complete(decide(&before, &Status::default(), None), &device_id));
    }
    let claim = claim(base, credential, &variant, &device_id, proxy).await?;
    if claim.code != CODE_OK {
        // 拒绝行要把第一发的状态读数带上（`with_status_probe`）—— 未知码的含义
        // 只能从「状态说什么 × 领取拒不拒」这个组合里夹出来。
        return Ok(with_status_probe(
            complete(decide(&before, &claim, None), &device_id),
            &before,
        ));
    }
    // 同设备号回查。回查本身失败（网络/会话）时按"未确认"处理 ——
    // 一发没被确认的 claim 报成成功，就是给用户一个假成功。
    let after = status(base, credential, &variant, &device_id, proxy).await.ok();
    Ok(complete(decide(&before, &claim, after.as_ref()), &device_id))
}

/// 判定 → 界面与日志消费的 `{success, msg, ...}`。
///
/// 设备号一并带回（`checkinDeviceId`）：这一轮用的哪个号，是把「上游回了 9074」
/// 与「官方客户端同日替用户签了」在后端日志里分开的唯一线索。
///
/// ⚠️ 两个雷区：
///   1. 数字口径：`credits`/`extra_credits` 是上游**报出的当日奖励数额**，不是
///      到账余额 —— 文案说"官方报出的当日奖励"，把"实际到账"留给界面上已有的
///      余额列，不用一句会撒谎的成功文案替它背书。
///   2. Rejected / Unconfirmed 的文案不许出现「已签到」「已领取」：批量层
///      `billing::checkin::checkin_completed_today` 是按 msg 子串判"当日已办"的，
///      写进去就把一次失败洗成已签、当日不再重试。（上游自己给的文案落在
///      `{reason}` 里，那一格若真说"已领"，被批量层当成当日已办是对上游原话
///      的忠实执行，不是我们的加工。）
fn complete(outcome: Outcome, device_id: &str) -> Value {
    let mut row = serde_json::Map::new();
    let awarded = match outcome {
        Outcome::NotEnabled => {
            row.insert("success".into(), json!(false));
            row.insert(
                "msg".into(),
                json!("当前没有可领取的签到活动（上游未对该账号开放这一档）"),
            );
            None
        }
        Outcome::AlreadyCheckedIn(award) => {
            row.insert("success".into(), json!(false));
            // `alreadyCompleted` 是给 `billing::checkin::checkin_completed_today`
            // 看的：今天已经签过 = 当日台账要落库，定时链明天才照常再试，
            // 而不是把"已签"当成失败反复重打。
            row.insert("alreadyCompleted".into(), json!(true));
            row.insert(
                "msg".into(),
                json!(if award.total() > 0 {
                    format!("今日已签到（官方报出当日奖励 {} 积分）", award.total())
                } else {
                    "今日已签到".to_string()
                }),
            );
            Some(award)
        }
        Outcome::Claimed(award) => {
            row.insert("success".into(), json!(true));
            row.insert(
                "msg".into(),
                json!(if award.total() > 0 {
                    format!(
                        "签到成功（官方报出当日奖励 {} 积分；到账看余额列）",
                        award.total()
                    )
                } else {
                    // 回查确认了"今天签过"，但上游没给数额 —— 不编一个数字出来
                    "签到成功（上游未给出奖励数额）".to_string()
                }),
            );
            Some(award)
        }
        Outcome::Rejected(code, message) => {
            row.insert("success".into(), json!(false));
            row.insert("code".into(), json!(code));
            // 上游给了文案就原样带出，没给才说"未给"。
            let reason = if message.is_empty() {
                "上游未给文案"
            } else {
                message.as_str()
            };
            let text = if code == CODE_REJECTED {
                // 两种读法都写出来，因为一手记录只到"偏向"的程度。
                format!(
                    "官方拒绝本次签到（{code}）：{reason}；这一码有两说 —— 参考实现判产品谱系/设备画像错配，\
                     同族端点的外部实测判「按 device id 限流、换张派生号就领得到」。本轮只发了一发，\
                     当日不再自动重试，想再试就再点一次签到"
                )
            } else if code == CODE_TOKEN_DEAD {
                format!(
                    "官方拒绝本次签到（{code}）：{reason}；这一码判为「服务端已作废这张登录态」\
                     （本地到期时间看不出来），处置是换发新令牌后重放一轮"
                )
            } else {
                // 不猜含义 —— 跨家搬码表正是把别家结论当自家事实的那类错。
                format!(
                    "官方拒绝本次签到（{code}）：{reason}；这一码本家没有实测依据，只按失败上报，当日台账不落"
                )
            };
            row.insert("msg".into(), json!(text));
            None
        }
        Outcome::Unconfirmed => {
            row.insert("success".into(), json!(false));
            row.insert("claimUnconfirmed".into(), json!(true));
            row.insert(
                "msg".into(),
                json!("签到返回成功但同设备号回查未确认（服务端静默拒签），当日不再自动重试"),
            );
            None
        }
    };
    if let Some(award) = awarded {
        row.insert("awarded".into(), json!(award.total()));
        row.insert("checkinCredits".into(), json!(award.credits));
        row.insert("checkinExtraCredits".into(), json!(award.extra_credits));
    }
    row.insert("checkinDeviceId".into(), json!(device_id));
    Value::Object(row)
}

/// 把「本轮第一发状态读到了什么」原样挂到**拒绝行**上。
///
/// 为什么这一格值钱：未知业务码自己不含信息，能把它夹出来的是状态与领取的
/// **组合形状**。「活动开着但对这个账号今日不发货」和「有份额但被资格/风控拒了」
/// 在只带 code 的日志里长得一模一样；带上状态五格就能一眼分开。
/// 只挂在拒绝行：成功行的数额走 `awarded`（回查那份），挂两份会供出两个不同的数。
fn with_status_probe(row: Value, status: &Status) -> Value {
    let mut row = row;
    if let Some(map) = row.as_object_mut() {
        map.insert("statusEnable".into(), json!(status.enable));
        map.insert("statusCheckedIn".into(), json!(status.checked_in));
        map.insert("statusDidCheckedIn".into(), json!(status.did_checked_in));
        map.insert("statusCredits".into(), json!(status.credits));
        map.insert("statusExtraCredits".into(), json!(status.extra_credits));
    }
    row
}

/// 状态查询（一轮里会发两次：领取前与领取后回查）。
async fn status(
    base: &str,
    credential: &Credential,
    variant: &str,
    device_id: &str,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
) -> Result<Status, GatewayError> {
    let payload = round_trip(base, EP_CHECKIN_STATUS, variant, device_id, credential, proxy).await?;
    Ok(status_of(&payload))
}

/// 领取。回被采纳那份响应的 code 与 message（数额仍由回查读 —— 上游在 claim 里
/// 不保证给奖励数额，但它的**文案**是判别未知码的唯一线索，必须留下）。
async fn claim(
    base: &str,
    credential: &Credential,
    variant: &str,
    device_id: &str,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
) -> Result<Status, GatewayError> {
    let payload = round_trip(base, EP_CHECKIN_CLAIM, variant, device_id, credential, proxy).await?;
    Ok(status_of(&payload))
}

/// 一发「方案 × 探测体」的往返，返回被采纳的那份响应体。
///
/// 分派规则：`code:0` 采纳；`9074` 换下一个探测体（同方案）；其它非零码换下一个
/// 鉴权方案。全部用尽仍非零时**返回最后一份**响应，由调用方按业务码判定 ——
/// 签到失败不是网关错误，不该冒成 5xx。只有 HTTP 层的会话失效（401 一类）才
/// 往上抛 `GatewayError`，让批量签到把这一行报成失败并给出去重登的提示。
async fn round_trip(
    base: &str,
    endpoint: &str,
    variant: &str,
    device_id: &str,
    credential: &Credential,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
) -> Result<Value, GatewayError> {
    let url = format!("{base}{endpoint}");
    let bodies = probe_bodies(variant);
    let mut scheme = Scheme::Jwt;
    let mut last = json!({});
    loop {
        for (index, body) in bodies.iter().enumerate() {
            let reply =
                post_checkin(&url, body, variant, device_id, &credential.access_token, scheme, proxy)
                    .await?;
            let payload = reply.json().unwrap_or_else(|| json!({}));
            let code = payload.get("code").and_then(Value::as_i64).unwrap_or(CODE_OK);
            last = payload;
            match probe_for(code) {
                Probe::Accept => return Ok(last),
                Probe::GiveUp => return Ok(last),
                Probe::NextBody => {
                    // **体序列用尽就是这一发的终点**。这一格在只有一个体的今天
                    // 是主路径：9074 不是鉴权方案的错，换 Bearer 再打一遍只是
                    // 多花一次上游配额，还正好踩在"9074 之后再查 status 会加重
                    // 限流"那条外部实测上。
                    if index + 1 == bodies.len() {
                        return Ok(last);
                    }
                    continue;
                }
                Probe::NextScheme => break,
            }
        }
        let Some(next) = scheme.next() else {
            return Ok(last);
        };
        scheme = next;
    }
}

/// 真正发出去的那一发：ug 画像 + 指定鉴权方案 + 本轮固定设备号。
///
/// HTTP 层的状态码在这里就判成错误（与 `usage.rs` 的出站同一口径）：
/// ug 族带着 `Cloud-IDE-JWT` 在生产上是通的（余额读数实测），所以一个 401 的
/// 含义是"这条会话真的死了"，而不是"该换 Bearer 了"。业务码的分派
/// （9074 换体、其它非零换方案）在 `round_trip` 那一层。
async fn post_checkin(
    url: &str,
    body: &Value,
    variant: &str,
    device_id: &str,
    access_token: &str,
    scheme: Scheme,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
) -> Result<super::http::Reply, GatewayError> {
    let mut headers = ug_headers(variant, access_token, device_id);
    // `ug_headers` 写的是 Cloud-IDE-JWT；换 Bearer 兜底时**只换这一个头**，
    // 其余画像头保持逐字一致（混改会撞上"画像对不上"的风控）。
    headers.insert(
        "Authorization".to_string(),
        scheme.authorization(access_token),
    );
    let prepared: Vec<(String, String)> = headers.into_iter().collect();
    let pairs: Vec<(&str, String)> = prepared
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect();
    let reply = post_json(url, body, &pairs, REQUEST_TIMEOUT, proxy).await?;
    if reply.status >= 400 {
        let head: String = reply.body.chars().take(ERROR_BODY_HEAD).collect();
        let kind = classify(reply.status, &head);
        return Err(GatewayError::with_status(
            i32::from(super::forward::status_for(kind)),
            format!("Trae 签到接口返回 {}: {}", reply.status, head.trim()),
        ));
    }
    Ok(reply)
}
