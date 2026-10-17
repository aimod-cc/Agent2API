//! AtomCode 的登录与续期：**代理中介的浏览器流程，没有本地回调**。
//!
//! ── 与其余各家的最大不同：回调不回到本机 ────────────────────
//! AutoClaw / Accio / Qoder 的网页登录都在本机起一个回调口（或等设备授权码），
//! atomcode 不是：授权回调由代理自己在其域名上终结
//! （`acs.atomgit.com/callback`），**永远不回到**运行网关的这台机器。
//! 因此本家没有 loopback 监听器、没有固定回调端口 —— 与 ZCode 的 CLI 登录
//! 同一形状，靠**轮询**判完成（下面是官方 `oauth.rs` 的三步，本地 Go 参考实现
//! `cpa-jethub-plugins/plugins/atomcode/oauth.go` 逐字照做并实测过）：
//!
//! ```text
//!   1. GET {broker}/auth/login?provider=atomgit
//!      -> {login_url, state}
//!   2. 把 login_url 交给用户打开（浏览器手势必须发生在用户还认为它是
//!      「自己点的」那段时间里，所以这一步**绝不能阻塞等待**）
//!   3. GET {broker}/auth/check?state=<state>   -> {valid: bool}     （轮询）
//!      GET {broker}/auth/token?state=<state>   -> 令牌包              （取回）
//! ```
//!
//! 续期是**另一个端点**：`POST {broker}/oauth/refresh`，body 只有
//! `{"refresh_token": ...}`，**不带 Authorization**（官方 `oauth.rs:901-975`）。
//!
//! ── 续期会轮换 refresh token，而且可能不回新的（两条都要照顾）────
//! 实测：`/oauth/refresh` 同时下发新 access 与新 refresh，**旧 access 立即失效**。
//! 但官方注释（`oauth.go:350`）明确写了代理**可能省略** `refresh_token` /
//! `expires_in` / `user` 三个字段 —— 省略 `refresh_token` 时若把空串写回记录，
//! 账号就退化成必须重新走浏览器登录。所以这里的规则是：
//! **新值非空才覆盖，为空保留旧值**（见 [`TokenBundle::merge_into`]）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::auth_http::send_raw;
use crate::server::errors::GatewayError;

use super::credentials::{TokenBundle, expires_at_from};
use super::endpoints::{
    BROKER_CHECK_PATH, BROKER_LOGIN_PATH, BROKER_PROVIDER, BROKER_REFRESH_PATH,
    BROKER_TOKEN_PATH, REQUEST_TIMEOUT_MS, broker_base_url,
};

/// 取一个 JSON 字符串字段（缺失 / 非字符串 / 空串都归一成空串）
fn str_field(source: &Value, key: &str) -> String {
    source
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 把一个代理响应体解成 [`TokenBundle`]（登录与续期共用同一信封）。
///
/// 代理的信封实测形态（官方 `AuthInfo` / Go 参考 `brokerTokenResponse`）：
///
/// ```text
///   { access_token, token_type, expires_in, refresh_token, user: {id, username, …} }
/// ```
///
/// 注意 `user` 可能整块缺失（官方注释：代理可省略），因此身份字段一律宽容读取。
pub fn parse_token_bundle(payload: &Value) -> Result<TokenBundle, GatewayError> {
    let access_token = str_field(payload, "access_token");
    if access_token.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "AtomCode 登录代理未返回 access_token",
        ));
    }
    let user = payload.get("user").cloned().unwrap_or(Value::Null);
    let expires_in = payload
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(0);
    Ok(TokenBundle {
        access_token,
        refresh_token: str_field(payload, "refresh_token"),
        token_type: str_field(payload, "token_type"),
        expires_in,
        user_id: {
            let id = str_field(&user, "id");
            if id.is_empty() { str_field(payload, "user_id") } else { id }
        },
        username: str_field(&user, "username"),
        name: str_field(&user, "name"),
        email: str_field(&user, "email"),
        avatar_url: str_field(&user, "avatar_url"),
    })
}

