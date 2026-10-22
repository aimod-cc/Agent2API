//! MiniMax Code 的网页登录：**OAuth 2.0 设备授权（RFC 8628）+ PKCE S256**。
//!
//! ── 实测（2026-10-21，`_recon/minimax-code-contract-*.md` §8）────
//! ```text
//! POST https://account.minimax.cn/oauth2/device/code
//!   client_id=mcode-public&scope=agent.default&audience=agent-backend
//!   &code_challenge=<S256>&code_challenge_method=S256
//! → {"device_code":"…","expires_in":300,"interval":3,"user_code":"K3CG-SPQZ",
//!    "verification_uri":"https://account.minimax.cn/oauth-authorize",
//!    "verification_uri_complete":"…?user_code=K3CG-SPQZ"}
//!
//! POST https://account.minimax.cn/oauth2/token
//!   client_id=mcode-public&grant_type=urn:ietf:params:oauth:grant-type:device_code
//!   &device_code=…&code_verifier=…
//! → 200 {access_token:"mmoat_…", refresh_token:"mmort_…", expires_in, scope}
//! → 400 {"error":"authorization_pending"}   还没授权（正常，继续等）
//! → 400 {"error":"slow_down"} / expired_token / access_denied
//! ```
//!
//! ── 为什么走设备码而不是「打开网页抓回调」─────────────────────
//! 客户端的登录就是这条 grant（`auth-state.json` 的 `authorization.leaseId` /
//! `verificationUri` 就是同一套）。它有两个好处：
//!   1. **不需要本地回调端口**：授权在用户的浏览器里完成，我们只轮询 ——
//!      于是桌面壳的登录窗口不必放行任何 `127.0.0.1` 导航（`login.rs` 的
//!      `allowed_hosts` 陷阱在这里天然绕开）；
//!   2. **PKCE**：设备码即使泄漏，没有本进程里的 verifier 也换不成令牌。
//!      verifier 只活在本结构体内，不进前端、不进日志、不落盘。
//!
//! ── 两个地区：只有授权服务器域名不同（2026-10-21 实测）──────────
//! ```text
//!   国内  POST https://account.minimax.cn/oauth2/device/code   → 授权页在 account.minimax.cn
//!   国际  POST https://account.minimax.io/oauth2/device/code   → 授权页在 account.minimax.io
//! ```
//! `client_id` / `scope` / `audience` 两地**完全相同**（`@mavis/oauth-core/dist/
//! contracts.js` 里是全局唯一常量）；授权页地址不自己拼，原样透传上游给的
//! `verification_uri_complete` —— 改写它会让用户看到的域与页面证书不一致。
//!
//! ── `slow_down` 的处理 ──────────────────────────────────────
//! 上游要求客户端按 `interval`（实测 3 秒）轮询，快了回 `slow_down`。
//! 本实现固定 3 秒一拍（`login.rs` 的循环），遇到 `slow_down` 当作「还没好」
//! 而不是失败 —— 一次抢跑不该让用户重来。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::cell::RefCell;

use serde_json::Value;

use crate::server::errors::GatewayError;

use super::credentials::decode_jwt_claims;
use super::endpoints;
use super::region::Region;

/// 一次设备授权会话。
pub struct DeviceLogin {
    /// 登录任务表里的 state（前端 `/wait` 用它查结果）
    pub state: String,
    /// 给用户打开的授权页（含 user_code，**不含 verifier**）
    pub auth_url: String,
    /// 用户在页面上要输入的设备码（界面上照抄给用户看）
    pub user_code: String,
    /// 这个会话属于哪个地区（换令牌 / 落账号都按它走）
    pub region: Region,
    device_code: String,
    code_verifier: String,
    /// 设备码到期时刻（毫秒）
    pub expires_at_ms: i64,
}

/// 换到的令牌。
#[derive(Clone, Debug)]
pub struct Credentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub user_id: String,
}

