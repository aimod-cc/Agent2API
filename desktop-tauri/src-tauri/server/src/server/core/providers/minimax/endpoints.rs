//! MiniMax Code 的地址常量与请求签名。
//!
//! ── 域名按地区分叉（别混，2026-10-21 实测 + `app.asar` 常量表）────
//! ```text
//!   网关（推理 + 账户 + 签到）  国内  https://agent.minimax.cn
//!                              国际  https://agent.minimaxi.com   ← .io 307 跳到这里
//!   授权服务器                  国内  https://account.minimax.cn
//!                              国际  https://account.minimax.io
//! ```
//! 来源：`app.asar` → `@mavis/config/dist/config.js` 的 `PRESET_BASE_URLS`
//! （`cn-prod` / `en-prod`）与 `@mavis/oauth-core/dist/endpoint-config.js` 的
//! `ACCOUNT_ORIGINS`。**域名一律从 `region.rs` 取**（`Region::gateway_url` /
//! `Region::auth_host`），本文件只放路径与签名 —— 别处再写一遍域名字面量
//! 就会出现「某一处忘了分地区」这种编译期看不出来的静默失效。
//!
//! ── 两个地区的协议**完全一致**（实测）────────────────────────
//! 同一套 Anthropic Messages、同一套 MD5 签名（地址不同不影响签名算法：
//! 签名对象是 `pathname + search`，不含域名）、同一个 `client_id=mcode-public`
//! （`@mavis/oauth-core/dist/contracts.js` 里是全局唯一常量，不按地区分）。
//! ⇒ 签名函数不需要 `Region` 参数，只有 URL 需要。
//!
//! ── 两条不同性质的路（**这是本家最容易搞混的一处**）──────────────
//! ```text
//!   推理   POST {gw}/mavis/api/v1/llm/v1/messages     ← 只认 Bearer，**不校验签名**
//!   账户   POST {gw}/matrix/api/v1/…                  ← Bearer + 三签名头 + 设备查询串
//!   签到   GET  {gw}/minimax-cloud/api/v1/signin/…    ← 同上（缺签名 → invalid signature）
//!   目录   GET  {gw}/mavis/api/v1/models              ← **免鉴权**（实测 200）
//! ```
//! 实测（`_recon/minimax-code-contract-2026-10-21.md` §2/§4）：不签名打签到接口
//! 回 `400 {"error":"invalid signature"}`；补签后无 token 回 401；再补假 token
//! 回 `401 {"error":"invalid access token"}`。而推理端点带不带查询串与签名都只
//! 取决于 Bearer ⇒ **签名只用于账户类请求**，转发链不必算它。
//!
//! ── 签名算法（两份客户端实现逐字一致，照抄 `cloud-transport.js`）──
//! ```text
//!   second      = floor(now_ms / 1000)
//!   bodyText    = POST ? JSON.stringify(body) : ""          // GET 是空串，不是 "{}"
//!   yyBody      = bodyText || "{}"                          // yy 里 GET 用 "{}"
//!   yy          = md5( encodeURIComponent(pathname + search) + "_" + yyBody
//!                      + md5(String(now_ms)) + "ooui" )
//!   x-timestamp = String(second)
//!   x-signature = md5( second + SALT + bodyText )
//! ```
//! ⚠️ 三处细节，错一个就 `invalid signature`：
//!   1. `encodeURIComponent` 的保留集是 `A-Za-z0-9-_.!~*'()`，**`:/` 都要编码**；
//!   2. 签名对象是 **pathname + search 整体**，所以查询串顺序也是签名的一部分；
//!   3. `yy` 的 body 段与 `x-signature` 的 body 段**规则不同**（前者 GET 用 `{}`，
//!      后者 GET 用空串）。抄成同一份必错。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use md5::{Digest, Md5};

/// 推理端点（相对网关）：**Anthropic Messages 协议**。
pub const MESSAGES_PATH: &str = "/mavis/api/v1/llm/v1/messages";

/// 模型目录端点（相对网关）：**免鉴权**，实测 200。
pub const CATALOG_PATH: &str = "/mavis/api/v1/models";

