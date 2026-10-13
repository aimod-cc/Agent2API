//! ZCode 的**激活事件上报**（`POST {zcode}/api/v1/event/report`）——
//! 领取资格的解锁动作。
//!
//! ── 为什么需要它（这是一条推断出来的因果，别当成可有可无的埋点）──
//! ZCode 的限时套餐领取（见 `claim.rs`）有个先决条件：**账号得先被上游认成
//! 「今天活跃过」**。官方客户端的做法是启动 / 日活时往事件接口打两条：
//! `app_launch` 与 `app_daily_active`；运营系统据此发放资格，`preview` 才会
//! 把套餐列出来。本家走的不是客户端，那条链不会自动发生 —— 于是症状是
//! 「明明有资格，探测却空 / 领取被判 1004 不符合资格」这种看不出原因的形状。
//! `zcode-switch` 的 `claim_refresh`（它的「刷新领取资格」）做的正是这件事：
//! 先上报这两条事件、再重新探测。本模块是它那一步的移植。
//!
//! ── 端点特征（照参考实现，别自己发明）───────────────────────
//!   · 路径 `{zcode_origin}/api/v1/event/report`，**两个地区同一个域**
//!     （zcode 平面两地相同，见 `region.rs`）；
//!   · **不带任何鉴权**：没有 `Authorization`、没有 `X-Device-Mid` ——
//!     身份在请求体的 `user_id` + `device_mid` 里（参考实现逐字如此）；
//!   · 信封 `{code, msg}`，`code == 0` 才算成功；
//!   · 每条事件一个请求（两条 = 两次往返），`event_id` 每条都新生成一个 UUID。
//!
//! ── 请求体的取值：能对上的对上，对不上的如实兜底 ──────────────
//! 字段清单逐条抄自参考实现（`activation_event_body`），其中三处是本机环境：
//!   · `client_timezone`：Windows 用 `tzutil /g` 读时区名再映射成 IANA
//!     （参考实现同法）；映射不到、或非 Windows（headless 容器）时给
//!     `"unknown"` —— 那也是参考实现在认不出时的取值；
//!   · `device_os_version`：Windows 读注册表的 `CurrentBuildNumber` 拼成
//!     `10.0.{build}`（参考实现同法）；其它平台给空串（同上，参考实现取不到
//!     时也是空串）；
//!   · `device_os_category`：编译期平台（windows / macos / linux）。
//! 其余是常量（`client_language` / `screen_resolution` / `event_region` …）：
//! 它们描述的是「官方客户端长什么样」，照抄参考实现的取值即可，本机没有
//! 对应的真实值可填 —— 编一个反而与客户端形态对不上。
//!
//! ── 失败为什么不阻断 ────────────────────────────────────────
//! 上报是**资格的前置动作**，不是领取本身：上游此刻拒绝（网络 / 风控 / 字段
//! 变更）只意味着这一次没补上，而领取仍有它自己的路径（用户手动点、验证码
//! 通过）。因此调用方（`api::zcode_claim`）拿到的是一句可读的原因，记日志、
//! 放进响应，但**不**把领取本身判失败。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic；探测本机环境要起
//! 子进程（`tzutil` / `reg`），因此走 `tokio::process` —— 不在异步线程上做
//! 阻塞调用（见 `probe_timezone` / `probe_os_version`）。

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::server::core::auth_http::send_raw;
use crate::server::core::proxies::ResolvedProxy;

use super::claim::app_version;
use super::credentials::new_uuid;
use super::region::Region;

/// 事件上报路径（参考实现 `EVENT_REPORT_URL` 的后半段）
const EVENT_REPORT_PATH: &str = "/api/v1/event/report";

/// 两条激活事件（顺序即上报顺序；参考实现逐字相同）。
///
/// `app_launch` = 「启动过客户端」，`app_daily_active` = 「今天活跃」——
/// 运营系统按后者发每日资格，两条都要发（参考实现也是两条都发）。
const ACTIVATION_EVENTS: [&str; 2] = ["app_launch", "app_daily_active"];