/// 轮询结果。
pub enum PollOutcome {
    /// 用户还没授权
    Pending,
    /// 上游要求放慢（同样继续等）
    SlowDown,
    /// 拿到令牌
    Ready(Box<Credentials>),
}

impl DeviceLogin {
    /// 申请一个设备码（`region` 决定问哪台授权服务器）。
    pub async fn start(region: Region) -> Result<Self, GatewayError> {
        let verifier = random_token();
        let challenge = pkce_challenge(&verifier);
        let body = format!(
            "client_id={client}&scope={scope}&audience={audience}\
             &code_challenge={challenge}&code_challenge_method=S256",
            client = endpoints::OAUTH_CLIENT_ID,
            scope = encode_space(endpoints::OAUTH_SCOPE),
            audience = endpoints::OAUTH_AUDIENCE,
            challenge = challenge,
        );
        let url = endpoints::auth_url(region, endpoints::DEVICE_CODE_PATH);
        let text = post_form(&url, &body, "申请设备码").await?;
        let payload: Value = serde_json::from_str(&text)
            .map_err(|_| GatewayError::with_status(502i32, "设备码响应不是合法 JSON"))?;
        let device_code = payload
            .get("device_code")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| GatewayError::with_status(502i32, "设备码响应里没有 device_code"))?
            .to_string();
        let user_code = payload
            .get("user_code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // 优先用 `verification_uri_complete`（带 code，用户点开就填好）；
        // 上游没给时用基础地址 + 手动拼 code
        let auth_url = payload
            .get("verification_uri_complete")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                let base = payload.get("verification_uri").and_then(Value::as_str)?;
                if user_code.is_empty() {
                    Some(base.to_string())
                } else {
                    Some(format!("{base}?user_code={user_code}"))
                }
            })
            .ok_or_else(|| GatewayError::with_status(502i32, "设备码响应里没有授权页地址"))?;
        let expires_in = payload
            .get("expires_in")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .unwrap_or(300);
        let state = random_state();
        // 登记「码 ↔ 任务」：取消入口只有 state（见 `mark_canceled`）
        register_code_for_state(&device_code, &state);
        Ok(Self {
            state,
            auth_url,
            user_code,
            region,
            device_code,
            code_verifier: verifier,
            expires_at_ms: crate::server::logging::now_ms() + expires_in * 1000,
        })
    }

    /// 问一次「用户授权了吗」。
    pub async fn poll(&self) -> Result<PollOutcome, GatewayError> {
        let outcome = self.poll_once().await;
        // 上游明确说这个码不能再用（过期 / 被拒绝）⇒ 本地也立刻作废：否则后台
        // 循环还会拿它继续打（一拍 3 秒打满 5 分钟），而结果注定是同一个错误。
        if let Err(error) = &outcome {
            if is_terminal_device_error(error.code.as_deref()) {
                self.mark_spent();
            }
        }
        outcome
    }

    /// 一旦用户取消或被上游判死，就把码划掉 —— 后续 `poll` 立刻返回「已作废」，
    /// 不再碰上游。
    pub fn mark_spent(&self) {
        SPENT.with(|spent| {
            spent.borrow_mut().insert(self.device_code.clone());
        });
    }

    /// 这枚码是否已被作废（取消 / 上游判死）。
    fn is_spent(&self) -> bool {
        SPENT.with(|spent| spent.borrow().contains(&self.device_code))
    }

    async fn poll_once(&self) -> Result<PollOutcome, GatewayError> {
        if self.is_spent() {
            return Err(GatewayError::with_status(
                400i32,
                "这次登录已取消，请重新发起",
            ));
        }
        if self.expires_at_ms > 0 && crate::server::logging::now_ms() >= self.expires_at_ms {
            return Err(GatewayError::with_status(400i32, "设备码已过期，请重新发起登录"));
        }
        let body = format!(
            "client_id={client}&grant_type={grant}&device_code={device}&code_verifier={verifier}",
            client = endpoints::OAUTH_CLIENT_ID,
            grant = encode_space(endpoints::DEVICE_CODE_GRANT),
            device = self.device_code,
            verifier = self.code_verifier,
        );
        let url = endpoints::auth_url(self.region, endpoints::TOKEN_PATH);
        let client = crate::server::core::egress::client_for_system_proxy();
        let response = client
            .post(&url)
            .timeout(std::time::Duration::from_millis(20_000))
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|err| {
                let mut error =
                    GatewayError::with_status(502i32, format!("设备授权轮询不可达：{err}"));
                // 调用方据此判断「网络抖一下，继续等」而不是「登录失败」
                error.code = Some("minimax_transport".to_string());
                error
            })?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

        if (200..300).contains(&status) {
            let access_token = payload
                .get("access_token")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayError::with_status(502i32, "登录响应里没有 access_token"))?
                .to_string();
            return Ok(PollOutcome::Ready(Box::new(build_credentials(&payload, access_token))));
        }

        let error_name = payload.get("error").and_then(Value::as_str).unwrap_or("");
        match error_name {
            "authorization_pending" => return Ok(PollOutcome::Pending),
            // `slow_down` 是「你问太快了」，不是失败 —— 一拍 3 秒本就合规，
            // 遇到它只说明上游那侧另有节流，继续等即可
            "slow_down" => return Ok(PollOutcome::SlowDown),
            _ => {}
        }
        let description = payload
            .get("error_description")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let message = match error_name {
            "expired_token" => "设备码已过期，请重新发起登录".to_string(),
            "access_denied" => "你在授权页取消了这次登录".to_string(),
            "" => format!("设备授权返回 HTTP {status}"),
            other => format!("设备授权失败：{other}{}", if description.is_empty() { String::new() } else { format!("（{description}）") }),
        };
        let mut error = GatewayError::with_status(if status == 401 { 401 } else { 400 }, message);
        // 把上游的错误标识带进 `code`：调用方（后台轮询 / cancel）据此判断
        // 「这枚设备码已经不能再用」，见 `is_terminal_device_error`。
        if !error_name.is_empty() {
            error.code = Some(error_name.to_string());
        }
        Err(error)
    }
}

