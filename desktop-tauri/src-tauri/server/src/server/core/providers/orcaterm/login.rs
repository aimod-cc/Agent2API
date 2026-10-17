//! OrcaTerm 登录：**官方桌面端 OAuth 链路**（`orcaterm.com/oauth/authorize`
//! → 微信扫码 → `orcaterm://oauth_callback?session_id=…` → `OAuthExchangeToken`）。
//!
//! ── 为什么不用 Web 控制台那条（第二轮修正，必读）───────────────
//! 初版走的是「打开 Web 控制台、等壳侧读到 `ot_session` Cookie」。实测发现
//! 那条**在内嵌窗口里不可能成功**：`ot_session` 只由控制台自己的 `AuthStore`
//! 在四条路径上写入（`syncCookie`），其中
//!   - `bootSync` 需要页面里存在服务端注入的 bootCache —— 实测服务端**不注入**
//!     （带 `ot_session` 抓首页时 HTML 里没有 bootCache / auth_identity）；
//!   - `onServerTokenRefreshed` 需要页面收到带 `X-OT-Token-Refreshed` 的响应；
//!   - `onOAuthExchanged` 只在**桌面端深链**（`orcaterm://oauth_callback`）里走；
//!   - `runReconcile` 从 `UNAUTHED` 直接 return，未登录时根本不写。
//! 于是用户在控制台里扫码成功后，那枚 Cookie 永远不会出现 —— 壳侧读到超时，
//! 用户看到的现象就是「扫码完了，应用没有任何反应」。
//!
//! ── 现在这条链（2026-10-17 实测）────────────────────────────
//!   1. 打开 `https://orcaterm.com/oauth/authorize?provider=wechat&source=desktop
//!      &session_id=<uuid>&return_url=orcaterm://oauth_callback` —— 实测该页
//!      **302 直达**微信二维码（appid `wx56e5199ef872dc22`，OrcaTerm 自己的
//!      开放平台应用），不需要点任何按钮；
//!   2. 用户扫码。上游把浏览器重定向到 `return_url`，并带上 `session_id`；
//!   3. 回调**由后端后台任务轮询换令牌**：`OAuthExchangeToken {sessionId}`
//!      → `{AccessToken, ExpiresIn}`。实测该 CGI 动作**不需要 Authorization
//!      头**（匿名请求只报「sessionId must be a UUID」/「登录会话已过期」这类
//!      业务错误），因此换令牌这一跳不依赖壳侧读任何窗口状态。
//!
//! 与 KukuAI / 小浣熊等「回调带 code」的家不同：这里的回调 URL 只带
//! `session_id`（一个我们自己生成的 UUID，既是 CSRF 凭据也是换令牌的入参）。
//! 壳侧把回调 URL 原样交给通用 `POST /api/session/login/callback`，
//! 由本模块的 [`exchange_login_code`] 解析并换令牌落账号 —— 与
//! CodeArts / AutoClaw 同款形态（回调 URL → 后端换码 → 落账号）。
//!
//! ── 为什么不再需要自定义协议接管 ─────────────────────────────
//! 自定义协议是**官方桌面客户端**的收尾方式；网关不需要接管它 —— 只要把
//! `return_url` 指向**我们自己的**回跳地址即可（实测 `http://127.0.0.1:<port>/…`
//! 与 `orcaterm://oauth_callback` 都被接受）。这里仍用 `orcaterm://oauth_callback`
//! 作为 `return_url`：壳侧登录窗口的 `on_navigation` 把它当成回调识别下来
//! （与 `office-raccoon://auth/callback` 同一机制），交给后端换令牌 ——
//! 这样既不依赖系统里装没装官方客户端，也不把回调暴露给别的进程。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::errors::GatewayError;

use super::credentials::{self, OrcatermCredentials};

/// 桌面端 OAuth 的回调地址（自定义协议）。
///
/// 上游把登录成功后的浏览器重定向到它并带上 `session_id`。壳侧登录窗口的
/// `on_navigation` 认这个地址（见 `is_login_callback`），把整个 URL 交回后端。
pub const CALLBACK_URL: &str = "orcaterm://oauth_callback";

/// 回调 URL 里承载一次性凭据的参数名（上游用它传我们生成的 UUID）。
pub const CALLBACK_QUERY_KEY: &str = "session_id";

