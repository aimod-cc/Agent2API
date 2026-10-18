//! WorkBuddy **成长任务域**（国内版）：查询 → 接单(accept) → 点亮(事件上报) → 领奖(claim)。
//!
//! ── 这是什么（从参考项目 workbuddy2api 的 scripts/task_runner.py 移植）──
//! 上游把「用产品换积分」的活动做成一棵任务树：每个账号有一批成长任务
//! （建画布、用模板、用专家、用技能、建自动化、发对话……），完成任务送
//! Credit 与 Energy。任务本身是上游监测真实行为点亮的，但监测点是
//! **客户端上报的事件**（`/v2/report`），因此可以用与真实客户端同形状的
//! 事件把「成就态」补齐，再走真实领奖端点入账。
//!
//! ── 端点（三套域，各有各的头与指纹）─────────────────────────
//! ```text
//! chat 域  https://copilot.tencent.com   growth 任务列表 / accept / claim / 领养
//!           /activity/growth/tasks、/activity/growth/buddy/*、/v2/activity/growth/energy
//! billing 域 https://www.codebuddy.cn   常规上报（POST /v2/report）
//! web 域    https://www.workbuddy.cn    网页成长中心（Library_read 上报 + claim 降级）
//! ```
//! 小程序域（school 开学季）与猫猫旅行分别在同目录的 `school.rs` / `travel.rs`。
//!
//! ── 事实来源 ────────────────────────────────────────────────
//! 事件形状、端点与判据全部有实测沉淀（参考项目 M1-M15 里程碑），本文件按
//! 「形状对齐、不发明字段」的原则逐条移植：每个事件构造器上方都标注了它的
//! 来源判据。**不要「顺手统一」字段** —— 服务端按形状（域指纹 + 判据字段）
//! 归账，多一个键、少一个键都可能让那次上报不被计入。
//!
//! ── 安全取向（与参考项目一致）───────────────────────────────
//! · 全部动作都由**用户在界面显式触发**（或按用户配置的定时排程触发），不做
//!   无人值守的全量补刷；写动作之间有间隔（见 [`GROWTH_WRITE_GAP_MS`]）。
//! · 领奖按上游返回值如实报告：`already_claimed`（已领）与失败分开统计，
//!   已领不算失败（幂等语义，重复点不会重复入账）。
//! · 不可伪造任务（真实捐款等）不碰：见 [`spec_of`] 的 `unforgeable`。
//!
//! ── 文件分工（单文件行数约定）───────────────────────────────
//!   growth/spec.rs    任务映射表 + 对象 id 来源（scenes/专家/技能/案例/主题）
//!   growth/events.rs  事件构造器（桌面/web/MP 三套指纹与事件形状）
//!   growth/mod.rs     域客户端：列表/accept/上报/claim/领养（本文件）
//!   school.rs         开学季活动域（任务 + 幸运转盘）
//!   travel.rs         猫猫旅行状态机（buddy info / status / depart / claim）
//!   streak.rs         连登管家（补签 → 档位兑换 → 抽奖闭环）
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。

pub mod events;
pub mod school;
pub mod spec;
pub mod streak;
pub mod travel;

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::auth_http::send_raw;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::logging;

use spec::{ids_for, spec_of, ObjectId};

/// growth 域基址（chat 域；任务列表 / accept / claim / 领养都在这里）。
pub const CHAT_BASE: &str = "https://copilot.tencent.com";
/// billing 域基址（常规 `/v2/report` 上报）。
pub const BILL_BASE: &str = "https://www.codebuddy.cn";
/// web 域基址（网页成长中心：web 事件上报 + claim 降级）。
pub const WEB_BASE: &str = "https://www.workbuddy.cn";

/// 写动作之间的默认间隔（毫秒）。参考项目所有写动作硬性 ≥1s ——
/// 秒发易触发上游风控（实测沉淀），本仓沿用同一口径。
pub const GROWTH_WRITE_GAP_MS: u64 = 1200;

/// 单请求超时（毫秒）。上报/领取都是小请求，20 秒与计费域同口径。
const REQUEST_TIMEOUT_MS: u64 = 20_000;

