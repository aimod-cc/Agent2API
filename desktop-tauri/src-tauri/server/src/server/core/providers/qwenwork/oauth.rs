//! 千问办公的 PKCE 设备授权（网页登录）。
//!
//! ── 与 Qoder 同一套协议，但**只有一处主机不同** ──────────────────
//! 客户端 `main.js` 里 `deviceFlow` 那段与 Qoder 逐字同构（`challenge` /
//! `challenge_method=S256` / `nonce` / `machine_id` / `client_id`），
//! 差别只有两个常量：
//!   * `client_id` = [`endpoints::DEVICE_CLIENT_ID`]（不是 Qoder 那个）；
//!   * 授权页挂在**网关域**（[`endpoints::DEVICE_AUTH_PATH`]），不是独立登录站。
//!
//! ── 实测依据 ────────────────────────────────────────────────
//!   * `GET https://gateway.qwenwork.cn/device/selectAccounts?challenge=…`
//!     → **302**（跳登录页，证明路径与参数名都对）；
//!   * 同一主机 `GET /api/v1/deviceToken/poll?nonce=…&verifier=…` → **400
//!     `INVALID_DEVICE_FLOW`**（参数不全，证明端点存在）；
//!   * `qwenwork.cn` 裸域该路径 404、`www.qwenwork.cn` 301 —— 所以主机必须
//!     是网关域，不要「顺手」改成门户域。
//!
//! ── verifier 不出后端 ───────────────────────────────────────
//! PKCE 的全部安全性在于 verifier 只有发起方知道。它**只**存在于
//! [`DeviceLogin`] 的私有字段里：不进前端状态（前端只拿 `authUrl`）、
//! 不进日志、不落盘。改动这里时请守住这条。
//!
//! ── 轮询的三种「还没好」────────────────────────────────────
//! 202（等待授权）/ 404（state 尚未建立）/ 400 `INVALID_DEVICE_FLOW`
//! 都**不是**错误，而是「继续等」。其中 400 尤其要当心：它看起来像「参数
//! 写错了」，但上游在 nonce 还没被授权页登记时也用这个码回答，把当次登录
//! 判失败会让用户刚打开浏览器就被判超时。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::errors::GatewayError;
use crate::server::logging;

use super::auth::{self, Credentials};
use super::endpoints;

/// 一次设备授权会话。
pub struct DeviceLogin {
    /// 登录任务表里的 state（前端 `/wait` 用它查结果）
    pub state: String,
    /// 给用户打开的授权页地址（含 challenge，**不含 verifier**）
    pub auth_url: String,
    poll_url: String,
    machine_id: String,
}

impl DeviceLogin {
    /// 造一次授权：现生成 verifier / challenge / nonce，拼授权页与轮询地址。
    pub fn new() -> Result<Self, GatewayError> {
        let verifier = random_token(64)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let nonce = random_uuid()?;
        let machine_id = machine_id()?;

        let mut auth_url = url::Url::parse(&endpoints::gateway_url(endpoints::DEVICE_AUTH_PATH))
            .map_err(|_| GatewayError::with_status(500, "千问办公授权地址配置无效"))?;
        auth_url
            .query_pairs_mut()
            .append_pair("challenge", &challenge)
            .append_pair("challenge_method", "S256")
            .append_pair("nonce", &nonce)
            .append_pair("machine_id", &machine_id)
            .append_pair("client_id", endpoints::DEVICE_CLIENT_ID)
            .append_pair("redirect_uri", endpoints::DEVICE_REDIRECT_URI);

        // 轮询**不能**带 client_id / redirect_uri：实测带了这两个上游回
        // `INVALID_DEVICE_FLOW`（它按 nonce+verifier 反查这次授权，别的都不认）。
        let mut poll_url = url::Url::parse(&endpoints::gateway_url(endpoints::DEVICE_POLL_PATH))
            .map_err(|_| GatewayError::with_status(500, "千问办公轮询地址配置无效"))?;
        poll_url
            .query_pairs_mut()
            .append_pair("nonce", &nonce)
            .append_pair("verifier", &verifier)
            .append_pair("challenge_method", "S256");

        Ok(Self {
            state: random_token(32)?,
            auth_url: auth_url.to_string(),
            poll_url: poll_url.to_string(),
            machine_id,
        })
    }