/// 登录窗口打开的地址：官方 OAuth 授权页（自动跳微信二维码）。
///
/// `session_id` 由 [`new_login_state`] 生成并进 URL —— 它是这次授权的
/// 一次性凭据，回调与换令牌都用它；`state`（登录任务表键）与它**是同一个值**：
/// 回调侧只能看到 URL 里的 `session_id`，两者分开就要在任务表外再存一份映射，
/// 而它们本来就一一对应（见 `LoginTaskState.state` 的注释口径）。
pub fn build_login_url(state: &str) -> String {
    format!(
        "https://orcaterm.com/oauth/authorize?provider=wechat&source=desktop&session_id={state}&return_url={}",
        urlencoding(CALLBACK_URL)
    )
}

/// 生成登录 state / OAuth session_id（不可预测的随机串）。
///
/// **必须是合法 UUID**：实测上游对 `sessionId` 做格式校验，非 UUID 直接回
/// `api-params-verify-fail: sessionId must be a UUID`。这里把随机串切成
/// 8-4-4-4-12 的十六进制形态（不引入 uuid 依赖）。
pub fn new_login_state() -> String {
    // `random_hex(16)` 返回 **32 个** 十六进制字符（每字节两个），正好是 UUID 的
    // 位数 —— 传 32 会得到 64 个字符，下面切片用的是 32 位口径，那样会越界 panic
    // （release 是 `panic=abort`，属必须避免的形态）。
    let hex = crate::server::access::random_hex(16);
    let mut chars: Vec<char> = hex.chars().collect();
    if chars.len() < 32 {
        // 系统随机源契约是「每字节两个字符」，拿不到 32 位就是上游实现变了；
        // 不 panic，退化成固定长度的兜底串（后续校验会以「会话过期」如实报错）。
        return "00000000-0000-4000-8000-000000000000".to_string();
    }
    // 版本位与变体位按 UUID v4 的口径写死（上游只校验形态，但既然叫 UUID
    // 就写成合规的 v4，免得哪天校验变严：第 13 位固定 4，第 17 位取 8-b）。
    chars[12] = '4';
    chars[16] = match chars[16] {
        '0' | '1' | '2' | '3' => '8',
        '4' | '5' | '6' | '7' => '9',
        '8' | '9' | 'a' | 'b' => 'a',
        _ => 'b',
    };
    let text: String = chars[..32].iter().collect();
    format!(
        "{}-{}-{}-{}-{}",
        &text[0..8],
        &text[8..12],
        &text[12..16],
        &text[16..20],
        &text[20..32]
    )
}

/// 最小 URL 编码（`return_url` 的取值里只有 `:` 与 `/` 需要转义）。
fn urlencoding(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            ':' => out.push_str("%3A"),
            '/' => out.push_str("%2F"),
            '?' => out.push_str("%3F"),
            '&' => out.push_str("%26"),
            '=' => out.push_str("%3D"),
            other => out.push(other),
        }
    }
    out
}

