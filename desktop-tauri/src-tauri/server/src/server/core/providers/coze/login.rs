//! Coze 登录：**主站网页登录**（`www.coze.cn`）+ 壳侧读整包 Cookie 收尾。
//!
//! ── 为什么必须是网页登录（而不是 API 复刻）──────────────────
//! Coze 的登录分三层：抖音账号体系（`passport`）+ 风控（图形验证 / 短信
//! 频控）+ 第三方 OAuth（微信 / 抖音 / 阿里账号都可能被选中）。发码接口带
//! 页面 JS 生成的签名参数，纯 HTTP 复刻既不稳也随时失效。把「怎么登录」还给
//! 用户：打开主站，用户像平时一样扫码 / 短信登录（官方页面自己处理风控），
//! 壳侧从 WebView2 的 Cookie 存储把 `.coze.cn` 域**整包** Cookie 交回。
//!
//! ── 流程 ──────────────────────────────────────────────────
//!   1. 适配器 `build_login_url` 返回主站地址 `https://www.coze.cn/`；
//!   2. 壳侧打开内嵌窗口（可见），用户在窗口里完成登录；
//!   3. 壳侧轮询 WebView2 Cookie 存储：出现 `sessionid_ss`（或 `sessionid`）
//!      即登录成功，把全部 `coze.cn` 域 Cookie 拼成 Cookie 头
//!      `POST /api/session/login/coze/complete`
//!      （见 `desktop-tauri/src/login.rs` 的 `submit_coze_login`）；
//!   4. 后端 `complete_login` 打 `passport/account/info/v2/` 复核并取
//!      `user_id_str` / `screen_name`，再由 `core::login` 落账号。
//!
//! ── 为什么必须交回**整包**（2026-10-21 实测）──────────────
//! `passport/account/info/v2/` 与 `incentive/*` 校验完整浏览器会话：只带
//! `sessionid_ss` 能过一部分接口，但缺 `ttwid` / `odin_tt` / `sid_guard`
//! 时任务中心与机器人列表会判「未登录」。主站登录天然下发这些，因此
//! 「整包交回」是这条链的正常形态，不要学别家只挑几个。
//!
//! ── 安全边界 ───────────────────────────────────────────────
//! 收尾路由**免鉴权**（调用方是壳侧登录窗口，不带 API Key），安全性由登录
//! 任务的一次性 state 承担（与 raccoon / kuku 回调同口径）。Cookie 走本机
//! loopback 请求体，一次性传输、任务结束后即作废。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use crate::server::errors::GatewayError;

use super::credentials::{self, CozeCredentials};
use super::WEB_BASE;

/// 生成登录任务 state（不可预测的随机串）。
pub fn new_login_state() -> String {
    random_hex(32)
}

/// Coze 登录页地址：**主站**（用户像平时用网页版一样登录）。
///
/// 不直接开 `passport` 子页：主站首页就是登录入口（二维码 / 短信 / 密码三
/// tab），且登录成功后会回到首页 —— 壳侧判「窗口离开登录页 + Cookie 里出现
/// `sessionid_ss`」比盯某个 DOM 稳得多。
pub fn build_login_url(_state: &str) -> String {
    format!("{WEB_BASE}/")
}

/// 复核结果：凭证 + 可选的「未通过复核」警告（登录收尾透传前端）。
pub type CompleteOutcome = (CozeCredentials, Option<String>);

/// 从壳侧交回的 Cookie 串里提取登录态，并用 `passport/account/info/v2/` 复核。
///
/// ── 复核失败**不阻断**登录，但**带警告**────────────────────
/// 用户在窗口里刚登录成功，凭证本身有效（`sessionid_ss` 存在）。复核失败通常
/// 是「Cookie 里少了 `ttwid` / `odin_tt`」这类完整性问题 —— 此时仍返回凭证
/// （uid 留空，账号层用 cookie 尾兜底 id），同时给一条警告说明补救办法。
/// 真正拦下来的是**连 `sessionid_ss` 都没有**那种情况（窗口还没登录完）。
pub async fn complete_login(cookie: &str) -> Result<CompleteOutcome, GatewayError> {
    let cookies = credentials::parse_cookie_parts(&serde_json::Value::String(cookie.to_string()));
    let credentials = CozeCredentials {
        cookies,
        user_id: String::new(),
        nickname: String::new(),
    };
    if !credentials.has_session() {
        return Err(GatewayError::with_status(
            400,
            "登录窗口没有捕获到 Coze 登录态（Cookie 里没有 sessionid_ss），请重新发起登录",
        ));
    }
    match credentials::enrich_identity(&credentials, None).await {
        Ok(enriched) => Ok((enriched, None)),
        Err(error) => {
            crate::server::logging::log(
                "[Login]",
                &format!(
                    "⚠️ Coze 登录态已交回但复核未通过（{}）；先按有效凭证落库，\
                     uid 留空待后续刷新补齐",
                    error.message
                ),
            );
            Ok((
                credentials,
                Some(
                    "Coze 账号信息复核未通过（登录 Cookie 可能不完整）：账号已添加，\
                     但任务中心 / 机器人列表可能报「未登录」。请重新登录，\
                     或粘贴 www.coze.cn 的完整 Cookie"
                        .to_string(),
                ),
            ))
        }
    }
}

/// 随机十六进制串（登录 state 用）。
fn random_hex(length: usize) -> String {
    let mut out = String::new();
    while out.len() < length {
        let mut bytes = [0u8; 16];
        let _ = getrandom::getrandom(&mut bytes);
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
            if out.len() >= length {
                break;
            }
        }
    }
    out
}