/// 上游说「这枚设备码不能再用了吗」。
///
/// 只有明确判死的两种才算：`expired_token`（5 分钟到了）、`access_denied`
/// （用户在授权页点了拒绝）。其余（`authorization_pending` / `slow_down` 已在上层
/// 分开处理，`invalid_grant` 之类是协议噪音）一律**不**划掉 —— 误划的后果是
/// 用户明明马上要授权成功，却被我们判成取消。
fn is_terminal_device_error(code: Option<&str>) -> bool {
    matches!(code, Some("expired_token") | Some("access_denied"))
}

thread_local! {
    /// 已作废的设备码（本次进程内）。
    ///
    /// ── 为什么用 `thread_local` 而不是账号表 / `DeviceLogin` 字段 ─────
    /// 取消发生在**另一条调用链**上（`/api/session/login/cancel` → `LoginService::cancel`），
    /// 它手里只有 state、拿不到那个 `DeviceLogin` 实例；而设备码本身也不该落盘
    /// （5 分钟就作废，存下来只是一份过期垃圾）。取消与轮询跑在同一个 tokio
    /// worker 上时按线程记即可 —— 要守住的只有「**别再拿这枚码打上游**」。
    /// 条目量级 = 进程生命周期内的登录次数（几十），无需淘汰。
    static SPENT: RefCell<std::collections::HashSet<String>> = RefCell::new(std::collections::HashSet::new());

    /// 设备码 ↔ 登录任务 state 的双向映射（见 [`mark_canceled`]）。
    ///
    /// 取消入口只拿得到 state（那是给前端的任务标识），而判定作废要按
    /// **设备码**做（`poll` 手里只有设备码）。两个方向各存一份，取值都是 O(n)
    /// 查找、n = 进程内的登录次数。
    static STATE_OF_CODE: RefCell<std::collections::HashMap<String, String>> =
        RefCell::new(std::collections::HashMap::new());
}