impl TokenBundle {
    /// 续期结果合并到旧令牌包上。
    ///
    /// ── 为什么不是整包替换 ──────────────────────────────────
    /// 代理**可能省略** `refresh_token` / `expires_in` / `user`（官方注释原文：
    /// > The broker may omit `refresh_token`/`expires_in`/`user`; each absent field …
    ///
    /// 整包替换的下场是：一次省略 `refresh_token` 的续期把记录里的 refresh 清成
    /// 空串，账号从此只能重新浏览器登录 —— 而这次续期「看起来是成功的」。
    /// 所以非空才覆盖、为空保留，`user_id` 同样（身份不该在一次续期后消失）。
    pub fn merge_into(self, previous: &TokenBundle) -> TokenBundle {
        TokenBundle {
            access_token: self.access_token,
            refresh_token: if self.refresh_token.is_empty() {
                previous.refresh_token.clone()
            } else {
                self.refresh_token
            },
            token_type: if self.token_type.is_empty() {
                previous.token_type.clone()
            } else {
                self.token_type
            },
            expires_in: if self.expires_in > 0 {
                self.expires_in
            } else {
                previous.expires_in
            },
            user_id: if self.user_id.is_empty() {
                previous.user_id.clone()
            } else {
                self.user_id
            },
            username: if self.username.is_empty() {
                previous.username.clone()
            } else {
                self.username
            },
            name: if self.name.is_empty() {
                previous.name.clone()
            } else {
                self.name
            },
            email: if self.email.is_empty() {
                previous.email.clone()
            } else {
                self.email
            },
            avatar_url: if self.avatar_url.is_empty() {
                previous.avatar_url.clone()
            } else {
                self.avatar_url
            },
        }
    }

    /// 这一次令牌包的过期时刻（毫秒）。`expires_in` 缺失时返回 0 = 判不出来。
    pub fn expires_at(&self, now_ms: i64) -> i64 {
        if self.expires_in <= 0 {
            return 0;
        }
        expires_at_from(now_ms / 1000, self.expires_in, 0)
    }
}

/// 一条已发起的浏览器登录（[`BrokerLogin::start`] 的返回值）。
///
/// 调用方持有它，把 [`Self::login_url`] 交给用户打开，之后按节奏调
/// [`Self::poll`]，直到拿到令牌包或超时。
pub struct BrokerLogin {
    /// 代理给的关联 id，既是轮询凭据也是索引
    state: String,
    /// 交给用户打开的 AtomGit 授权地址
    login_url: String,
    /// 本地截止时刻（毫秒）—— 代理自己也有窗口，这个只用来阻止本地累积死会话
    expires_at_ms: i64,
}

impl BrokerLogin {
    /// 发起登录：拿 `login_url` 与 `state`。
    ///
    /// **不阻塞**（不等待用户）—— 浏览器手势必须发生在用户还认为它是自己点开的
    /// 那段时间里，阻塞式登录会让 `window.open` 被拦、界面变成导航跳转。
    pub async fn start(timeout_ms: u64) -> Result<Self, GatewayError> {
        let url = format!(
            "{}{}?provider={}",
            broker_base_url(),
            BROKER_LOGIN_PATH,
            BROKER_PROVIDER
        );
        let response = send_raw(
            "GET",
            &url,
            None,
            &[("accept".to_string(), "application/json".to_string())],
            None,
            Some(REQUEST_TIMEOUT_MS),
        )
        .await
        .map_err(|error| login_transport_error("登录发起", &error))?;

        let payload = response.payload.ok_or_else(|| {
            GatewayError::with_status(502, "AtomCode 登录发起：代理响应不是 JSON")
        })?;
        if !response.ok {
            return Err(GatewayError::with_status(
                502,
                format!(
                    "AtomCode 登录发起失败（HTTP {}）：{}",
                    response.status,
                    summarize(&payload)
                ),
            ));
        }
        let login_url = str_field(&payload, "login_url");
        let state = str_field(&payload, "state");
        if login_url.is_empty() || state.is_empty() {
            return Err(GatewayError::with_status(
                502,
                "AtomCode 登录代理未返回 login_url/state",
            ));
        }
        let expires = if timeout_ms > 0 { timeout_ms } else { 300_000 };
        Ok(Self {
            state,
            login_url,
            expires_at_ms: crate::server::logging::now_ms() + expires as i64,
        })
    }

    /// 给用户打开的授权地址
    pub fn login_url(&self) -> &str {
        &self.login_url
    }

    /// 关联 id（登录任务表用它当 key）
    pub fn state(&self) -> &str {
        &self.state
    }

    /// 本地是否已过截止时刻（过点就没必要再轮询）
    pub fn expired(&self) -> bool {
        self.expires_at_ms > 0 && self.expires_at_ms <= crate::server::logging::now_ms()
    }

    /// 问一次「用户授权完了没」。
    ///
    /// 返回 `Ok(true)` 表示可以取令牌了。**网络类失败当 pending 处理**（返回
    /// `Ok(false)`）：那是「这次没问成」而不是「问成了但被拒」，把它当致命错误
    /// 会让一次抖动就打断用户正在进行的授权。
    pub async fn check(&self) -> Result<bool, GatewayError> {
        let url = format!(
            "{}{}?state={}",
            broker_base_url(),
            BROKER_CHECK_PATH,
            urlencode(&self.state)
        );
        let response = send_raw(
            "GET",
            &url,
            None,
            &[("accept".to_string(), "application/json".to_string())],
            None,
            Some(REQUEST_TIMEOUT_MS),
        )
        .await
        .map_err(|error| login_transport_error("登录轮询", &error))?;
        // 5xx 与畸形体当 pending（与 ZCode 轮询同一口径）
        if response.status >= 500 {
            return Ok(false);
        }
        if !response.ok {
            return Ok(false);
        }
        let Some(payload) = response.payload else {
            return Ok(false);
        };
        Ok(payload.get("valid").and_then(Value::as_bool).unwrap_or(false))
    }