    /// 问一次「用户授权了吗」。`Ok(None)` = 还没好（继续等）。
    pub async fn poll(&self) -> Result<Option<Credentials>, GatewayError> {
        let client = crate::server::core::egress::client_for_system_proxy();
        let response = client
            .get(&self.poll_url)
            .timeout(super::CONTROL_TIMEOUT)
            .header("Accept", "application/json")
            .header("User-Agent", super::user_agent())
            .send()
            .await
            .map_err(|err| {
                let mut error =
                    GatewayError::with_status(502, format!("千问办公设备授权轮询不可达：{err}"));
                // 调用方据此判断「网络抖一下，继续等」而不是「登录失败」
                error.code = Some("qwenwork_transport".to_string());
                error
            })?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();

        // 三种「还没好」见模块头
        if status == 202 || status == 404 {
            return Ok(None);
        }
        if status == 400 && text.contains("INVALID_DEVICE_FLOW") {
            logging::verbose("[Login]", "千问办公设备授权尚未登记，继续等待");
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            return Err(GatewayError::with_status(
                if status == 401 { 401 } else { 502 },
                format!("千问办公设备授权返回 HTTP {status}"),
            ));
        }

        let body: Value = serde_json::from_str(&text)
            .map_err(|_| GatewayError::with_status(502, "千问办公设备授权响应不是合法 JSON"))?;
        let token = body
            .get("token")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if token.is_empty() {
            return Err(GatewayError::with_status(502, "千问办公设备授权响应缺少 token"));
        }
        let user_id = body
            .get("user_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            // 上游有时不回 `user_id`，那就从令牌本体解析（更权威）
            .or_else(|| auth::user_id_from_jwt(token))
            .unwrap_or_default();
        let expires_at = body
            .get("expires_at")
            .and_then(auth::parse_iso8601_ms_value)
            .or_else(|| auth::expires_at_from_jwt(token))
            .unwrap_or_else(|| auth::now_ms() + 7 * 24 * 60 * 60 * 1000);
        Ok(Some(Credentials {
            token: token.to_string(),
            refresh_token: body
                .get("refresh_token")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            expires_at: Some(expires_at),
            user_id,
            username: String::new(),
            name: String::new(),
            plan_name: String::new(),
        }))
    }

    /// 机器标识（落进 Credentials 以便下次续期时带回，上游按它做设备一致性校验）。
    pub fn machine_id(&self) -> &str {
        &self.machine_id
    }
}

/// 随机串（字母数字 + `-._~`，符合 PKCE 的 unreserved 字符集）。
fn random_token(length: usize) -> Result<String, GatewayError> {
    const ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut bytes = vec![0u8; length];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| GatewayError::with_status(500, "无法生成千问办公登录随机串"))?;
    Ok(bytes
        .iter()
        .map(|byte| ALPHABET[*byte as usize % ALPHABET.len()] as char)
        .collect())
}

/// 随机 UUID（带连字符，与客户端 `crypto.randomUUID()` 同形）。
fn random_uuid() -> Result<String, GatewayError> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| GatewayError::with_status(500, "无法生成千问办公设备标识"))?;
    // RFC 4122 v4
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]
    ))
}

/// 设备标识：沿用 Qoder 那套「`{config_dir}` 下存一份」的做法，只是文件名不同。
///
/// ── 为什么与 Qoder 各自存一份 ──────────────────────────────
/// 两家的设备标识会被上游当作风控维度（客户端把机器码标在请求里），
/// 共用一个既没有依据、哪天换了算法还会互相牵动。文件读写失败不阻断登录：
/// 现造一个临时标识用这次，只是下次会变（上游对首次登录的设备一致性要求最松）。
fn machine_id() -> Result<String, GatewayError> {
    let directory = crate::server::config::config_dir();
    let path = directory.join("qwenwork-machine-id");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let trimmed = text.trim();
        if !trimmed.is_empty() && trimmed.len() <= 256 && !trimmed.chars().any(char::is_control) {
            return Ok(trimmed.to_string());
        }
    }
    let value = random_uuid()?;
    if std::fs::create_dir_all(&directory)
        .and_then(|_| std::fs::write(&path, &value))
        .is_err()
    {
        logging::verbose("[Login]", "千问办公设备标识写入失败（本次沿用临时值）");
    }
    Ok(value)
}