/// 登记「这枚设备码属于哪个登录任务」。
///
/// 取消时不撤销上游的 `device_code`（协议里没有 revoke 端点，见模块头），
/// 但必须让本进程**不再用它**：`mark_canceled` 按 state 找到码、把它划掉，
/// 后台轮询下一拍就退出（不再每 3 秒白打一次注定失败的上游请求）。
fn register_code_for_state(device_code: &str, state: &str) {
    STATE_OF_CODE.with(|map| {
        map.borrow_mut()
            .insert(device_code.to_string(), state.to_string());
    });
}

/// 登录任务被取消：把它的设备码划掉（`LoginService::cancel` 的钩子）。
///
/// 只认本家的任务 —— 别的家 state 也在这张表里不存在，查不到就是空操作
/// （与 `drop_codearts_pending` 同一处置：不做前缀判断，查不到就什么都不做）。
pub fn mark_canceled(state: &str) {
    let code = STATE_OF_CODE.with(|map| {
        map.borrow()
            .iter()
            .find(|(_, value)| value.as_str() == state)
            .map(|(code, _)| code.clone())
    });
    if let Some(code) = code {
        SPENT.with(|spent| {
            spent.borrow_mut().insert(code);
        });
    }
}

/// 用刷新令牌换一对新令牌（**refresh_token 单次有效、每次轮换**）。
///
/// 调用方必须在拿到新令牌后立刻落盘：旧的刷新令牌在应答到达的瞬间就死了，
/// 丢一次就等于账号要用户重新登录。
///
/// `region` 必须是**账号所属地区**：国内账号的刷新令牌拿到 `account.minimax.io`
/// 去换，得到的是一个含义模糊的 `invalid_grant`（令牌不认识，而不是"过期"），
/// 用户会被引向「重新登录」而其实什么都没坏。
pub async fn refresh(region: Region, refresh_token: &str) -> Result<Credentials, GatewayError> {
    if refresh_token.trim().is_empty() {
        return Err(GatewayError::with_status(
            400i32,
            "没有可用于续期的 refresh token，请重新登录 MiniMax Code",
        ));
    }
    let body = format!(
        "grant_type=refresh_token&refresh_token={token}&client_id={client}\
         &scope={scope}&audience={audience}",
        token = refresh_token.trim(),
        client = endpoints::OAUTH_CLIENT_ID,
        scope = encode_space(endpoints::OAUTH_SCOPE),
        audience = endpoints::OAUTH_AUDIENCE,
    );
    let url = endpoints::auth_url(region, endpoints::TOKEN_PATH);
    let client = crate::server::core::egress::client_for_system_proxy();
    let response = client
        .post(&url)
        .timeout(std::time::Duration::from_millis(20_000))
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|err| GatewayError::with_status(502i32, format!("续期请求不可达：{err}")))?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        let error = payload.get("error").and_then(Value::as_str).unwrap_or("");
        // `invalid_grant` = 刷新令牌本身没了（已被轮换掉 / 被撤销）—— 这是
        // 永久失败，只有重新登录能救；其余（5xx / 网络）按临时失败处理
        if error == "invalid_grant" {
            // 为什么不只说「令牌失效」：本家的 refresh token **单次有效、每次
            // 轮换**，所以它最常见的失效原因不是过期，而是**同一个账号在别处
            // 续过一次**（本仓的调试脚本、另一台机器的面板、或官方客户端自己）。
            // 只写「已失效」会让用户以为是我们的 bug，或者去做无意义的等待；
            // 把「谁先续谁赢」这条机制说出来，用户才知道该去做什么。
            //
            // ⚠️ 文案是**纯文本**（前端按文本渲染，不解析 Markdown）：别写 `**强调**`，
            // 否则用户会看到一对原样的星号。
            return Err(GatewayError::with_status(
                401i32,
                format!(
                    "续期失败：刷新令牌已失效（invalid_grant），请重新登录 MiniMax Code {}。\
                     这通常不是账号问题 —— 本家的刷新令牌单次有效、每次轮换，\
                     同一个账号在别处（官方客户端 / 另一台机器的面板）续过一次，\
                     这里存的那一枚就会被作废，谁先续谁赢",
                    region.label()
                ),
            ));
        }
        return Err(GatewayError::with_status(
            if status == 401 { 401 } else { 502 },
            format!(
                "续期失败：上游返回 HTTP {status}{}",
                if error.is_empty() { String::new() } else { format!("（{error}）") }
            ),
        ));
    }
    let access_token = payload
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayError::with_status(502i32, "续期响应里没有 access_token"))?
        .to_string();
    Ok(build_credentials(&payload, access_token))
}