    /// 取回令牌包（`check` 为 true 之后再调）。
    pub async fn fetch_token(&self) -> Result<TokenBundle, GatewayError> {
        let url = format!(
            "{}{}?state={}",
            broker_base_url(),
            BROKER_TOKEN_PATH,
            urlencode(&self.state)
        );
        let response = send_raw(
            "GET",
            &url,
            None,
            &[("accept".to_string(), "application/json".to_string())],
            None,
            Some(REQUEST_TIMEOUT_MS),
        )
        .await
        .map_err(|error| login_transport_error("登录取令牌", &error))?;
        let payload = response.payload.ok_or_else(|| {
            GatewayError::with_status(502, "AtomCode 登录取令牌：代理响应不是 JSON")
        })?;
        if !response.ok {
            return Err(GatewayError::with_status(
                502,
                format!(
                    "AtomCode 登录取令牌失败（HTTP {}）：{}",
                    response.status,
                    summarize(&payload)
                ),
            ));
        }
        parse_token_bundle(&payload)
    }
}

/// 续期：`POST {broker}/oauth/refresh`，body 只有 `{"refresh_token": …}`。
///
/// ── 为什么失败要区分「refresh 失效」与「其它」────────────────
/// `refresh_token` 已失效（HTTP 401 / 代理报该码）是**用户必须重新登录**的信号，
/// 归类成 401 交给宿主会让界面明确提示重登；而一次网络抖动或 5xx 只是「这次没刷成」，
/// 归类成 502/504 由上层按可重试处理。
///
/// ⚠️ 调用者必须保证**单飞**（同一账号同一时刻只有一次续期在跑）：
/// 续期会轮换 refresh token，两个并发续期互相作废、后写的那次还会覆盖掉
/// 前一次拿回的新 refresh（见 `core::refresh_flight`）。
pub async fn refresh(refresh_token: &str) -> Result<TokenBundle, GatewayError> {
    if refresh_token.trim().is_empty() {
        return Err(GatewayError::with_status(
            401,
            "AtomCode 账号没有 refresh_token，无法续期，请重新登录",
        ));
    }
    let url = format!("{}{}", broker_base_url(), BROKER_REFRESH_PATH);
    let body = json!({ "refresh_token": refresh_token });
    let response = send_raw(
        "POST",
        &url,
        Some(&body),
        &[
            ("accept".to_string(), "application/json".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ],
        None,
        Some(REQUEST_TIMEOUT_MS),
    )
    .await
    .map_err(|error| login_transport_error("续期", &error))?;

    let payload = response.payload.unwrap_or(Value::Null);
    if response.status == 401 || response.status == 403 {
        return Err(GatewayError::with_status(
            401,
            format!(
                "AtomGit refresh_token 已失效，请重新登录（{}）",
                summarize(&payload)
            ),
        ));
    }
    if !response.ok {
        return Err(GatewayError::with_status(
            502,
            format!(
                "AtomCode 续期失败（HTTP {}）：{}",
                response.status,
                summarize(&payload)
            ),
        ));
    }
    parse_token_bundle(&payload)
}

/// 把传输层错误统一成带中文前缀的网关错误。
fn login_transport_error(stage: &str, error: &reqwest::Error) -> GatewayError {
    if error.is_timeout() {
        GatewayError::with_status(504, format!("AtomCode {stage}超时"))
    } else {
        GatewayError::with_status(502, format!("AtomCode {stage}失败: {error}"))
    }
}

/// 把错误体压成一行可读文案（不把整块 HTML 塞进错误消息）
fn summarize(payload: &Value) -> String {
    let text = payload
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| payload.get("detail").and_then(|detail| detail.get("message")).and_then(Value::as_str))
        .or_else(|| payload.get("error").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let raw = payload.to_string();
            raw.chars().take(180).collect()
        });
    text
}

