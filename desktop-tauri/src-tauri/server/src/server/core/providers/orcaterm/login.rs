//! OrcaTerm 登录：**官方 Web 控制台登录页** + 壳侧读 `ot_session` Cookie 收尾。
//!
//! ── 上游有几条登录链路（2026-10-16 实测对照）────────────────────
//! 同一套账号体系在三个入口各走各的协议，网关能用的只有第三条：
//!   1. `orcaterm.com/oauth/authorize?provider=wechat&source=desktop` —— 桌面
//!      客户端那条：回调跳到自定义协议 `orcaterm://oauth_callback?session_id=…`，
//!      再由客户端调 `exchangeToken(session_id)` 换令牌。**回调只认自定义协议**，
//!      而 WebView2 的登录窗口没有 Electron 那种协议接管能力（见
//!      `desktop-tauri/src/login.rs` 模块头），所以这条在壳里收不到；
//!   2. `open.weixin.qq.com/connect/qrconnect`（appid `wx56e5199ef872dc22`，
//!      OrcaTerm 自己的开放平台应用）—— 服务端换码接口
//!      `POST /oauth/wechat/exchange {code, state}` 的 state 由上游
//!      `/oauth/wechat/config` 现发、绑定服务端会话，网关**代跑没有把握**
//!      （拿别人的 state 换码，失败形态只有 `STATE_EXPIRED` / `STATE_MISMATCH`，
//!      无从区分是时序问题还是绑定问题）；
//!   3. **Web 控制台**（`orcaterm.cloud.tencent.com/terminal`）—— 界面左下角
//!      头像 → 立即登录 → 微信扫码（走云账号 `cloud.tencent.com` 的
//!      `wxQrcode` + qrconnect 内嵌 iframe），登录成功后控制台把令牌写进
//!      **`ot_session` Cookie**（`document.cookie`，非 HttpOnly，
//!      `AuthStore.syncCookie`）。登录态判据也是这个 Cookie（未登录时
//!      完全不存在，实测对照：匿名访问拿不到任何 `ot_session`）。
//!
//! 本模块采用第 3 条：把「怎么登录」还给用户（在官方控制台里正常扫码），
//! 网关只在旁边等一枚 Cookie —— 与 KukuAI 主站方案同一形态
//! （`providers/kuku/login.rs` 模块头），也是本仓库既有的
//! `POST /api/session/login/<家>/complete` 收尾口径（壳侧进程提交，一次性
//! state 承担安全性）。
//!
//! ── 为什么不做「自动化扫码页」──────────────────────────────
//! 上游的扫码是**云账号**（`cloud.tencent.com`）那一套：iframe 里的
//! `qrconnect` 地址带一次性 `intent_id` 与 CSRF state（由控制台 JS 现算），
//! 复刻它等于把上游前端实现抄一遍 —— 与 KukuAI 放弃 passport 页面自动化
//! 同一条理由（页面 DOM 与风控行为随版本变）。壳侧只做一件低风险的事：
//! 打开控制台后**帮用户点开登录面板**（头像 → 立即登录），点不动就等用户
//! 自己点（见壳侧注入脚本的说明），扫码与换令牌全在官方页面里完成。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::errors::GatewayError;

use super::credentials::{self, OrcatermCredentials};

/// 登录窗口打开的地址：**Web 控制台首页**。
///
/// 为什么不直接开 `orcaterm.com/oauth/authorize`（本家自己的 OAuth 页）：
/// 那条是给桌面客户端用的（回调走自定义协议，见模块头第 1 条），Web 版控制台
/// 的登录走的是云账号那一套（第 3 条），而网关要的正是「控制台登录完成后的
/// `ot_session`」—— 从控制台本身进站，登录方式、回调地址、登录态写入全是
/// 官方自己那套，上游改版也不会让网关这边失配。
///
/// `state` 不进 URL（与 KukuAI 同款）：这条链路的交回由壳侧进程主动 POST
/// （`POST /api/session/login/orcaterm/complete`），state 在请求体里，
/// 不需要回调地址承载它。
pub fn build_login_url(_state: &str) -> String {
    "https://orcaterm.cloud.tencent.com/terminal".to_string()
}

/// 生成登录任务 state（不可预测的随机串；与各家 `new_login_state` 同源）。
pub fn new_login_state() -> String {
    crate::server::access::random_hex(32)
}

/// 壳侧交回登录态 Cookie（`ot_session=…` 或裸 token）→ 归一化 → 真实校验。
///
/// ── 为什么必须打一次 `GetUserInfo` ──────────────────────────
/// Cookie 里可能是一枚**已过期 / 已被上游撤销**的令牌（控制台窗口存活期间
/// 换过号、上游顶号等），JWT 还能解出 `userId` 与 `exp`，落库却不可用 ——
/// 与「粘贴 accessToken」入口同一口径（`api::accounts` 的 OrcaTerm 分支）：
/// 添加时校验「现在真的能用」，失败不落空记录。
///
/// 校验失败**不落账号**：与 KukuAI「凭证有效但业务复核没过、带警告落库」不同 ——
/// 那家的 BDUSS 本身是有效登录态（换发业务令牌是另一回事），而这里
/// `GetUserInfo` 被拒就意味着这枚令牌根本不能用（续期接口 `OAuthRefreshToken`
/// 同样会拒），落下来只会得到一个必然 401 的账号。
pub async fn complete_login(token: &str) -> Result<OrcatermCredentials, GatewayError> {
    let raw = token.trim();
    if raw.is_empty() {
        return Err(GatewayError::with_status(400, "登录窗口没有捕获到 ot_session，请重新发起登录"));
    }
    let normalized = credentials::normalize_token(raw);
    if normalized.split('.').count() != 3 {
        return Err(GatewayError::with_status(
            400,
            "登录窗口捕获到的 ot_session 不是一个有效的 OrcaTerm 令牌（JWT 应为三段点分），请重新发起登录",
        ));
    }
    let mut parsed = credentials::credentials_from_payload(&Value::String(normalized.clone()))
        .map_err(|reason| GatewayError::with_status(400, reason))?;
    // 已经过期的令牌**当场拒收**（而不是落库后靠续期救）：刚落库的账号第一次
    // 转发就会 401，用户体验与「添加成功」的提示自相矛盾；而窗口里出现过期
    // 令牌只有一种情形 —— 用户其实没扫完码/扫的是别处留下的旧 Cookie，
    // 让他重新扫一次才是对的。
    //
    // 只判「真的过期」而不判「临期」（与 `is_expiring` 的 10 分钟窗口区分开）：
    // 临期令牌仍是可用的，落库后由续期机制接管（`OAuthRefreshToken` 实测能
    // 拿任意有效旧令牌换新），为此把一次成功的扫码判成失败不值当。
    let expired = parsed.expires_at > 0
        && parsed.expires_at <= crate::server::logging::now_ms() / 1000;
    if expired {
        return Err(GatewayError::with_status(
            400,
            "登录窗口里的 ot_session 已过期，请在窗口里重新扫码登录",
        ));
    }
    // 归一化后的 token 再放回凭证：`credentials_from_payload` 只认裸 token，
    // 而壳侧交回的是 Cookie 形态（`ot_session=…`），这里保证落库的是裸 JWT。
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