/// 从令牌响应造凭证（含 JWT 解析出的 `exp` / `user.id`）。
fn build_credentials(payload: &Value, access_token: String) -> Credentials {
    let refresh_token = payload
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let (jwt_expires, jwt_user) = decode_jwt_claims(&access_token);
    let expires_at = payload
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .map(|seconds| crate::server::logging::now_ms() + seconds * 1000)
        // `expires_in` 缺失时用 JWT 的 `exp`（更权威，实测两者一致）
        .unwrap_or(jwt_expires);
    Credentials {
        access_token,
        refresh_token,
        expires_at,
        user_id: jwt_user,
    }
}

/// 表单 POST → 响应文本（非 2xx 也算成功返回，由调用方按状态码判）。
async fn post_form(url: &str, body: &str, what: &str) -> Result<String, GatewayError> {
    let client = crate::server::core::egress::client_for_system_proxy();
    let response = client
        .post(url)
        .timeout(std::time::Duration::from_millis(20_000))
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body.to_string())
        .send()
        .await
        .map_err(|err| GatewayError::with_status(502i32, format!("{what}不可达：{err}")))?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(GatewayError::with_status(
            if status == 401 { 401 } else { 502 },
            format!(
                "{what}失败：HTTP {status}{}",
                if text.trim().is_empty() { String::new() } else { format!("：{text}") }
            ),
        ));
    }
    Ok(text)
}

/// 表单值里的空格编码成 `%20`（`scope` 可能多值；`+` 也合法但 `%20` 更稳）。
fn encode_space(value: &str) -> String {
    value.replace(' ', "%20")
}