/// 会员 / 积分摘要端点（相对网关）：余额主读数。
pub const MEMBERSHIP_PATH: &str = "/matrix/api/v1/commerce/get_membership_info";

/// 逐份积分批次端点（相对网关）：到期两列的数据源。
pub const CREDIT_DETAILS_PATH: &str = "/minimax-cloud/api/v1/credit/details";

/// 签到面板端点（相对网关）：七天一轮的进度。
pub const SIGNIN_STATUS_PATH: &str = "/minimax-cloud/api/v1/signin/status";

/// 签到领取端点（相对网关）。
pub const SIGNIN_CLAIM_PATH: &str = "/minimax-cloud/api/v1/signin/claim";

/// 设备码申请（相对授权服务器）。
pub const DEVICE_CODE_PATH: &str = "/oauth2/device/code";

/// 换令牌 / 续期（相对授权服务器）。
pub const TOKEN_PATH: &str = "/oauth2/token";

/// 设备授权的 grant type（RFC 8628）。
pub const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// 组装一条绝对地址（网关类端点：推理 / 目录 / 账户 / 签到）。
///
/// 存在的理由只有一个：**域名只从 `region.rs` 取**。调用点写
/// `gateway_url(region, MEMBERSHIP_PATH)` 而不是自己 `format!` 域名，
/// 就不会出现「新加一条支线时忘了分地区」那种编译期看不出来的静默失效。
pub fn gateway_url(region: crate::server::core::providers::minimax::region::Region, path: &str) -> String {
    format!("{}{}", region.gateway_url(), path)
}

/// 组装一条绝对地址（授权服务器类端点：设备码 / 换令牌 / 续期）。
pub fn auth_url(region: crate::server::core::providers::minimax::region::Region, path: &str) -> String {
    format!("{}{}", region.auth_host(), path)
}

/// OAuth client id：客户端自己的公开 client（`mcode-public`）。
///
/// 来源：`~/.minimax/auth/prod/cn/mcode-public/auth-state.json` 的 `clientId`，
/// 以及 `dsh-connect-minimaxcode` 的 `MINIMAX_OAUTH_CLIENT_ID`（两处一致）。
pub const OAUTH_CLIENT_ID: &str = "mcode-public";

/// OAuth scope：客户端申请的唯一个。
pub const OAUTH_SCOPE: &str = "agent.default";

/// OAuth audience：令牌签发对象。
pub const OAUTH_AUDIENCE: &str = "agent-backend";

/// 签名盐（`cloud-transport.js` 的 `SIGNATURE_SALT`）。
const SIGNATURE_SALT: &str = "I*7Cf%WZ#S&%1RlZJ&C2";

/// `yy` 头固定的后缀（`YY_SUFFIX`）。
const YY_SUFFIX: &str = "ooui";

/// 设备参数里的客户端版本号（`version_code`，客户端 `buildPublicParams` 同值）。
const VERSION_CODE: &str = "22201";

/// 一次账户类请求的签名物。
pub struct Signed {
    /// 带设备查询串的相对路径（`path?query`，直接拼在网关上）
    pub path_with_query: String,
    /// 三个签名头（`yy` / `x-timestamp` / `x-signature`）
    pub headers: Vec<(String, String)>,
}