/// 事件上报客户端 UA（桌面指纹通道，参考项目实测值）。
pub const DESKTOP_UA: &str = "WorkBuddy/5.5.6 WorkBuddy/5.5.6 CLI/2.137.1";
/// 常规 growth 请求的 CLI UA（参考项目 `task_common.CLIENT_UA` 的对等值）。
pub const CLI_UA: &str = "CLI/2.63.2 CodeBuddy/2.63.2";
/// web 域浏览器 UA（Library_read 与 web claim 用）。
pub const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36";
/// 小程序 UA（school 域用；微信注入形态）。
pub const MP_UA: &str = "Mozilla/5.0 (Linux; Android 14; MicroMessenger/8.0.49 WeChat/0.8.0 \
MiniProgramEnv/android; wkbrowser xweb)";

/// 账号上下文：uid / 昵称 / token / domain / 出口。
pub struct GrowthAccount {
    pub uid: String,
    pub nick: String,
    pub token: String,
    /// 登录态里的 domain（`auth.domain`，如 copilot.tencent.com）—— 参考实现
    /// 的 `X-Domain` 头就来自它（请求形态是实测沉淀的一部分）。
    pub domain: String,
    pub proxy: Option<ResolvedProxy>,
}

impl GrowthAccount {
    /// 从账号存储取会话派生（没有可用凭证返回 None）。
    pub fn from_store(store: &AccountStore, account_id: &str) -> Option<Self> {
        let entry = store.get_session_by_id(account_id)?;
        let session = &entry.session;
        let uid = session
            .get("account")
            .and_then(|account| account.get("uid"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let nick = session
            .get("account")
            .and_then(|account| account.get("nickname"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let token = session
            .get("auth")
            .and_then(|auth| auth.get("accessToken"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let domain = session
            .get("auth")
            .and_then(|auth| auth.get("domain"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if token.is_empty() {
            return None;
        }
        // 出口解析与 billing 同款：解析失败回退直连（记一条 warn 由调用方做）
        let proxy = ResolvedProxy::from_json(session.get("proxy").unwrap_or(&Value::Null))
            .ok()
            .flatten();
        Some(Self { uid, nick, token, domain, proxy })
    }

    /// 稳定派生一个 36 位 hex 设备标识（machineId / sessionId 用，幂等）。
    ///
    /// 由 uid + 盐派生而不是每次随机：模拟「同一台固定设备」的画像，
    /// 随机值会让上游看到设备不断变化。只用于事件指纹，不参与业务逻辑。
    pub fn derive_id(&self, salt: &str) -> String {
        use md5::{Digest, Md5};
        let mut hasher = Md5::new();
        hasher.update(format!("{salt}:{}", self.uid).as_bytes());
        let digest = hasher.finalize();
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

/// growth 域的一次 HTTP 调用结果：HTTP 状态 + 已解析 payload。
pub struct GrowthCall {
    pub status: u16,
    pub payload: Option<Value>,
}

impl GrowthCall {
    /// 业务信封的 code（缺省 None）。
    pub fn code(&self) -> Option<i64> {
        self.payload
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_i64)
    }

    /// 业务信封的 msg / message。
    pub fn msg(&self) -> String {
        self.payload
            .as_ref()
            .and_then(|value| {
                value
                    .get("msg")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("message").and_then(Value::as_str))
            })
            .unwrap_or("")
            .to_string()
    }

    /// 业务成功：HTTP 2xx 且 code 为 0 或缺省。
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status) && matches!(self.code(), Some(0) | None)
    }

    /// 信封的 data 段。
    pub fn data(&self) -> &Value {
        static NULL: Value = Value::Null;
        self.payload
            .as_ref()
            .and_then(|value| value.get("data"))
            .unwrap_or(&NULL)
    }
}

/// 发一次 growth 域请求。
///
/// `base` 是域基址，`path` 是路径（会原样拼接，不做归一 —— 调用方传全路径）。
/// `headers` 是**完整头集合**（各域的头差异大：桌面/web/MP 三套指纹各写一份，
/// 见 events.rs）；基础头（Authorization / Accept / Content-Type / Origin /
/// Referer / UA / X-Domain / X-User-Id）由本函数统一补上，调用方显式给的同名
/// 头会**覆盖**它们（如桌面通道换 X-Domain 与 UA、web 通道换 Origin/Referer）
/// —— 这套基础头形状照抄参考项目 `task_common._headers`（实测成功形状）。
pub async fn growth_request(
    account: &GrowthAccount,
    method: &str,
    url: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
) -> Result<GrowthCall, String> {
    let mut final_headers: Vec<(String, String)> = vec![
        ("Authorization".to_string(), format!("Bearer {}", account.token)),
        ("Accept".to_string(), "application/json".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), CLI_UA.to_string()),
        ("Origin".to_string(), "https://www.codebuddy.cn".to_string()),
        ("Referer".to_string(), "https://www.codebuddy.cn/".to_string()),
    ];
    if !account.uid.is_empty() {
        final_headers.push(("X-User-Id".to_string(), account.uid.clone()));
    }
    if !account.domain.is_empty() {
        final_headers.push(("X-Domain".to_string(), account.domain.clone()));
    }
    for (key, value) in headers {
        // 调用方显式给了同名头就覆盖（比如桌面/web 通道换 UA 与 X-Domain）
        if let Some(existing) = final_headers.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            existing.1 = value.clone();
        } else {
            final_headers.push((key.clone(), value.clone()));
        }
    }
    let response = send_raw(
        method,
        url,
        body,
        &final_headers,
        account.proxy.as_ref(),
        Some(REQUEST_TIMEOUT_MS),
    )
    .await
    .map_err(|error| {
        if error.is_timeout() {
            "成长任务接口请求超时".to_string()
        } else {
            format!("成长任务接口请求失败: {error}")
        }
    })?;
    Ok(GrowthCall { status: response.status, payload: response.payload })
}

/// 便捷封装：chat 域（copilot.tencent.com）请求。
pub async fn chat_call(
    account: &GrowthAccount,
    method: &str,
    path: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
) -> Result<GrowthCall, String> {
    growth_request(account, method, &format!("{CHAT_BASE}{path}"), body, headers).await
}

/// 便捷封装：billing 域（www.codebuddy.cn）请求。
pub async fn bill_call(
    account: &GrowthAccount,
    method: &str,
    path: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
) -> Result<GrowthCall, String> {
    growth_request(account, method, &format!("{BILL_BASE}{path}"), body, headers).await
}

/// 便捷封装：web 域（www.workbuddy.cn）请求。
pub async fn web_call(
    account: &GrowthAccount,
    method: &str,
    path: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
) -> Result<GrowthCall, String> {
    growth_request(account, method, &format!("{WEB_BASE}{path}"), body, headers).await
}

// ─── growth 任务列表 / accept / claim ────────────────────────

/// growth 任务列表（GET /v2/activity/growth/tasks）。
///
/// `mp=true` 时带 `X-Client-Platform: miniprogram` —— 小程序限定任务
/// （Sequential_Tasks_*）只在该口径下发，且 accept/claim 也必须带同一头
/// （缺头 accept 返回 task not found，实测）。
pub async fn list_tasks(account: &GrowthAccount, mp: bool) -> Result<Vec<Value>, String> {
    let headers = events::mp_platform_header(mp);
    let call = chat_call(account, "GET", "/v2/activity/growth/tasks", None, &headers).await?;
    if !call.ok() {
        return Err(format!(
            "任务列表拉取失败（HTTP {} code={}）",
            call.status,
            call.code().map(|c| c.to_string()).unwrap_or_else(|| "-".to_string())
        ));
    }
    Ok(call
        .data()
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// 在任务列表里找单个任务。
pub fn find_task(tasks: &[Value], code: &str) -> Option<Value> {
    tasks
        .iter()
        .find(|task| task.get("task_code").and_then(Value::as_str) == Some(code))
        .cloned()
}

/// accept 任务（POST /v2/activity/growth/tasks/accept，body `{task_codes:[code]}`）。
///
/// 返回 (ok, status_text)：`ok` 表示响应里 results[0].status == "accepted"
/// 或回读已非 not_accepted；`status_text` 给界面/日志用。
/// 参考项目实测：存在 HTTP 200 + msg=OK 但 status 非 accepted 的形态，
/// 因此调用方（`light_up`）还会回读 accept_status 确认生效。
pub async fn accept_task(
    account: &GrowthAccount,
    code: &str,
    mp: bool,
) -> Result<(bool, String), String> {
    let headers = events::mp_platform_header(mp);
    let call = chat_call(
        account,
        "POST",
        "/v2/activity/growth/tasks/accept",
        Some(&json!({ "task_codes": [code] })),
        &headers,
    )
    .await?;
    let status = call
        .data()
        .get("results")
        .and_then(Value::as_array)
        .and_then(|results| results.first())
        .and_then(|result| result.get("status"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| call.msg());
    let ok = call.status == 200 && status == "accepted";
    Ok((ok, status))
}

/// 领奖（POST {chat}/activity/growth/tasks/{code}/claim，无 body）。
///
/// ── 幂等与降级（M15 实测口径，逐条对齐参考项目）──────────────
/// · 已领任务返回 `already_claimed` + credit/energy 读数 —— 不算失败；
/// · chat 域 400 时降级 web 域（`{web}/activity/growth/tasks/{code}/claim`，
///   带 Origin/Referer/x-client-platform: web 头）—— 部分任务 chat 域 400；
/// · `mp=true` 时 chat 域先带 `X-Client-Platform: miniprogram`（小程序任务
///   领奖必需）。
///
/// 返回：`1` = 新入账（credit/energy 已给）；`2` = 已领（幂等）；
/// `0` = 失败。第二个返回值是这一笔的收益读数与文案（给记录行）。
pub async fn claim_task(
    account: &GrowthAccount,
    code: &str,
    mp: bool,
) -> (i32, i64, i64, String) {
    let path = format!("/activity/growth/tasks/{code}/claim");
    let headers = events::mp_platform_header(mp);
    let call = chat_call(account, "POST", &path, None, &headers).await;
    let call = match call {
        Ok(call) => call,
        Err(error) => return (0, 0, 0, error),
    };
    if call.status == 400 {
        // chat 域 400 → 降级 web 域（fork ClaimReward 同款头）
        logging::verbose(
            "[Growth]",
            &format!("claim {code} chat 域 400（{}），降级 web 域", call.msg()),
        );
        let web_headers = events::web_claim_headers(account);
        let web = web_call(account, "POST", &path, None, &web_headers).await;
        return match web {
            Ok(web) => parse_claim(account, code, &web),
            Err(error) => (0, 0, 0, error),
        };
    }
    parse_claim(account, code, &call)
}

/// claim 响应解析（chat / web 两域共用）。
fn parse_claim(account: &GrowthAccount, code: &str, call: &GrowthCall) -> (i32, i64, i64, String) {
    let uid8: String = account.uid.chars().take(8).collect();
    if !call.ok() {
        let message = format!(
            "claim HTTP {} code={} {}",
            call.status,
            call.code().map(|c| c.to_string()).unwrap_or_else(|| "-".to_string()),
            call.msg()
        );
        logging::log("[Growth]", &format!("{uid8} {code}: {message}"));
        return (0, 0, 0, message);
    }
    let credit = call.data().get("credit").and_then(Value::as_i64).unwrap_or(0);
    let energy = call.data().get("energy").and_then(Value::as_i64).unwrap_or(0);
    if call
        .data()
        .get("already_claimed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let message = format!("已领取（credit+{credit} energy+{energy}）");
        logging::verbose("[Growth]", &format!("{uid8} {code}: claim already_claimed {message}"));
        return (2, credit, energy, message);
    }
    let message = format!("领取成功（credit+{credit} energy+{energy}）");
    logging::log("[Growth]", &format!("{uid8} {code}: claim {message}"));
    (1, credit, energy, message)
}

/// 能量余额（只读 oracle，GET /v2/activity/growth/energy → data.balance）。
pub async fn energy_balance(account: &GrowthAccount) -> Option<i64> {
    let call = chat_call(account, "GET", "/v2/activity/growth/energy", None, &[]).await.ok()?;
    if !call.ok() {
        return None;
    }
    call.data().get("balance").and_then(Value::as_i64)
}

/// 连登天数（只读 oracle，GET /activity/growth/streak → data.streak.days）。
pub async fn growth_streak(account: &GrowthAccount) -> Option<i64> {
    let call = chat_call(account, "GET", "/activity/growth/streak", None, &[]).await.ok()?;
    if !call.ok() {
        return None;
    }
    call.data()
        .get("streak")
        .and_then(|streak| streak.get("days"))
        .and_then(Value::as_i64)
}

// ─── 领养第一只 Buddy ────────────────────────────────────────

/// 领养第一只 Buddy：report（前置解锁）→ agreement → buddy/first。
///
/// `buddy/first` 直接发 +300c+8e，任务随之完成（后续 claim 为 already_claimed）。
/// 门槛未达（当日活跃对话量不够）时上游返回 HTTP 400
/// "first_buddy task not completed yet" —— 属**预期**正常态，不算失败。
pub async fn adopt_buddy(account: &GrowthAccount) -> (bool, String) {
    // 1) 前置解锁：一次 chat_request_send 上报
    let report = events::report_chat_send(account, false).await;
    let _ = report;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    // 2) 同意协议（幂等）
    let _ = chat_call(
        account,
        "POST",
        "/activity/growth/buddy/agreement",
        Some(&json!({ "agree": true })),
        &[],
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    // 3) 领养
    let call = chat_call(account, "POST", "/activity/growth/buddy/first", Some(&json!({})), &[]).await;
    match call {
        Ok(call) => {
            if call.ok() {
                let credit = call.data().get("credit").and_then(Value::as_i64).unwrap_or(0);
                let energy = call.data().get("energy").and_then(Value::as_i64).unwrap_or(0);
                (true, format!("领养成功（credit+{credit} energy+{energy}）"))
            } else if call.msg().to_lowercase().contains("first_buddy task not completed") {
                // 门槛未达：预期正常态（当日活跃对话量不够），如实标注
                (false, "领养门槛未达（需先完成当日对话量），明天再试".to_string())
            } else {
                (false, format!("领养失败（HTTP {} {}）", call.status, call.msg()))
            }
        }
        Err(error) => (false, error),
    }
}

// ─── 点亮：单任务一体流程 ────────────────────────────────────

/// 一次任务处理的结论（写进任务记录的 outcome）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskOutcome {
    /// 本轮新入账 / 新点亮
    Done,
    /// 已领 / 已完成（幂等）
    Already,
    /// 有意跳过（不可伪造 / 未映射 / 时段未到 / 无可用对象）
    Skipped,
    /// 上游报错
    Failed,
}

impl TaskOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Already => "already",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

/// 单任务处理结果行（`process_task` 的返回，界面按行渲染）。
pub struct TaskReport {
    pub code: String,
    pub outcome: TaskOutcome,
    pub message: String,
    /// 本次入账读数（Done 时可能非零）
    pub credit: i64,
    pub energy: i64,
}

/// 单任务一体流程：accept（带登记验证）→ 按 target 补齐上报 → 回读 → claim。
///
/// ── 为什么 accept 要「回读确认」────────────────────────────
/// 参考项目实测：上游存在 HTTP 200 + msg=OK 但 accept 未真正登记的形态，
/// 此时后续上报全部不归账、任务永远点不亮。所以 accept 之后必须回读
/// `accept_status` 确认已非 not_accepted 才继续；一次重试后仍不生效就放弃
/// 本轮（不硬刷）。
///
/// ── 上报的域与指纹按任务 kind 分派（见 events.rs）────────────
/// 常规任务走 billing 域（`{bill}/v2/report`）；桌面指纹事件组走 chat 域；
/// web 域事件走 `{web}/v2/report`；MP 事件组走 `{bill}/v2/report` 的
/// 小程序指纹形状。
pub async fn process_task(account: &GrowthAccount, code: &str, task: &Value) -> TaskReport {
    let spec = spec_of(code);
    let uid8: String = account.uid.chars().take(8).collect();
    let accept_status = task
        .get("accept_status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let progress = task.get("progress").cloned().unwrap_or(Value::Null);
    let cur = progress.get("current").and_then(Value::as_i64).unwrap_or(0);
    let target = progress
        .get("target")
        .and_then(Value::as_i64)
        .or_else(|| spec.as_ref().map(|spec| spec.target))
        .unwrap_or(1);

    let Some(spec) = spec else {
        return TaskReport {
            code: code.to_string(),
            outcome: TaskOutcome::Skipped,
            message: "非映射任务（未经实测沉淀），保守跳过".to_string(),
            credit: 0,
            energy: 0,
        };
    };
    if spec.unforgeable {
        return TaskReport {
            code: code.to_string(),
            outcome: TaskOutcome::Skipped,
            message: format!("不可伪造（{}），跳过", spec.reason),
            credit: 0,
            energy: 0,
        };
    }

    // 已领：跳过（幂等；界面上的 redo 也走这里，先查再动）
    if accept_status == "claimed" {
        return TaskReport {
            code: code.to_string(),
            outcome: TaskOutcome::Already,
            message: format!("已领取（{cur}/{target}）"),
            credit: 0,
            energy: 0,
        };
    }
    // 未到上线时间（服务端 locked + unlock_at，如 Sequential_Tasks_5 的
    // 2026-09-25）：accept 会被服务端拒绝（"task locked until …"），
    // 上报/claim 均无意义。跳过并如实标注解锁时间（到点后自然可执行）。
    if task.get("locked").and_then(Value::as_bool).unwrap_or(false) {
        let unlock_at = task
            .get("unlock_at")
            .and_then(Value::as_str)
            .unwrap_or("未知");
        return TaskReport {
            code: code.to_string(),
            outcome: TaskOutcome::Skipped,
            message: format!("未到上线时间（解锁 {unlock_at}）"),
            credit: 0,
            energy: 0,
        };
    }
    // 已完成未领：直接 claim
    if cur >= target || accept_status == "completed" {
        let (result, credit, energy, message) = claim_task(account, code, spec.mp).await;
        let outcome = match result {
            1 => TaskOutcome::Done,
            2 => TaskOutcome::Already,
            _ => TaskOutcome::Failed,
        };
        return TaskReport { code: code.to_string(), outcome, message, credit, energy };
    }

    // 夜猫任务：非窗口期（23:00-08:00 CST）不动
    if code == "black_cat" && !events::within_night_window() {
        return TaskReport {
            code: code.to_string(),
            outcome: TaskOutcome::Skipped,
            message: format!("非夜猫窗口（23:00-08:00），当前 {cur}/{target}"),
            credit: 0,
            energy: 0,
        };
    }

    // 需要点亮：先 accept（带登记验证）
    if accept_status == "not_accepted" || accept_status.is_empty() {
        if !accept_with_verify(account, code, spec.mp).await {
            return TaskReport {
                code: code.to_string(),
                outcome: TaskOutcome::Failed,
                message: "accept 未登记生效，本轮跳过待下次".to_string(),
                credit: 0,
                energy: 0,
            };
        }
    }

    // 需要上报的次数（黑猫窗口内最多补 1 次）
    let mut need = (target - cur).max(0);
    if code == "black_cat" {
        need = need.min(1);
    }

    // 对象 id 来源（scenes / 专家 / 技能 / 案例 / 主题；无对象类返回空占位）
    let objects: Vec<ObjectId> = if need > 0 {
        match ids_for(&spec.kind, account, need as usize, cur.max(0) as usize).await {
            Ok(list) => list,
            Err(error) => {
                return TaskReport {
                    code: code.to_string(),
                    outcome: TaskOutcome::Skipped,
                    message: format!("对象 id 获取失败：{error}"),
                    credit: 0,
                    energy: 0,
                };
            }
        }
    } else {
        Vec::new()
    };
    if need > 0 && objects.is_empty() && spec.needs_object {
        return TaskReport {
            code: code.to_string(),
            outcome: TaskOutcome::Skipped,
            message: format!("{} 无可用对象 id，跳过点亮", spec.kind),
            credit: 0,
            energy: 0,
        };
    }

    // 按 kind 分派上报（各 kind 的域/指纹/事件形状在 events.rs）
    let report_error = events::light_up(account, code, &spec.kind, &objects, need as usize).await;
    if let Some(error) = report_error {
        logging::log("[Growth]", &format!("{uid8} {code}: 上报失败 {error}"));
        return TaskReport {
            code: code.to_string(),
            outcome: TaskOutcome::Failed,
            message: format!("上报失败：{error}"),
            credit: 0,
            energy: 0,
        };
    }

    // 服务端归账留时，然后回读
    tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
    let after = match list_tasks(account, spec.mp).await {
        Ok(tasks) => find_task(&tasks, code),
        Err(_) => None,
    };
    let (cur2, target2, ast2) = after
        .as_ref()
        .map(|task| {
            let progress = task.get("progress").cloned().unwrap_or(Value::Null);
            (
                progress.get("current").and_then(Value::as_i64).unwrap_or(0),
                progress.get("target").and_then(Value::as_i64).unwrap_or(target),
                task.get("accept_status").and_then(Value::as_str).unwrap_or("").to_string(),
            )
        })
        .unwrap_or((0, target, String::new()));

    if ast2 == "claimed" {
        return TaskReport {
            code: code.to_string(),
            outcome: TaskOutcome::Already,
            message: format!("本轮已入账（{cur2}/{target2}）"),
            credit: 0,
            energy: 0,
        };
    }
    if ast2 == "completed" || cur2 >= target2 {
        let (result, credit, energy, message) = claim_task(account, code, spec.mp).await;
        let outcome = match result {
            1 => TaskOutcome::Done,
            2 => TaskOutcome::Already,
            _ => TaskOutcome::Failed,
        };
        return TaskReport { code: code.to_string(), outcome, message, credit, energy };
    }
    TaskReport {
        code: code.to_string(),
        outcome: TaskOutcome::Skipped,
        message: format!("上报未达 target（{cur2}/{target2}），待下次"),
        credit: 0,
        energy: 0,
    }
}

/// 领养任务（first_buddy）的一体流程：走领养链路，app 侧的直接收益已由
/// [`adopt_buddy`] 返回；之后任务状态变 completed，再走 claim。
pub async fn process_first_buddy(account: &GrowthAccount, task: &Value) -> TaskReport {
    let accept_status = task
        .get("accept_status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let progress = task.get("progress").cloned().unwrap_or(Value::Null);
    let cur = progress.get("current").and_then(Value::as_i64).unwrap_or(0);
    let target = progress.get("target").and_then(Value::as_i64).unwrap_or(1);
    if accept_status == "claimed" {
        return TaskReport {
            code: "first_buddy".to_string(),
            outcome: TaskOutcome::Already,
            message: "已领取".to_string(),
            credit: 0,
            energy: 0,
        };
    }
    if cur >= target || accept_status == "completed" {
        let (result, credit, energy, message) = claim_task(account, "first_buddy", false).await;
        let outcome = match result {
            1 => TaskOutcome::Done,
            2 => TaskOutcome::Already,
            _ => TaskOutcome::Failed,
        };
        return TaskReport { code: "first_buddy".to_string(), outcome, message, credit, energy };
    }
    if accept_status == "not_accepted" || accept_status.is_empty() {
        let _ = accept_with_verify(account, "first_buddy", false).await;
    }
    let (ok, message) = adopt_buddy(account).await;
    let outcome = if ok {
        TaskOutcome::Done
    } else if message.contains("门槛未达") {
        TaskOutcome::Skipped
    } else {
        TaskOutcome::Failed
    };
    // 领养成即任务完成：直接 try claim（已是既领状态则幂等返回 already）
    if ok {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let (result, credit2, energy2, claim_message) =
            claim_task(account, "first_buddy", false).await;
        if result == 1 {
            return TaskReport {
                code: "first_buddy".to_string(),
                outcome: TaskOutcome::Done,
                message: format!("{message}；{claim_message}"),
                credit: credit2,
                energy: energy2,
            };
        }
    }
    TaskReport {
        code: "first_buddy".to_string(),
        outcome,
        message,
        credit: 0,
        energy: 0,
    }
}

/// accept 并验证登记生效（回读 accept_status 确认；一次重试）。
async fn accept_with_verify(account: &GrowthAccount, code: &str, mp: bool) -> bool {
    for attempt in 1..=2 {
        let (ok, status) = match accept_task(account, code, mp).await {
            Ok(result) => result,
            Err(error) => {
                logging::verbose("[Growth]", &format!("accept {code} 失败（尝试 {attempt}）：{error}"));
                (false, error)
            }
        };
        // 回读确认（响应可能说 accepted 但服务端未落账）
        let after = list_tasks(account, mp)
            .await
            .ok()
            .and_then(|tasks| find_task(&tasks, code))
            .and_then(|task| {
                task.get("accept_status").and_then(Value::as_str).map(str::to_string)
            })
            .unwrap_or_default();
        let verified = ok && after != "not_accepted" && !after.is_empty();
        logging::verbose(
            "[Growth]",
            &format!("accept {code} 尝试{attempt} status={status} 回读={after} verified={verified}"),
        );
        if verified {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(GROWTH_WRITE_GAP_MS)).await;
    }
    false
}


/// 账号是不是成长任务适用的对象（国内版 WorkBuddy；global 不适用）。
///
/// 判据与参考项目 `auth_is_global` 同款：international 账号没有 CN 任务中心。
pub fn supports_growth(account: &Value) -> bool {
    let provider = account
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
    provider == crate::server::core::providers::workbuddy::Region::Cn.provider_id()
}