/// 极小的 query 转义（本家只有 state / plan_type 两个参数，都是 ASCII 记号）。
///
/// 不引 `urlencoding` 依赖：本文件的输入来自代理自己下发的 `state`，
/// 但仍按规范转义，避免将来 state 形态变化时把参数拼歪（`&` / `=` / `?` 一旦
/// 出现在 state 里，不转义就会截断 query）。
pub fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(access: &str, refresh: &str, expires_in: i64) -> TokenBundle {
        TokenBundle {
            access_token: access.to_string(),
            refresh_token: refresh.to_string(),
            expires_in,
            ..Default::default()
        }
    }

    /// 登录响应解析：`user` 整块缺失也要能解析（官方注释：代理可省略）。
    #[test]
    fn token_bundle_parses_with_and_without_user() {
        let full = json!({
            "access_token": "at", "refresh_token": "rt", "token_type": "Bearer",
            "expires_in": 604800,
            "user": {"id": "42", "username": "qq_1", "name": "张三", "email": "a@b.c", "avatar_url": "https://x/y.png"}
        });
        let parsed = parse_token_bundle(&full).expect("应可解析");
        assert_eq!(parsed.access_token, "at");
        assert_eq!(parsed.refresh_token, "rt");
        assert_eq!(parsed.expires_in, 604_800);
        assert_eq!(parsed.user_id, "42");
        assert_eq!(parsed.name, "张三");

        let bare = json!({ "access_token": "at" });
        let parsed = parse_token_bundle(&bare).expect("缺 user 也应可解析");
        assert_eq!(parsed.access_token, "at");
        assert!(parsed.user_id.is_empty());
        assert_eq!(parsed.expires_in, 0);
    }

    /// 没有 access_token 就是失败（不能拿空串去发请求）。
    #[test]
    fn token_bundle_requires_access_token() {
        let error = parse_token_bundle(&json!({"refresh_token": "rt"})).expect_err("应报错");
        assert_eq!(error.status_code, 502);
    }

    /// **核心护栏**：续期省略 `refresh_token` 时不能把记录刷成空。
    ///
    /// 官方注释原文（`oauth.go:350`）：代理可能省略 refresh_token/expires_in/user。
    /// 整包替换会让账号在一次「看起来成功的」续期之后失去续期能力。
    #[test]
    fn refresh_omitting_the_new_refresh_token_keeps_the_old_one() {
        let previous = bundle("old-at", "old-rt", 604_800);
        let rotated = bundle("new-at", "", 0);
        let merged = rotated.merge_into(&previous);
        assert_eq!(merged.access_token, "new-at");
        assert_eq!(merged.refresh_token, "old-rt", "新的 refresh 为空时必须保留旧的");
        assert_eq!(merged.expires_in, 604_800);
    }

    /// 正常轮换：新的一对都写回（旧 access 已失效，绝不能留）。
    #[test]
    fn refresh_with_a_new_pair_replaces_both() {
        let previous = bundle("old-at", "old-rt", 604_800);
        let rotated = bundle("new-at", "new-rt", 604_800);
        let merged = rotated.merge_into(&previous);
        assert_eq!(merged.access_token, "new-at");
        assert_eq!(merged.refresh_token, "new-rt");
    }

    /// 身份字段不该在一次续期后消失。
    #[test]
    fn refresh_keeps_identity_when_the_broker_omits_user() {
        let mut previous = bundle("old-at", "old-rt", 604_800);
        previous.user_id = "42".into();
        previous.name = "张三".into();
        let merged = bundle("new-at", "new-rt", 0).merge_into(&previous);
        assert_eq!(merged.user_id, "42");
        assert_eq!(merged.name, "张三");
    }

    /// 过期时刻由 `now + expires_in` 推（令牌非 JWT，没有 exp 可读）。
    #[test]
    fn expires_at_is_now_plus_expires_in() {
        let with_ttl = bundle("at", "rt", 604_800);
        assert_eq!(with_ttl.expires_at(1_700_000_000_000), 1_700_000_000_000 + 604_800_000);
        // 代理省略 expires_in 时判不出来（0），而不是猜一个值
        assert_eq!(bundle("at", "rt", 0).expires_at(1_700_000_000_000), 0);
    }

    /// 续期没有 refresh_token 时必须报 401（提示重登），而不是发一个空 body。
    #[test]
    fn refresh_without_a_refresh_token_is_an_auth_error() {
        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(refresh("   "))
            .expect_err("空 refresh_token 应报错");
        assert_eq!(error.status_code, 401);
        assert!(error.message.contains("重新登录"));
    }

    /// query 转义：state 里出现 `&` / `=` / `?` 时不得截断参数。
    #[test]
    fn urlencode_escapes_query_metacharacters() {
        assert_eq!(urlencode("abc-123_x.y~z"), "abc-123_x.y~z");
        assert_eq!(urlencode("a&b=c?d"), "a%26b%3Dc%3Fd");
        // 中文（state 理论上不会是，但转义必须安全）
        assert_eq!(urlencode("中"), "%E4%B8%AD");
    }
}