/// 单次请求超时（参考实现 `ACTIVATION_TIMEOUT_SECS = 10`）
const REQUEST_TIMEOUT_MS: u64 = 10_000;

/// 客户端语言（参考实现的 `ZCODE_LANG`）。
///
/// 它描述的是**官方客户端**的语言设置，不是本界面的语言 —— 本家有六种界面
/// 语言，而这一栏在上游那边只影响事件的文案归属，照抄客户端的默认值即可。
const CLIENT_LANGUAGE: &str = "zh-CN";

/// 上报时带的分辨率（参考实现的常量）。
///
/// 上游对事件只做统计，分辨率不是校验项；这是官方客户端在全屏下的常见值，
/// 照抄它比编一个（或空串）更接近客户端形态。
const SCREEN_RESOLUTION: &str = "2560x1440";

/// 上报一次激活（两个事件都发完才算成功；一条失败不阻止另一条，
/// 返回的是**第一个**失败原因）。
///
/// `user_id` 是账号记录里的 `userId`（OAuth 的 `user.user_id`）—— 空值在这里
/// **不是**错误而是「这份凭证没有身份」：手工粘贴 API Key 建的账号没有它，
/// 上游的事件没有 `user_id` 就归不到任何账号名下，发了也没有意义。因此调用方
/// 应在调用前判空（`api::zcode_claim` 就是这么做的），这里再兜一次。
///
/// 返回 `Err` 是一句给用户看的原因（网络失败 / 上游拒绝 / 缺身份）。
pub async fn report(
    region: Region,
    user_id: &str,
    device_mid: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<(), String> {
    let user_id = user_id.trim();
    if user_id.is_empty() {
        return Err(
            "该账号没有用户标识（手工粘贴 API Key 建的账号没有这一项），无法上报激活事件；\
             用「网页登录」添加的账号才会带"
                .to_string(),
        );
    }
    let url = format!("{}{EVENT_REPORT_PATH}", region.zcode_origin());
    let (timezone, os_version) = tokio::join!(probe_timezone(), probe_os_version());
    // ── 一条失败不阻止下一条（与参考实现的一处不同）──────────────
    // 参考实现在第一条事件失败时就返回。这里两条都发、把**第一个**失败原因带走：
    // 资格认的是 `app_daily_active`（第二条），为一条统计事件（`app_launch`）的
    // 失败放弃它不值 —— 而两条都失败时返回值仍然是失败，如实。
    let mut first_error: Option<String> = None;
    for event in ACTIVATION_EVENTS {
        // 每条事件一个**新** UUID：上游按 `event_id` 去重，重复用同一个会被判成
        // 重复事件（那正是「报了等于没报」）。取不到随机源就如实失败 ——
        // 发一个空 id 出去只会拿回一句看不懂的上游报错
        let Some(event_id) = new_uuid() else {
            let reason = "本机随机源不可用，无法生成事件 id（激活事件未上报）".to_string();
            return Err(first_error.unwrap_or(reason));
        };
        let body = event_body(event, &event_id, user_id, device_mid, &timezone, &os_version);
        let failure = match send_raw("POST", &url, Some(&body), &[], proxy, Some(REQUEST_TIMEOUT_MS))
            .await
        {
            Ok(response) => {
                let payload = response.payload.unwrap_or(Value::Null);
                // 信封 `{code, msg}`：无 body 或 code 缺失按失败处理（参考实现
                // 同样要求 `code == 0`）
                let code = payload.get("code").and_then(Value::as_i64).unwrap_or(-1);
                (code != 0).then(|| {
                    format!("激活事件上报被上游拒绝：{}", envelope_message(&payload, code))
                })
            }
            Err(error) => Some(if error.is_timeout() {
                "激活事件上报超时".to_string()
            } else {
                format!("激活事件上报失败: {error}")
            }),
        };
        if let Some(reason) = failure {
            if first_error.is_none() {
                first_error = Some(reason);
            }
        }
    }
    match first_error {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

/// 一条事件的请求体（字段与取值口径见模块头）。
fn event_body(
    event: &str,
    event_id: &str,
    user_id: &str,
    device_mid: &str,
    timezone: &str,
    os_version: &str,
) -> Value {
    json!({
        "event_id": event_id,
        "client_timezone": timezone,
        "client_language": CLIENT_LANGUAGE,
        // `element_name` 就是事件名（参考实现把两者写成同一个值）
        "element_name": event,
        "event_region": "app",
        "event_type": "view",
        "event_text": "",
        "event_extra_detail": {},
        "user_id": user_id,
        "screen_resolution": SCREEN_RESOLUTION,
        "app_version": app_version(),
        "device_os_category": super::claim::os_category(),
        "device_os_version": os_version,
        "device_mid": device_mid,
        // 这两个字段参考实现发的是空串 / 空对象的字符串形态，照抄：
        // 上游把它们当「这次的来源信息」，本家没有对应内容，别编
        "mac_id": "",
        "marketing_params": "{}",
    })
}

/// 上游信封里的一句话（`msg` → `message` → 「业务码 N」）
fn envelope_message(payload: &Value, code: i64) -> String {
    payload
        .get("msg")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("业务码 {code}"))
}

/// 本机时区名（IANA）——`OnceLock` 缓存，进程内只探测一次。
///
/// 竞态（两个请求同时首探）只会多起一次子进程，值仍以先落进缓存的那个为准 ——
/// 为一次环境探测引入异步 once 的复杂度不值得。
async fn probe_timezone() -> String {
    static CACHE: OnceLock<String> = OnceLock::new();
    if let Some(value) = CACHE.get() {
        return value.clone();
    }
    let probed = timezone_now().await;
    CACHE.get_or_init(|| probed).clone()
}

/// 本机操作系统版本（Windows 的 `10.0.{build}`，其它平台空串）
async fn probe_os_version() -> String {
    static CACHE: OnceLock<String> = OnceLock::new();
    if let Some(value) = CACHE.get() {
        return value.clone();
    }
    let probed = os_version_now().await;
    CACHE.get_or_init(|| probed).clone()
}

/// Windows：`tzutil /g` → IANA 名；其它平台与认不出的名字给 `unknown`。
///
/// 映射表只收参考实现列出的那几个（中国 / 新加坡 / 东京 / UTC）：其余时区在
/// 上游那边只影响统计归属，给 `unknown` 是参考实现自己的兜底，不算丢信息。
/// `fork` 出来取值的写法与参考实现一致：`tzutil` 只在 Windows 存在，
/// 非 Windows 直接短路（连子进程都不起）。
async fn timezone_now() -> String {
    #[cfg(not(windows))]
    {
        "unknown".to_string()
    }
    #[cfg(windows)]
    {
        let output = tokio::process::Command::new("tzutil")
            .arg("/g")
            .output()
            .await
            .ok();
        let name = output
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default();
        match name.as_str() {
            "China Standard Time" | "China Daylight Time" => "Asia/Shanghai",
            "Singapore Standard Time" => "Asia/Singapore",
            "Tokyo Standard Time" => "Asia/Tokyo",
            "UTC" => "UTC",
            _ => "unknown",
        }
        .to_string()
    }
}

/// Windows：注册表 `CurrentBuildNumber` → `10.0.{build}`；其它平台给空串。
async fn os_version_now() -> String {
    #[cfg(not(windows))]
    {
        String::new()
    }
    #[cfg(windows)]
    {
        let output = tokio::process::Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
                "/v",
                "CurrentBuildNumber",
            ])
            .output()
            .await
            .ok();
        let text = output
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_default();
        // `reg query` 的输出形如 `    CurrentBuildNumber    REG_SZ    26200`
        // 取含键名那一行里最后一个非空字段
        text.lines()
            .find(|line| line.contains("CurrentBuildNumber"))
            .and_then(|line| line.split_whitespace().last())
            .map(|build| format!("10.0.{build}"))
            .unwrap_or_default()
    }
}