/// 从回调 URL 里取出 `session_id`（没有就是 None）。
pub fn callback_session_id(callback_url: &str) -> Option<String> {
    let query = callback_url.split_once('?')?.1;
    for part in query.split('&') {
        let (key, value) = part.split_once('=')?;
        if key == CALLBACK_QUERY_KEY && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// 回调换令牌：`session_id` → `OAuthExchangeToken` → 落账号。
///
/// ── 为什么敢在这里直连上游不鉴权 ────────────────────────────
/// 实测该动作匿名可调，凭据就是 `session_id` 本身（一次性、由本进程生成、
/// 只出现在我们自己窗口的 URL 里）—— 安全性由「不可预测 + 一次性」承担，
/// 与设备授权码（Cline）同一口径。
///
/// ── 失败形态 ────────────────────────────────────────────────
///   - `TOKEN_EXPIRED`「登录会话已过期」：用户没扫完就关了页面 / 扫得太慢，
///     文案原样透出（可行动）；
///   - 其余业务错误原样透出（含 Code）。
pub async fn exchange_session(
    session_id: &str,
) -> Result<OrcatermCredentials, GatewayError> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err(GatewayError::with_status(400, "回调里没有 session_id，无法换取登录态"));
    }
    let response = credentials::cgi_action_anonymous(
        "OAuthExchangeToken",
        serde_json::json!({ "sessionId": session_id }),
        None,
    )
    .await?;
    if let Some(error) = response.get("Error") {
        let code = error.get("Code").and_then(Value::as_str).unwrap_or("UNKNOWN");
        let message = error.get("Message").and_then(Value::as_str).unwrap_or("上游错误");
        // 会话过期是用户可行动的错误（重新扫一次），单独给一句更直白的文案
        if code == "TOKEN_EXPIRED" {
            return Err(GatewayError::with_status(
                400,
                "OrcaTerm 登录会话已过期（扫码可能没完成或等待过久），请重新发起网页登录",
            ));
        }
        return Err(GatewayError::with_status(
            401,
            format!("OrcaTerm 换取登录态失败（{code}：{message}）"),
        ));
    }
    let token = response
        .get("AccessToken")
        .or_else(|| response.get("accessToken"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if token.split('.').count() != 3 {
        return Err(GatewayError::with_status(
            502,
            "OrcaTerm 换回来的登录态不是有效令牌（JWT 应为三段点分），请重试",
        ));
    }
    let mut parsed = credentials::credentials_from_payload(&Value::String(token.clone()))
        .map_err(|reason| GatewayError::with_status(400, reason))?;
    parsed.access_token = token;
    // 换回来的令牌理论上必定有效（刚由上游铸造），但仍然打一次探活补齐身份 ——
    // 下游落账号要用 nickname / user_id，且探活能兜住「铸造成功但账号被冻结」
    // 这类边缘情形（与粘贴入口同一口径）。
    let (user_id, nickname) = credentials::fetch_user_info(&parsed, None).await?;
    if parsed.user_id.is_empty() {
        parsed.user_id = user_id;
    }
    if parsed.nickname.is_empty() {
        parsed.nickname = nickname;
    }
    Ok(parsed)
}

/// 壳侧交回登录态 Cookie / 令牌（`ot_session=…` 或裸 token）→ 归一化 → 真实校验。
///
/// 保留给「粘贴 / 导入」以外的兜底入口：既有的 `POST /api/session/login/orcaterm/complete`
/// 仍可用（例如壳侧哪天从窗口里读到了令牌），与官方 OAuth 链并存，两条都落到
/// 同一套「校验通过才落库」的口径上。
pub async fn complete_login(token: &str) -> Result<OrcatermCredentials, GatewayError> {
    let raw = token.trim();
    if raw.is_empty() {
        return Err(GatewayError::with_status(400, "没有捕获到 OrcaTerm 登录态，请重新发起登录"));
    }
    let normalized = credentials::normalize_token(raw);
    if normalized.split('.').count() != 3 {
        return Err(GatewayError::with_status(
            400,
            "捕获到的内容不是一个有效的 OrcaTerm 令牌（JWT 应为三段点分），请重新发起登录",
        ));
    }
    let mut parsed = credentials::credentials_from_payload(&Value::String(normalized.clone()))
        .map_err(|reason| GatewayError::with_status(400, reason))?;
    // 已经过期的令牌**当场拒收**（而不是落库后靠续期救）：刚落库的账号第一次
    // 转发就会 401，用户体验与「添加成功」的提示自相矛盾。
    //
    // 只判「真的过期」而不判「临期」（与 `is_expiring` 的 10 分钟窗口区分开）：
    // 临期令牌仍是可用的，落库后由续期机制接管（`OAuthRefreshToken` 实测能
    // 拿任意有效旧令牌换新），为此把一次成功的登录判成失败不值当。
    let expired = parsed.expires_at > 0
        && parsed.expires_at <= crate::server::logging::now_ms() / 1000;
    if expired {
        return Err(GatewayError::with_status(
            400,
            "OrcaTerm 登录态已过期，请重新发起网页登录",
        ));
    }
    // 归一化后的 token 再放回凭证：`credentials_from_payload` 只认裸 token，
    // 而调用方可能交回 Cookie 形态（`ot_session=…`），这里保证落库的是裸 JWT。
    parsed.access_token = normalized;
    let (user_id, nickname) = credentials::fetch_user_info(&parsed, None).await?;
    if parsed.user_id.is_empty() {
        parsed.user_id = user_id;
    }
    if parsed.nickname.is_empty() {
        parsed.nickname = nickname;
    }
    Ok(parsed)
}