/// 把凭据探一次用户资料，补上昵称/套餐名（失败不影响登录成功）。
///
/// ── 为什么整段都是同步的 ────────────────────────────────────
/// 它的调用点在登录任务的 `async` 块里，而那个块要被 `crate::spawn_task`
/// 扔到多线程运行时上（要求 `Send`）。reqwest 的 `Response` 在 `.json()`
/// 之前持有连接，跨 `.await` 会让整块变成非 `Send`。所以这里先把响应整个
/// 读成字符串（`.text().await` 后立刻结束持有），再纯同步地解析 ——
/// 于是本函数内部**只有一次 await**，且那一刻不跨任何非 Send 值。
pub async fn enrich(credentials: &mut Credentials) {
    let url = endpoints::gateway_url(endpoints::USERINFO_PATH);
    let client = crate::server::core::egress::client_for_system_proxy();
    let Ok(response) = client
        .get(&url)
        .timeout(super::CONTROL_TIMEOUT)
        .header("Authorization", format!("Bearer {}", credentials.token))
        .header("Accept", "application/json")
        .header("User-Agent", super::user_agent())
        .send()
        .await
    else {
        return;
    };
    if !response.status().is_success() {
        return;
    }
    let text = response.text().await.unwrap_or_default();
    let Ok(body) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    apply_profile(credentials, &body);
}

/// 把 `userinfo` 的字段填进凭据（纯函数，便于单测）。
fn apply_profile(credentials: &mut Credentials, body: &Value) {
    let pick = |keys: &[&str]| -> String {
        for key in keys {
            if let Some(text) = body.get(*key).and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    return text.to_string();
                }
            }
        }
        String::new()
    };
    // 实测 `userinfo` 的键：id / email / name / nickname / username / plan
    credentials.name = pick(&["name", "nickname", "display_name"]);
    credentials.username = pick(&["username", "email", "account"]);
    // `userinfo.plan.name` 在本家实测是**空串**（免费版），真正的套餐名在
    // `account-context.data.plan.name`；这里读到非空才写，让它去覆盖导入时的值
    let plan = body
        .pointer("/plan/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(plan) = plan {
        credentials.plan_name = plan.to_string();
    }
    if credentials.user_id.is_empty() {
        credentials.user_id = pick(&["id", "user_id", "uid"]);
    }
    logging::log_event(
        "info",
        "qwenwork",
        "网页登录已取到用户资料",
        Some(json!({ "uid": credentials.user_id, "plan": credentials.plan_name })),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 授权页必须带上全五个参数，且**绝不能**带 verifier。
    #[test]
    fn auth_url_carries_challenge_but_never_the_verifier() {
        let flow = DeviceLogin::new().expect("造授权会话");
        for key in [
            "challenge=",
            "challenge_method=S256",
            "nonce=",
            "machine_id=",
            "client_id=",
            "redirect_uri=",
        ] {
            assert!(flow.auth_url.contains(key), "授权页缺 {key}");
        }
        assert!(
            !flow.auth_url.contains("verifier"),
            "verifier 不能出现在给用户打开的地址里"
        );
    }

    /// 轮询地址带 verifier，且不带 client_id / redirect_uri
    /// （实测带了这两个上游回 INVALID_DEVICE_FLOW）。
    #[test]
    fn poll_url_carries_verifier_only() {
        let flow = DeviceLogin::new().expect("造授权会话");
        assert!(flow.poll_url.contains("verifier="));
        assert!(flow.poll_url.contains("nonce="));
        assert!(!flow.poll_url.contains("client_id="));
        assert!(!flow.poll_url.contains("redirect_uri="));
    }

    /// 两处主机都在网关域上（门户域那条 404，别「顺手」改）。
    #[test]
    fn both_urls_live_on_the_gateway_host() {
        let flow = DeviceLogin::new().expect("造授权会话");
        assert!(flow.auth_url.starts_with(endpoints::GATEWAY_BASE));
        assert!(flow.poll_url.starts_with(endpoints::GATEWAY_BASE));
    }

    /// 每次发起都是全新的 challenge/nonce（否则重复授权会串会话）。
    #[test]
    fn each_login_gets_fresh_randomness() {
        let first = DeviceLogin::new().expect("造授权会话");
        let second = DeviceLogin::new().expect("造授权会话");
        assert_ne!(first.auth_url, second.auth_url);
        assert_ne!(first.state, second.state);
    }

    #[test]
    fn random_tokens_use_the_pkce_alphabet_only() {
        let token = random_token(64).expect("随机串");
        assert_eq!(token.len(), 64);
        assert!(token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)));
    }

    #[test]
    fn random_uuid_is_v4_shaped() {
        let uuid = random_uuid().expect("随机 uuid");
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.as_bytes()[14], b'4');
        assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    }
}