/// 小写 hex 的 MD5。
fn md5_hex(text: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(32);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 复刻 JavaScript 的 `encodeURIComponent`。
///
/// 放行集是 `A-Za-z0-9` 与 `- _ . ! ~ * ' ( )`，其余（含 `:/?&=+%` 与非 ASCII）
/// 一律 `%XX` 大写。**不能**用 `url::form_urlencoded`：它把空格编码成 `+`、
/// 且不编码 `*`，两者都会让签名对不上。
pub fn encode_uri_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        let keep = matches!(byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 设备参数的地区标记（`buildPublicParams` 的 `region` 槽）。
///
/// 客户端按运行期地区写 `cn` / `en`（`getRuntimeRegion()` 的两个取值），
/// 本仓**固定写 `cn`**。这个结论是有实测支撑的，不是省事：
/// ```text
///   真实国内令牌（客户端 auth/prod/cn）打 .minimax.cn / .minimaxi.com
///   的余额 / 台账 / 签到 / 推理四条，region=cn 与 region=en **都返回 200**。
/// ```
/// 也就是说它是「上游只用来做统计/路由提示、不参与鉴权」的一格。
/// ⇒ 不为它再分一个「国际版写 `en`」的分支：一个没有实测差异的分叉
///   只会多一个出错点。
///
/// ⚠️ 别再拿**假令牌**去测这一格（曾如此误判过）：假令牌会停在
/// `401 invalid access token`，两种取值看起来一模一样，什么也证明不了 ——
/// 要验证这一格必须用真令牌。
const DEVICE_QUERY_REGION: &str = "cn";

/// 设备参数查询串（顺序即构造顺序，**不要改成 Map 遍历**）。
///
/// `user_id` 取 `real_user_id`（客户端里它是登录身份，缺省写 `0`——
/// 客户端的 `buildPublicParams` 在未登录时不写这个键，但 `quota/transport.js`
/// 那条写 `?? '0'`；两种上游都接受，这里跟后者写 `0`，让 URL 形状稳定）。
pub fn device_query(now_ms: i64, timezone_offset: i64, user_id: &str) -> String {
    let user = if user_id.trim().is_empty() { "0" } else { user_id.trim() };
    format!(
        "device_platform=web&biz_id=3&app_id=3001&version_code={VERSION_CODE}\
         &unix={now_ms}&timezone_offset={timezone_offset}&sys_language=zh&lang=zh\
         &device_id=0&user_id={user}&client=desktop&region={region}&is_desktop=1",
        now_ms = now_ms,
        timezone_offset = timezone_offset,
        user = user,
        region = DEVICE_QUERY_REGION,
    )
}

/// 算一次账户类请求的签名（见模块头的三处细节）。
///
/// * `path` —— 不含域名的路径（如 `/minimax-cloud/api/v1/signin/status`）；
/// * `extra_query` —— 调用方自己的查询串（如 `timezone_id=Asia%2FShanghai`），
///   拼在设备参数**之前**（顺序只影响签名，与上游语义无关，固定即正确）；
/// * `body_text` —— POST 时为序列化后的 JSON 串，GET 时为空串。
pub fn sign(
    path: &str,
    extra_query: &str,
    body_text: &str,
    now_ms: i64,
    timezone_offset: i64,
    user_id: &str,
) -> Signed {
    let mut query = String::new();
    if !extra_query.trim().is_empty() {
        query.push_str(extra_query.trim());
        query.push('&');
    }
    query.push_str(&device_query(now_ms, timezone_offset, user_id));
    let path_with_query = format!("{path}?{query}");

    let second = now_ms.div_euclid(1000);
    // yy 的 body 段：GET（body_text 为空）用字面量 `{}`，POST 用真实 body
    let yy_body = if body_text.is_empty() { "{}" } else { body_text };
    let yy = md5_hex(&format!(
        "{}_{}{}{}",
        encode_uri_component(&path_with_query),
        yy_body,
        md5_hex(&now_ms.to_string()),
        YY_SUFFIX
    ));
    // x-signature 的 body 段：GET 就是空串（不是 `{}`）
    let signature = md5_hex(&format!("{second}{SIGNATURE_SALT}{body_text}"));

    Signed {
        path_with_query,
        headers: vec![
            ("yy".to_string(), yy),
            ("x-timestamp".to_string(), second.to_string()),
            ("x-signature".to_string(), signature),
        ],
    }
}

/// 时区偏移（秒）。客户端由 `new Date().getTimezoneOffset() * -60` 得到。
///
/// ── 为什么跟随系统时区而不是固定 28800 ───────────────────────
/// 它决定上游按哪一天判「今天是哪天」（签到）。写死 28800 时，系统时区不在
/// UTC+8 的用户（出差、系统时区设错）会看到与我们界面上不同的日界。两个地区
/// 都走这条：上游只要求「与本地口径一致」，不要求「必须是东八区」。
pub fn timezone_offset() -> i64 {
    // 用系统时区算，而不是写死 28800：国内用户在 UTC+8 之外（少数场景：出差、
    // 系统时区设错）时，写死的值会让上游按「北京日」判签到天数，与用户看到的
    // 日历不一致；而 `x-timestamp` 本来就是本地口径，跟随系统更自洽。
    // 拿不到偏移时退回 28800（客户端与两份参考实现都用它）。
    match chrono_offset_seconds() {
        Some(offset) => offset,
        None => 28800,
    }
}

/// 本机时区偏移（秒，东正西负）。取不到返回 None。
fn chrono_offset_seconds() -> Option<i64> {
    // 与 `marvis/balance.rs` 的 `local_time_ms` 同一思路：不引新依赖，
    // 用 chrono 的 Local 与固定偏移之差算出「本机比 UTC 快多少秒」。
    use chrono::{Local, Offset};
    let now = Local::now();
    let offset = now.offset().fix().local_minus_utc();
    Some(i64::from(offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_uri_component_keeps_the_js_reserved_set() {
        // 放行集内的字符原样、`/` 与 `:` 与 `&` 必须编码 —— 这两条是最容易错的地方
        assert_eq!(encode_uri_component("aZ0-_.!~*'()"), "aZ0-_.!~*'()");
        assert_eq!(encode_uri_component("/a/b"), "%2Fa%2Fb");
        assert_eq!(encode_uri_component("a?b=c&d"), "a%3Fb%3Dc%26d");
        // 非 ASCII 按 UTF-8 字节逐个编码
        assert_eq!(encode_uri_component("日"), "%E6%97%A5");
    }

    #[test]
    fn signature_matches_the_reference_formula() {
        // 用固定时刻手算一遍：公式错在任何一处都会在真机上变成 invalid signature，
        // 所以这里把四个中间量都钉住。
        let signed = sign(
            "/minimax-cloud/api/v1/signin/status",
            "timezone_id=Asia%2FShanghai",
            "",
            1_792_551_600_000,
            28800,
            "42",
        );
        assert_eq!(signed.headers[1].0, "x-timestamp");
        assert_eq!(signed.headers[1].1, "1792551600");
        // GET 的 x-signature = md5(second + SALT + "") —— 与 body 无关
        assert_eq!(
            signed.headers[2].1,
            md5_hex(&format!("1792551600{SIGNATURE_SALT}"))
        );
        // yy 必须含 encodeURIComponent 后的整条 path?query
        let expected_yy = md5_hex(&format!(
            "{}_{}{}{}",
            encode_uri_component(&signed.path_with_query),
            "{}",
            md5_hex("1792551600000"),
            YY_SUFFIX
        ));
        assert_eq!(signed.headers[0].1, expected_yy);
    }

    #[test]
    fn post_body_feeds_both_signatures() {
        let body = "{\"workspace_id\":\"w1\"}";
        let signed = sign("/matrix/api/v1/x", "", body, 1_700_000_000_123, 28800, "");
        assert_eq!(
            signed.headers[2].1,
            md5_hex(&format!("1700000000{SIGNATURE_SALT}{body}"))
        );
        let expected_yy = md5_hex(&format!(
            "{}_{}{}{}",
            encode_uri_component(&signed.path_with_query),
            body,
            md5_hex("1700000000123"),
            YY_SUFFIX
        ));
        assert_eq!(signed.headers[0].1, expected_yy);
    }

    #[test]
    fn device_query_keeps_the_client_order_and_defaults_user_to_zero() {
        let query = device_query(1_700_000_000_000, 28800, "");
        assert!(query.starts_with("device_platform=web&biz_id=3&app_id=3001&version_code=22201&unix="));
        assert!(query.contains("&device_id=0&user_id=0&client=desktop&region=cn&is_desktop=1"));
        assert!(query.contains("&timezone_offset=28800"));
        // 非空 user_id 原样进
        assert!(device_query(1, 0, "abc").contains("&user_id=abc&"));
    }

    #[test]
    fn empty_extra_query_does_not_leave_a_stray_ampersand() {
        let signed = sign("/a", "", "", 1_700_000_000_000, 0, "");
        assert!(signed.path_with_query.starts_with("/a?device_platform=web"));
    }
}