/// PKCE 的 S256 challenge：`base64url(sha256(verifier))`，无填充。
fn pkce_challenge(verifier: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// 43 字符的 base64url 随机串（RFC 7636 要求 43–128 字符）。
fn random_token() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// 登录任务表里的 state（32 字节随机，够长到不可猜）。
fn random_state() -> String {
    random_token()
}

/// 用系统熵源填随机字节；取不到时退回「时间 + 地址熵」的组合。
///
/// 与 `qwenwork/oauth.rs` 同一手法（`getrandom::getrandom`，仓库已依赖）：
/// 不用 `unwrap` —— release 是 `panic=abort`，一次熵源失败不该让进程死掉；
/// 而这里的随机性只用于 PKCE verifier 与本地任务标识，退化路径
/// （纳秒时间戳 + 堆地址 + 计数器）仍远好于「固定值」。
fn fill_random(buf: &mut [u8]) {
    if getrandom::getrandom(buf).is_ok() {
        return;
    }
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let tick = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let seed = now ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for (index, slot) in buf.iter_mut().enumerate() {
        *slot = (seed >> ((index % 8) * 8)) as u8 ^ (index as u8).wrapping_mul(31);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pkce_challenge_is_base64url_sha256() {
        // RFC 7636 附录 B 的示例 verifier
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_token_is_long_enough_and_url_safe() {
        let token = random_token();
        // 32 字节 base64url 无填充 = 43 字符（RFC 7636 的下限）
        assert_eq!(token.chars().count(), 43);
        assert!(
            token
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
            "必须只用 base64url 字符集：{token}"
        );
        assert_ne!(token, random_token(), "两次生成必须不同");
    }

    #[test]
    fn credentials_prefer_expires_in_and_fall_back_to_jwt() {
        // 只给 expires_in
        let payload = json!({"access_token": "mmoat_opaque", "refresh_token": "mmort_x", "expires_in": 3600});
        let credentials = build_credentials(&payload, "mmoat_opaque".into());
        assert!(credentials.expires_at > crate::server::logging::now_ms());
        assert_eq!(credentials.refresh_token, "mmort_x");
        assert!(credentials.user_id.is_empty(), "不透明令牌解析不出 user.id");
    }

    #[test]
    fn credentials_read_jwt_claims_when_expires_in_is_absent() {
        let payload_jwt = "eyJleHAiOjE3MDAwMDAwMDAsInVzZXIiOnsiaWQiOiJ1NDIifX0";
        let token = format!("header.{payload_jwt}.sig");
        let credentials = build_credentials(&json!({"refresh_token": "mmort_y"}), token.clone());
        assert_eq!(credentials.access_token, token);
        assert_eq!(credentials.expires_at, 1_700_000_000_000);
        assert_eq!(credentials.user_id, "u42");
    }

    #[test]
    fn refresh_without_token_fails_locally() {
        // 空刷新令牌不该白打一次上游（那条早退分支根本不 await 网络）
        let error = futures_block(refresh(Region::Cn, "  ")).expect_err("空刷新令牌应被拒");
        assert!(error.to_string().contains("refresh token"), "实际：{error}");
        // 两个地区同一个早退分支（地区不该改变这条判断）
        assert!(futures_block(refresh(Region::Intl, "")).is_err());
    }

    /// 两个地区的授权服务器必须是**不同主机**，且国际版不是 `.cn` 的兄弟域。
    ///
    /// 这条钉的是「设备码打到哪个域」：实测国际版的校验在 `account.minimax.io`
    /// （`verification_uri` 也在该域），拼成 `account.minimaxi.com` 会 404。
    #[test]
    fn each_region_asks_its_own_auth_host() {
        let cn = endpoints::auth_url(Region::Cn, endpoints::DEVICE_CODE_PATH);
        let intl = endpoints::auth_url(Region::Intl, endpoints::DEVICE_CODE_PATH);
        assert_eq!(cn, "https://account.minimax.cn/oauth2/device/code");
        assert_eq!(intl, "https://account.minimax.io/oauth2/device/code");
        assert!(!intl.contains("minimax.cn"), "国际版不能打到国内授权服务器");
        // 换令牌 / 续期同域：地区参数是唯一的开关
        assert!(endpoints::auth_url(Region::Intl, endpoints::TOKEN_PATH)
            .starts_with("https://account.minimax.io/"));
    }

    /// **真机自检**（CI 上 ignored，开发机手动跑）：设备码这条链在真上游通不通。
    ///
    /// ```bash
    /// cargo test -p agent2api-server --lib -j 1 \
    ///   minimax::login::tests::obtains_a_real_device_code -- --ignored --nocapture
    /// ```
    ///
    /// 它回答的是单测回答不了的问题：**授权服务器此刻是否接受我们的
    /// client_id / scope / audience / PKCE 组合**。这条链一旦被上游收紧，
    /// 界面上表现为「点登录后一直等待」，而所有纯函数测试仍然全绿 ——
    /// 所以必须有一条真的打到 `account.minimax.cn` 的用例。
    ///
    /// 不碰用户账号、不换令牌：只申请一个设备码就扔掉（5 分钟后自己失效）。
    #[tokio::test]
    #[ignore = "真机自检：需要能访问 account.minimax.cn"]
    async fn obtains_a_real_device_code() {
        for region in Region::ALL {
            match DeviceLogin::start(region).await {
                Ok(flow) => {
                    println!(
                        "{} 设备码={:?} 授权页={} 到期={}",
                        region.label(),
                        flow.user_code,
                        flow.auth_url,
                        flow.expires_at_ms
                    );
                    assert!(!flow.user_code.is_empty(), "上游必须回 user_code");
                    assert!(
                        flow.auth_url.starts_with("https://"),
                        "授权页必须是 https：{}",
                        flow.auth_url
                    );
                    assert!(!flow.device_code.is_empty(), "device_code 不能为空");
                    // PKCE verifier 必须留在本进程（43 字符 base64url）
                    assert_eq!(flow.code_verifier.chars().count(), 43);
                }
                Err(error) => {
                    // 国际版可能有地区门（账号体系未验证），如实打印而不是让整条用例红
                    println!("{} 申请设备码失败：{}", region.label(), error.message);
                    assert_eq!(region, Region::Cn, "国内版必须能申请到设备码");
                }
            }
        }
    }

    /// 极小的「跑一个立即 ready 的 future」助手 —— 只为让上面那条同步断言
    /// 不必引 `#[tokio::test]`（那条路径根本不 await 网络）。
    fn futures_block<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Wake, Waker};
        struct Noop;
        impl Wake for Noop {
            fn wake(self: std::sync::Arc<Self>) {}
        }
        let waker = Waker::from(std::sync::Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut pinned = std::pin::pin!(future);
        match pinned.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("该分支不应等待网络"),
        }
    }

    /// ── 取消后不再打上游（2026-10-22）───────────────────────────
    /// 设备码协议没有 revoke 端点，所以「取消」在本地就是「把这枚码划掉」。
    /// 判据必须生效在**发起请求之前**：否则后台循环会每 3 秒拿一枚注定失败的
    /// 码打满 5 分钟，日志里还全是上游那侧的 `expired_token`。
    ///
    /// 这条用例不联网：构造函数直接拼 `DeviceLogin`，把到期时刻设成「还很久」，
    /// 唯一能让 `poll` 提前返回的就是「已作废」这一支。
    #[tokio::test]
    async fn a_canceled_code_fails_before_any_network_call() {
        let flow = DeviceLogin {
            state: "state-canceled".to_string(),
            auth_url: "https://example.invalid/authorize".to_string(),
            user_code: "ABCD-EFGH".to_string(),
            region: Region::Cn,
            device_code: "device-code-for-cancel-test".to_string(),
            code_verifier: random_token(),
            // 远未到期：排除「过期」那一支，确保命中的是「已作废」
            expires_at_ms: crate::server::logging::now_ms() + 600_000,
        };
        register_code_for_state(&flow.device_code, &flow.state);
        assert!(!flow.is_spent(), "前提：刚登记的码不该是作废状态");

        mark_canceled(&flow.state);
        assert!(flow.is_spent(), "取消后码必须作废");

        let outcome = flow.poll().await;
        let Err(error) = outcome else {
            panic!("已作废的码不该再向上游发起轮询");
        };
        assert!(
            error.message.contains("已取消"),
            "文案要点明是取消（不是过期 / 上游报错），实际：{}",
            error.message
        );
    }

    /// 判据本身：只有上游明确判死的那两种错误才允许划掉码。
    ///
    /// 误划的代价是「用户马上要授权成功，却被我们判成取消」—— 所以白名单式判断，
    /// 协议噪音（`invalid_grant` / 空串）一律不划。
    #[test]
    fn only_terminal_device_errors_are_treated_as_dead() {
        assert!(is_terminal_device_error(Some("expired_token")));
        assert!(is_terminal_device_error(Some("access_denied")));
        assert!(!is_terminal_device_error(Some("authorization_pending")));
        assert!(!is_terminal_device_error(Some("slow_down")));
        assert!(!is_terminal_device_error(Some("invalid_grant")));
        assert!(!is_terminal_device_error(None));
    }
}
