//! KukuAI 登录：**主站网页登录**（`kuku.baidu.com`）+ 壳侧读完整会话 Cookie 收尾。
//!
//! ── 为什么不是纯 API 短信、也不是 passport 页面自动化 ─────────
//! 百度 passport 的短信发码（`senddpass`）带一串 wappass 风控签名参数（由
//! 页面 JS 的风控库生成），纯 HTTP 复刻既不稳定也随时可能随版本失效
//! （2026-10-10 实测 `getcodetype` 等旧接口已 404）。passport 页面自动化
//! 也试过并放弃：登录页 DOM 与风控行为随版本变，自动化链路脆弱。
//! 最终方案把「怎么登录」还给用户：打开 KukuAI **主站**，用户用手机验证码 /
//! 扫码正常登录（官方页面自己处理风控），壳侧从 WebView2 的 Cookie 存储读
//! 完整会话交回。
//!
//! ── 流程 ──────────────────────────────────────────────────
//!   1. 适配器 `build_login_url` 返回主站地址 `https://kuku.baidu.com/`；
//!   2. 壳侧打开内嵌窗口（可见），用户在其中完成登录；
//!   3. 壳侧每 4 秒读一次 WebView2 Cookie 存储（独立线程 + 超时，Windows 上
//!      同步读会死锁）：出现 `BDUSS` 即登录成功，把**全部百度域 Cookie**
//!      （BDUSS / STOKEN / BAIDUID / BAIDUID_BFESS …，含 HttpOnly）
//!      拼成 Cookie 头 `POST /api/session/login/kuku/complete`
//!      （见 `desktop-tauri/src/login.rs` 的 `submit_kuku_login`）；
//!   4. 后端 `complete_login` 用完整会话打 `userreport` 复核并取 `uk`，
//!      落账号（见 `core::login::finish_kuku_login`）。
//!
//! ── 为什么必须交回**完整**会话（2026-10-07 浏览器对照实测）────
//! `userreport` 校验完整浏览器会话：只带 BDUSS/STOKEN/gfprotpl 会被判
//! 「未登录」（errno=-6），补上 BAIDUID / BAIDUID_BFESS 后立即可用。
//! 主站方案天然满足（首访即下发 BAIDUID，登录后 BDUSS 一并写入）。
//!
//! ── 安全边界 ───────────────────────────────────────────────
//! 收尾路由**免鉴权**（调用方是壳侧进程的登录窗口，不带 API Key），安全性由
//! 登录任务的一次性 state 承担（与 raccoon 回调同口径）。cookie 走本机
//! loopback 请求体，一次性传输、任务结束后即作废。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use crate::server::errors::GatewayError;

use super::credentials::{KukuCredentials, enrich_identity, parse_cookie_parts};

/// 生成登录任务 state（不可预测的随机串；与 `random_gid` 同源）。
pub fn new_login_state() -> String {
    random_hex(32)
}

/// KukuAI 登录页地址：**主站**（用户像平时用网页版一样登录）。
///
/// ── 为什么是主站而不是 passport 登录页（2026-10-07 最终方案）────
/// 早期试过两条路，都被现实否掉：
///   1. `passport.baidu.com/v2/?login&tpl=pp`：没有短信 tab，脚本扑空；
///   2. `passPassport/passApi/html/loginMerge.html`（客户端同款）：能登录，
///      但要在页面里自动填号/发码/填码 —— 页面 DOM 与风控行为随时会变，
///      自动化链路脆弱（脚本注入时机、图形验证码、提交时机都踩过坑）。
/// 主站方案把「怎么登录」还给用户自己：打开 `kuku.baidu.com`，用户用
/// 手机验证码 / 扫码（官方页面自己处理风控），登录成功后壳侧从 WebView2
/// 的 Cookie 存储读**完整会话**交回（含 HttpOnly 的 BDUSS / BAIDUID_BFESS，
/// 见 `desktop-tauri/src/login.rs` 的 `submit_kuku_login`）。
/// 主站对首访即下发 BAIDUID/BAIDUID_BFESS（实测），登录后 BDUSS 一并写入，
/// 因此交回的会话是完整的 —— 这正是 userreport 类接口要求的形态。
pub fn build_login_url(_state: &str) -> String {
    "https://kuku.baidu.com/".to_string()
}

/// 复核结果：凭证 + 可选的「未通过复核」警告（登录收尾把警告透传给前端）。
pub type CompleteOutcome = (KukuCredentials, Option<String>);

/// 从壳侧交回的 Cookie 串里提取登录态，并用 kuku 的 userreport 复核。
///
/// 壳侧交回的是登录窗口的**完整百度域 Cookie**（BDUSS / STOKEN / BAIDUID /
/// BAIDUID_BFESS …）：userreport 校验完整会话，只带前三个可能被判「未登录」
/// （2026-10-07 浏览器对照实测，见 `KukuCredentials::extras` 的说明）。
///
/// ── 复核失败**不阻断**登录，但**带警告**────────────────────
/// 复核（`enrich_identity`）内部会先换发 genflowpro 作用域 STOKEN
/// （`engine.rs`，需要本机客户端引擎 + Cookie 里的 PTOKEN）。走到警告
/// 分支的情形：没找到客户端引擎 / Cookie 缺 PTOKEN 换不了，且原凭证的
/// STOKEN 又过不了业务校验。用户在窗口里刚登录成功，凭证本身是有效的
/// （BDUSS 有效），因此失败时**仍返回凭证**（uid 为空，账号层用 cookie
/// 尾兜底 id）——转发与模型刷新会在需要时重新拿三件套并再次尝试换发
/// （`session::refresh_triple` 的自愈路径）；同时返回一条警告，由登录
/// 收尾透传前端告知补救办法。
pub async fn complete_login(cookie: &str) -> Result<CompleteOutcome, GatewayError> {
    let (bduss, stoken, extras) = parse_cookie_parts(cookie);
    if bduss.len() < 8 {
        return Err(GatewayError::with_status(
            400,
            "登录窗口没有捕获到有效的 BDUSS（百度通行证登录态），请重新发起登录",
        ));
    }
    let credentials = KukuCredentials {
        bduss,
        stoken,
        extras,
        uid: String::new(),
        nickname: String::new(),
    };
    match enrich_identity(&credentials, None, true).await {
        Ok(enriched) => Ok((enriched, None)),
        Err(error) => {
                crate::server::logging::log(
                    "[Login]",
                    &format!(
                        "⚠️ KukuAI 登录态已交回但复核未通过（{}）；先按有效凭证落库，\
                         uid 留空待后续刷新补齐（复核包含换发业务会话令牌，\
                         需要本机客户端引擎与 Cookie 里的 PTOKEN）",
                        error.message
                    ),
                );
            Ok((
                credentials,
                Some(
                    "换发 KukuAI 业务会话令牌失败（未找到库库AI 客户端引擎，或登录 \
                     Cookie 里缺 PTOKEN）：账号已添加，但模型刷新可能报「未登录」。\
                     请安装 / 修复库库AI 客户端后重新添加，或重新粘贴完整 Cookie"
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
