//! 桌面壳的面板会话（登录令牌的保存、注入与自动续期）。
//!
//! ── 这套东西解决什么 ────────────────────────────────────────
//! 桌面窗口加载 `tauri://localhost`，网关在 `127.0.0.1:{port}` —— 两者跨源，
//! 登录响应的 HttpOnly cookie 落不到网关的源上（就算落到了，窗口的页面也带不上）。
//! 所以桌面形态借用网页端「中转模式」同一条通道：登录/续期的响应体里带回
//! `accessToken` / `refreshToken`，之后每个管理请求带 `x-panel-token` 头。
//! 服务端 `access_candidates` 本来就把这条头通道排在 cookie 之后 —— 两种形态
//! 共用一套校验，这里只是把「谁来保管令牌」从 localStorage 换成了壳进程。
//!
//! ── 什么时候启用 ────────────────────────────────────────────
//! 只有**注册过管理员**之后才需要：那时 `/api/*` 才要求会话或 Key（见
//! `access::panel_auth_enabled`）。没注册时壳照旧只带第一把 Key，行为与改造前
//! 逐字一致 —— 这是「桌面当正常软件用」的默认形态，不能被登录打断。
//!
//! ── 登录流程 ────────────────────────────────────────────────
//! 已注册管理员时，窗口先加载 `login.html`（与网页端同一份页面），登录成功后
//! 页面把令牌交给壳（`panel_session_store` 命令）并跳转到主界面。壳把令牌
//! 加密后写进桌面设置（`AppSettings::panel_session`），重启后直接复用。
//!
//! ── 自动续期 ────────────────────────────────────────────────
//! access 只有 2 小时。`authorized_call` 收到 401 且响应体是
//! `panel_login_required` 时，先用 refresh 换一组新令牌、重试一次；续期也失败
//! 才把错误抛给界面（界面据此提示「登录已过期」，并把窗口送回登录页）。
//!
//! ── 为什么令牌要加密落盘 ────────────────────────────────────
//! 它是一对管理面凭据。`AppSettings` 整份是明文 JSON 进 `kv` 表，直接放进去等于
//! 「拷走数据库就接管面板」。这里用账号域同一把密钥（`account.key`）封装：
//! 能读那把钥匙的人本来就能读全部账号，信任边界不扩大。

use std::sync::Mutex;

use serde_json::Value;

use crate::gateway;
use crate::settings::{self, PanelSession};

/// 进程内的令牌缓存（避免每个请求都解密一次）。
///
/// `None` = 还没读过 / 已清空；两者都要回落到磁盘再读。`Some` 里的空串表示
/// 「确认没有令牌」，与「没读过」区分开 —— 否则每个请求都会去尝试解密一次。
static CACHE: Mutex<Option<TokenPair>> = Mutex::new(None);

#[derive(Clone, Debug, Default)]
pub struct TokenPair {
    pub access: String,
    pub refresh: String,
}

impl TokenPair {
    fn is_empty(&self) -> bool {
        self.access.is_empty() && self.refresh.is_empty()
    }
}

/// 读令牌：缓存 → 桌面设置（解密）→ 空。
///
/// 解不开（密钥换过 / 文件丢了）按「没有令牌」处理，不报错 —— 用户重新登录一次
/// 就能自愈，而在这里报错会让每个管理请求都弹一条看不懂的提示。
fn load_tokens() -> TokenPair {
    if let Ok(guard) = CACHE.lock() {
        if let Some(pair) = guard.as_ref() {
            return pair.clone();
        }
    }
    let stored = settings::load().panel_session;
    let pair = TokenPair {
        access: open(&stored.access),
        refresh: open(&stored.refresh),
    };
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(pair.clone());
    }
    pair
}

/// 写入令牌（登录成功后由界面交来）：加密落盘 + 更新缓存。
pub fn store(access: &str, refresh: &str) -> Result<(), String> {
    let pair = TokenPair {
        access: access.to_string(),
        refresh: refresh.to_string(),
    };
    let mut current = settings::load();
    current.panel_session = PanelSession {
        access: seal(access),
        refresh: seal(refresh),
    };
    settings::save(&current)?;
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(pair);
    }
    Ok(())
}

/// 清空令牌（登出 / 续期彻底失败 / 改了密码后被服务端撤销）。
pub fn clear() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(TokenPair::default());
    }
    let mut current = settings::load();
    if !current.panel_session.is_present() {
        return;
    }
    current.panel_session = PanelSession::default();
    let _ = settings::save(&current);
}

/// 是否有可尝试的令牌（不判断有效性 —— 那是服务端的事）。
pub fn has_tokens() -> bool {
    !load_tokens().is_empty()
}

/// 给请求加上会话令牌头（有令牌时）。`x-panel-token` 与 `Authorization` 一起发：
/// 后者是标准头，连自定义头都剥的中转也放它过去（与 web_shim 同口径）。
pub fn attach(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let pair = load_tokens();
    let mut builder = builder;
    if !pair.access.is_empty() {
        builder = builder
            .header("x-panel-token", &pair.access)
            .header("Authorization", format!("Bearer {}", pair.access));
    }
    if !pair.refresh.is_empty() {
        builder = builder.header("x-panel-refresh", &pair.refresh);
    }
    builder
}

/// 用 refresh 换一组新令牌。成功返回 true。
///
/// 直接打 `/api/panel/refresh?auth-mode=body`：query 标记让服务端把新令牌放进
/// 响应体（桌面窗口没有 cookie 通道，只能走 body）。新令牌**必须**落盘 ——
/// 服务端的轮换会让旧 refresh 立即作废，不保存就等于下次续期必然失败。
pub async fn refresh() -> bool {
    let pair = load_tokens();
    if pair.refresh.is_empty() {
        return false;
    }
    let url = format!(
        "http://127.0.0.1:{}/api/panel/refresh?auth-mode=body",
        gateway::proxy_port()
    );
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(gateway::REQUEST_TIMEOUT_MS))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    let response = client
        .post(&url)
        .header("x-panel-refresh", &pair.refresh)
        .header("x-panel-auth-mode", "body")
        .send()
        .await;
    let Ok(response) = response else { return false };
    if !response.status().is_success() {
        // 续期失败 = 会话链真没了（过期 / 重放 / 被改密码撤销）：清掉死令牌，
        // 界面下一次 `panel_session_status` 会发现「没有会话」并回到登录页
        clear();
        return false;
    }
    let Ok(text) = response.text().await else { return false };
    let Ok(payload) = serde_json::from_str::<Value>(&text) else { return false };
    let data = payload.get("data").unwrap_or(&Value::Null);
    let access = data.get("accessToken").and_then(Value::as_str).unwrap_or("");
    let refresh = data.get("refreshToken").and_then(Value::as_str).unwrap_or("");
    if access.is_empty() || refresh.is_empty() {
        return false;
    }
    store(access, refresh).is_ok()
}

// ── 加解密（账号域密钥；失败一律当作「没有」）────────────────

fn seal(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    agent2api_server::server::core::crypto::seal(
        plain.as_bytes(),
        agent2api_server::server::core::crypto::SecretScope::Accounts,
    )
    .unwrap_or_default()
}

fn open(stored: &str) -> String {
    if stored.is_empty() {
        return String::new();
    }
    agent2api_server::server::core::crypto::open(
        stored,
        agent2api_server::server::core::crypto::SecretScope::Accounts,
    )
    .ok()
    .and_then(|bytes| String::from_utf8(bytes).ok())
    .unwrap_or_default()
}
