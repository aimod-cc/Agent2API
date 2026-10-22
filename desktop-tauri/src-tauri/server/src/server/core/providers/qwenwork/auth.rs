//! 千问办公的凭据链路：**JWT（登录态）→ MaaS access-key（推理密钥）**。
//!
//! ── 为什么这里有两级 token（不看懂这段会写错整个适配器）───────────
//! 千问办公把「你是谁」和「你能调哪个模型端点」拆成了两个令牌，链路是：
//!
//! ```text
//!   auth-v2.dat (本机登录态)
//!        │  ① 解密（Chromium os_crypt：DPAPI → AES-256-GCM）
//!        ▼
//!   JWT  (556 字符, HS256)            ← 只能打 gateway.qwenwork.cn
//!        │  ② POST {gateway}/api/desktop-sessions
//!        ▼
//!   sk-mm-v1.* (MaaS access-key)      ← 只能打 maas.xiaoluozi.cn/v1/*
//!        │  ttl_ms = 2 小时
//!        └─ 过期后用同一个 JWT 再签一次（无需 refresh_token）
//! ```
//!
//! **两级不能混用**（实测）：
//!   - JWT 打 `maas.xiaoluozi.cn` → `401 unrecognized bearer scheme`
//!   - MaaS key 打 `gateway.qwenwork.cn/api/...` → 401
//!
//! 因此 [`super::mod`] 里 `build_chat_request` 用的是 MaaS key，而
//! `list_models` / `query_usage` 用的是 JWT —— 这不是不一致，是上游要求。
//!
//! ── auth-v2.dat 为什么可以解（不依赖客户端在跑）─────────────────
//! 文件格式是 Chromium 的 `os_crypt`：`Local State` 里存一份 DPAPI 包装过的
//! AES-256 主密钥，`auth-v2.dat` 是 `v10` 前缀 + `12B nonce + 密文 + 16B tag`
//! 的 AES-256-GCM 载荷。DPAPI 以**当前 Windows 用户**身份解密，不需要密码 ——
//! 所以只要 Agent2API 与千问办公跑在同一个用户下，就能在客户端完全没启动时
//! 读到登录态。这是本适配器「免登录、免逆向」的根本。
//!
//! ── 明文回退（`auth-v2.dat.json`）───────────────────────────
//! 上游 `AuthStore.save` 在 `safeStorage` 不可用时会把明文写到
//! `auth-v2.dat.json`，并在下次 `load` 时自动加密回 `.dat`。这里两条路都认：
//! 先试加密文件，失败再试 `.json`（顺序不能反 —— 加密文件才是当前有效的）。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本模块在对话链路上，绝不 unwrap/expect/panic。DPAPI 失败、文件缺失、
//! JSON 结构不符全部走 `GatewayError`。

use std::path::PathBuf;

use serde_json::Value;

use crate::server::errors::GatewayError;
use crate::server::logging;

use super::endpoints;

/// MaaS 密钥到期前提前多久续签。
///
/// `ttl_ms` 是 2 小时，提前 10 分钟续签给时钟偏差与网络重试留余量。
pub const REFRESH_MARGIN_MS: i64 = 10 * 60 * 1000;

/// 解密后的登录态。
#[derive(Clone, Debug)]
pub struct Credentials {
    /// JWT（打 gateway 用）
    pub token: String,
    /// 刷新令牌（`ory_rt_*`）。目前**不使用** —— JWT 有效期 7 天，
    /// 且签发 MaaS key 不需要它；留着是为了将来接自动续登录。
    pub refresh_token: String,
    /// JWT 过期时间（毫秒时间戳）
    pub expires_at: Option<i64>,
    /// 用户 id（UUID）
    pub user_id: String,
    /// 用户名
    pub username: String,
    /// 昵称
    pub name: String,
    /// 套餐名（如「个人免费版」）
    pub plan_name: String,
}

impl Credentials {
    /// 从 accounts.json 里的账号载荷解析。
    ///
    /// 兼容两种来源：
    ///   1. 本机登录态导出（含 `token` / `refreshToken` / `user`）
    ///   2. 用户手动粘贴（`accessToken` / `access_token` 同义键）
    pub fn from_payload(payload: &Value) -> Result<Self, GatewayError> {
        if !payload.is_object() {
            return Err(GatewayError::with_status(400, "千问办公账号内容必须是 JSON 对象"));
        }
        let token = first_string(payload, &["token", "accessToken", "access_token", "access"]);
        if token.is_empty() {
            return Err(GatewayError::with_status(
                400,
                "缺少千问办公登录令牌（token / accessToken）",
            ));
        }
        let user = payload.get("user").cloned().unwrap_or(Value::Null);
        let user_id = if user.is_object() {
            first_string(&user, &["id", "userId", "user_id"])
        } else {
            first_string(payload, &["userId", "user_id", "uid"])
        };
        let username = if user.is_object() {
            first_string(&user, &["username"])
        } else {
            first_string(payload, &["username"])
        };
        let name = if user.is_object() {
            first_string(&user, &["name", "nickname"])
        } else {
            first_string(payload, &["name", "nickname"])
        };
        let plan_name = if user.is_object() {
            first_string(&user, &["planName", "plan", "tier"])
        } else {
            first_string(payload, &["planName", "plan", "tier"])
        };
        if user_id.is_empty() {
            return Err(GatewayError::with_status(
                400,
                "缺少千问办公用户 id（user.id），无法标识账号",
            ));
        }
        // 本机登录态文件**通常不写过期时刻**（实测客户端只存 token / user /
        // refreshToken），而 JWT 里本来就有 `exp` —— 那才是「这枚令牌什么时候
        // 失效」的事实来源。因此在文件字段缺席时从 JWT 补一次：
        // 少了这一层，「有效期」那一列对本家永远是空的，而用户只能按
        // 「突然转发 401」来发现令牌过期（本家没有自动续期接口，过期必须重登）。
        let expires_at = ["expiresAt", "expires_at", "expires"]
            .iter()
            .find_map(|k| timestamp(payload.get(*k)))
            .or_else(|| expires_at_from_jwt(&token));
        Ok(Self {
            token,
            refresh_token: first_string(payload, &["refreshToken", "refresh_token", "refresh"]),
            expires_at,
            user_id: user_id.chars().take(64).collect(),
            username: username.chars().take(100).collect(),
            name: name.chars().take(100).collect(),
            plan_name: plan_name.chars().take(100).collect(),
        })
    }

    /// JWT 是否已过期（没有过期信息时按「未过期」处理，交给上游 401 兜底）。
    pub fn is_expired(&self, now_ms: i64) -> bool {
        match self.expires_at {
            Some(at) => now_ms >= at,
            None => false,
        }
    }

    /// 把登录态序列化回账号载荷（`ensure_access_token` 刷新后会重新落盘）。
    pub fn to_payload(&self) -> Value {
        let mut user = serde_json::Map::new();
        user.insert("id".into(), Value::String(self.user_id.clone()));
        if !self.username.is_empty() {
            user.insert("username".into(), Value::String(self.username.clone()));
        }
        if !self.name.is_empty() {
            user.insert("name".into(), Value::String(self.name.clone()));
        }
        if !self.plan_name.is_empty() {
            user.insert("planName".into(), Value::String(self.plan_name.clone()));
        }
        let mut root = serde_json::Map::new();
        root.insert("token".into(), Value::String(self.token.clone()));
        if !self.refresh_token.is_empty() {
            root.insert("refreshToken".into(), Value::String(self.refresh_token.clone()));
        }
        if let Some(at) = self.expires_at {
            root.insert("expiresAt".into(), Value::Number(at.into()));
        }
        root.insert("user".into(), Value::Object(user));
        Value::Object(root)
    }
}

/// 签发出来的 MaaS 密钥。
#[derive(Clone, Debug)]
pub struct MaasKey {
    /// `sk-mm-v1.*`
    pub access_key: String,
    /// 推理 base（响应里的 `base_url`，可能不含 `/v1`）
    pub base_url: String,
    /// 过期时间（毫秒时间戳）；0 表示上游没给，按「不轮换」处理
    pub expires_at_ms: i64,
    /// 签发会话 id（推理响应里会作为 `request_id` 回显）
    pub session_id: String,
}

impl MaasKey {
    /// 在 `now_ms` 时刻这个密钥还能不能用。
    pub fn is_usable(&self, now_ms: i64) -> bool {
        if self.access_key.is_empty() {
            return false;
        }
        // 上游没给过期时间时（expires_at_ms == 0）按永久可用处理，
        // 由 401 触发的强制刷新兜底（与 `refresh_access_token` 的默认语义一致）。
        self.expires_at_ms <= 0 || now_ms + REFRESH_MARGIN_MS < self.expires_at_ms
    }

    /// 解析 `POST /api/desktop-sessions` 的响应。
    ///
    /// 实测响应：`{access_token, base_url, expires_at_ms, session_id, ttl_ms}`。
    /// `expires_at_ms` 缺失时用 `ttl_ms` 现算，两者都没有则记 0。
    pub fn from_mint_response(body: &Value) -> Result<Self, GatewayError> {
        let access_key = first_string(body, &["access_token", "accessToken", "key"]);
        if access_key.is_empty() {
            return Err(GatewayError::with_status(
                502,
                "千问办公签发接口返回了空密钥（access_token 缺失）",
            ));
        }
        let mut expires_at_ms = body
            .get("expires_at_ms")
            .and_then(Value::as_i64)
            .or_else(|| body.get("expiresAtMs").and_then(Value::as_i64))
            .unwrap_or(0);
        if expires_at_ms == 0 {
            if let Some(ttl) = body.get("ttl_ms").and_then(Value::as_i64) {
                if ttl > 0 {
                    expires_at_ms = now_ms() + ttl;
                }
            }
        }
        let base_url = {
            let raw = first_string(body, &["base_url", "baseUrl"]);
            if raw.is_empty() {
                endpoints::DEFAULT_MAAS_BASE.to_string()
            } else {
                raw
            }
        };
        Ok(Self {
            access_key,
            base_url,
            expires_at_ms,
            session_id: first_string(body, &["session_id", "sessionId"]),
        })
    }
}

/// 当前毫秒时间戳。
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── JWT 载荷读取（不验签）──────────────────────────────────────

/// 解出 JWT 的 payload 段。
///
/// ── 为什么**不验签** ───────────────────────────────────────
/// 我们不是鉴权方，验不了（HS256 的密钥在服务端）。这里只读三类**元信息**：
/// 用户 id、过期时间、昵称 —— 真正的「这枚令牌还能不能用」由
/// `QwenworkAdapter::verify_and_enrich` 打一次 `userinfo` 回答。
/// 因此篡改 JWT 在这里毫无收益：改出来的 id 只会让账号记录挂错名字，
/// 而上游照样不认那枚令牌。
fn jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    if payload.is_empty() {
        return None;
    }
    let bytes = super::crypt::base64_decode(payload)?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

/// 从 JWT 里读用户 id（`sub` 优先，其次 `user_id` / `uid`）。
///
/// `sub` 是这类 Ory/Keycloak 系 JWT 的标准用户标识位（实测 556 字符的
/// 千问办公 JWT 就是 UUID 形态的 `sub`）。读不到返回 None —— 调用方据此
/// 要求用户补字段，而不是瞎编一个 id 出来。
pub fn user_id_from_jwt(token: &str) -> Option<String> {
    let payload = jwt_payload(token)?;
    let id = first_string(&payload, &["sub", "user_id", "userId", "uid"]);
    if id.is_empty() {
        None
    } else {
        Some(id.chars().take(64).collect())
    }
}

/// 从 JWT 里读过期时间（`exp`，秒级 → 毫秒）。
///
/// 读不到返回 None：`Credentials::is_expired` 对 None 的处理是「按未过期算」，
/// 交给上游 401 兜底 —— 这比拿一个编出来的时间误判「已过期」安全。
pub fn expires_at_from_jwt(token: &str) -> Option<i64> {
    let payload = jwt_payload(token)?;
    let exp = payload.get("exp").and_then(Value::as_i64)?;
    if exp <= 0 {
        return None;
    }
    Some(if exp < 1_000_000_000_000 {
        exp * 1000
    } else {
        exp
    })
}

// ── 本机登录态读取 ──────────────────────────────────────────────

/// 千问办公在本机的数据目录。
///
/// Windows 上是 `%APPDATA%\QwenWorkCN`（Electron 的 `userData`）。
/// 非 Windows 或拿不到 `APPDATA` 时返回 None —— 本适配器目前只在 Windows 上
/// 有意义（客户端只有 Windows 版），返回 None 让上层给出明确提示而不是瞎找。
pub fn local_data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(appdata).join("QwenWorkCN"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 从本机登录态读取账号（加密文件优先，明文 `.json` 兜底）。
///
/// 供「添加账号」的『从本机导入』入口使用。读不到时返回的错误文案要能指导
/// 用户动作 —— 「先在千问办公客户端登录一次」才是他该做的事。
pub fn read_local_credentials() -> Result<Credentials, GatewayError> {
    let dir = local_data_dir().ok_or_else(|| {
        GatewayError::with_status(400, "当前系统不支持读取千问办公本机登录态（仅 Windows）")
    })?;
    if !dir.is_dir() {
        return Err(GatewayError::with_status(
            400,
            "未找到千问办公本机数据目录，请先安装并登录千问办公客户端",
        ));
    }

    let encrypted = dir.join("auth-v2.dat");
    if encrypted.is_file() {
        match read_encrypted(&dir, &encrypted) {
            Ok(creds) => return Ok(creds),
            Err(err) => {
                // 解密失败不直接返回：明文回退可能才是当前有效的那份
                // （客户端在 safeStorage 不可用时写到 `.json`）。
                logging::log_event(
                    "warn",
                    "qwenwork",
                    "本机登录态解密失败，尝试明文回退",
                    Some(serde_json::json!({ "error": err.message })),
                );
            }
        }
    }

    let plain = dir.join("auth-v2.dat.json");
    if plain.is_file() {
        let text = std::fs::read_to_string(&plain).map_err(|err| {
            GatewayError::with_status(
                500,
                format!("读取千问办公登录态失败：{}", err.kind()),
            )
        })?;
        let value: Value = serde_json::from_str(&text).map_err(|_| {
            GatewayError::with_status(400, "千问办公明文登录态不是合法 JSON")
        })?;
        return Credentials::from_payload(&value);
    }

    Err(GatewayError::with_status(
        400,
        "未找到千问办公登录态，请先在客户端完成登录",
    ))
}

/// 读 `auth-v2.dat`：Chromium os_crypt（DPAPI 主密钥 + AES-256-GCM）。
#[cfg(windows)]
fn read_encrypted(dir: &std::path::Path, file: &std::path::Path) -> Result<Credentials, GatewayError> {
    let state_text = std::fs::read_to_string(dir.join("Local State")).map_err(|_| {
        GatewayError::with_status(400, "千问办公 Local State 缺失，无法解出登录态")
    })?;
    let state: Value = serde_json::from_str(&state_text)
        .map_err(|_| GatewayError::with_status(400, "千问办公 Local State 不是合法 JSON"))?;
    let b64 = state
        .get("os_crypt")
        .and_then(|v| v.get("encrypted_key"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            GatewayError::with_status(400, "千问办公 Local State 缺少 os_crypt.encrypted_key")
        })?;
    let wrapped = crate::server::core::providers::qwenwork::crypt::base64_decode(b64)
        .ok_or_else(|| GatewayError::with_status(400, "os_crypt.encrypted_key 不是合法 base64"))?;
    // 密钥外层是 "DPAPI" 魔数 + DPAPI 密文
    let key_blob = wrapped.strip_prefix(b"DPAPI").ok_or_else(|| {
        GatewayError::with_status(400, "os_crypt.encrypted_key 前缀不是 DPAPI")
    })?;
    let master = crate::server::core::providers::qwenwork::crypt::dpapi_unprotect(key_blob)?;

    let raw = std::fs::read(file)
        .map_err(|_| GatewayError::with_status(400, "读取千问办公 auth-v2.dat 失败"))?;
    let payload = raw.strip_prefix(b"v10").ok_or_else(|| {
        GatewayError::with_status(400, "千问办公 auth-v2.dat 不是 v10 格式")
    })?;
    if payload.len() < 12 + 16 {
        return Err(GatewayError::with_status(400, "千问办公 auth-v2.dat 长度不足"));
    }
    let (nonce, cipher) = payload.split_at(12);
    let plain = crate::server::core::providers::qwenwork::crypt::aes256_gcm_open(&master, nonce, cipher)?;
    let value: Value = serde_json::from_slice(&plain)
        .map_err(|_| GatewayError::with_status(400, "千问办公登录态解密后不是合法 JSON"))?;
    // schemaVersion 必须为 2；不是 2 说明客户端改版了，宁可报错也不要猜字段
    match value.get("schemaVersion").and_then(Value::as_i64) {
        Some(2) => {}
        Some(other) => {
            return Err(GatewayError::with_status(
                400,
                format!("千问办公登录态 schemaVersion={other}，当前适配器只认 2"),
            ))
        }
        None => {
            return Err(GatewayError::with_status(
                400,
                "千问办公登录态缺少 schemaVersion",
            ))
        }
    }
    Credentials::from_payload(&value)
}

#[cfg(not(windows))]
fn read_encrypted(_dir: &std::path::Path, _file: &std::path::Path) -> Result<Credentials, GatewayError> {
    Err(GatewayError::with_status(400, "非 Windows 系统无法解密千问办公登录态"))
}

// ── 小工具 ──────────────────────────────────────────────────────

pub(crate) fn first_string(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(s) = value.get(*key).and_then(Value::as_str) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

fn timestamp(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(n) = value.as_i64() {
        // 秒级时间戳（< 1e12）换算成毫秒：`expiresAt` 在 auth-v2.dat 里是 ISO
        // 字符串，但用户手贴的账号记录常用秒级数字，两种都收。
        return Some(if n < 1_000_000_000_000 { n * 1000 } else { n });
    }
    let text = value.as_str()?.trim();
    parse_iso8601_ms(text)
}

/// 把任意 JSON 时间值（ISO 字符串 / 秒级或毫秒级数字）转成毫秒时间戳。
///
/// 设备授权的 `expires_at` 在不同上游版本里是字符串也有可能是数字，
/// 登录链路不该为此各写一份 —— 这里统一入口。
pub fn parse_iso8601_ms_value(value: &Value) -> Option<i64> {
    timestamp(Some(value))
}

/// 解析 `2026-10-26T12:32:47Z` / `...+08:00` 这类 ISO 8601 时间为毫秒时间戳。
fn parse_iso8601_ms(text: &str) -> Option<i64> {
    // 只认 `YYYY-MM-DDTHH:MM:SS` 前缀，时间与日期分开算，避免引第三方依赖
    let (date, rest) = text.split_once('T')?;
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let mo: i64 = dp.next()?.parse().ok()?;
    let d: i64 = dp.next()?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    // 去掉尾部时区（Z / +08:00 / -05:00），统一按 UTC 计算
    let mut tz_offset_sec = 0i64;
    let mut hms = rest;
    if let Some(idx) = rest.find(['Z', 'z']) {
        hms = &rest[..idx];
    } else if let Some(idx) = rest.rfind(['+', '-']) {
        let (time, tz) = rest.split_at(idx);
        hms = time;
        let sign = if tz.starts_with('-') { -1 } else { 1 };
        let body = &tz[1..];
        let mut parts = body.split(':');
        let hh: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let mm: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        tz_offset_sec = sign * (hh * 3600 + mm * 60);
    }
    let hms = hms.trim_end_matches('Z').trim_end_matches('z');
    let mut tp = hms.split(':');
    let hh: i64 = tp.next()?.parse().ok()?;
    let mi: i64 = tp.next().unwrap_or("0").parse().ok()?;
    let ss_part = tp.next().unwrap_or("0");
    let ss: i64 = ss_part.split('.').next()?.parse().ok()?;
    if !(0..=23).contains(&hh) || !(0..=59).contains(&mi) || !(0..=60).contains(&ss) {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    let secs = days * 86400 + hh * 3600 + mi * 60 + ss - tz_offset_sec;
    Some(secs * 1000)
}

/// 儒略日式公历转天数（Howard Hinnant 的 `days_from_civil`）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 本机登录态文件里**没有**过期字段时，`expiresAt` 必须从 JWT 的 `exp` 来
    /// —— 少了这一层，账号页「有效期」那一列对本家永远是空的，而本家没有自动
    /// 续期接口，用户只能靠「突然转发 401」发现令牌过期。
    #[test]
    fn expires_at_falls_back_to_the_jwt_exp() {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::STANDARD;
        let payload = engine.encode(r#"{"sub":"u1","exp":1793029526}"#);
        let token = format!("header.{payload}.sig");
        let payload = serde_json::json!({
            "token": token,
            "user": { "id": "u1" },
        });
        let creds = Credentials::from_payload(&payload).expect("parse");
        assert_eq!(creds.expires_at, Some(1_793_029_526_000));
    }

    #[test]
    fn parses_iso8601_utc() {
        // 2026-10-26T12:32:47Z
        let ms = parse_iso8601_ms("2026-10-26T12:32:47Z").expect("parse");
        // 1970 → 2026-10-26 的秒数（用同一算法反向校验：整除关系）
        assert_eq!(ms % 1000, 0);
        assert!(ms > 1_700_000_000_000 && ms < 1_900_000_000_000);
        // 与已知点对比：1970-01-01T00:00:00Z == 0
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00Z"), Some(0));
        // 2020-01-01T00:00:00Z == 1577836800000
        assert_eq!(parse_iso8601_ms("2020-01-01T00:00:00Z"), Some(1_577_836_800_000));
    }

    #[test]
    fn parses_iso8601_with_offset() {
        // +08:00 的同一时刻应比 Z 少 8 小时
        let z = parse_iso8601_ms("2026-10-26T12:32:47Z").unwrap();
        let plus8 = parse_iso8601_ms("2026-10-26T20:32:47+08:00").unwrap();
        assert_eq!(z, plus8);
    }

    #[test]
    fn credentials_roundtrip() {
        let payload = serde_json::json!({
            "token": "abc.def.ghi",
            "refreshToken": "ory_rt_x",
            "expiresAt": "2026-10-26T12:32:47Z",
            "user": { "id": "u-1", "username": "u", "name": "N", "planName": "免费" }
        });
        let c = Credentials::from_payload(&payload).expect("parse");
        assert_eq!(c.user_id, "u-1");
        assert_eq!(c.username, "u");
        assert!(c.expires_at.is_some());
        let back = c.to_payload();
        let c2 = Credentials::from_payload(&back).expect("reparse");
        assert_eq!(c2.user_id, "u-1");
        assert_eq!(c2.token, "abc.def.ghi");
    }

    #[test]
    fn credentials_rejects_missing_token() {
        let payload = serde_json::json!({ "user": { "id": "u" } });
        assert!(Credentials::from_payload(&payload).is_err());
    }

    #[test]
    fn maas_key_from_response_uses_ttl_when_no_expiry() {
        let now = now_ms();
        let body = serde_json::json!({
            "access_token": "sk-mm-v1.x",
            "base_url": "https://maas.xiaoluozi.cn",
            "ttl_ms": 7_200_000_i64
        });
        let k = MaasKey::from_mint_response(&body).expect("parse");
        assert_eq!(k.base_url, "https://maas.xiaoluozi.cn");
        assert!(k.expires_at_ms >= now + 7_000_000);
        assert!(k.is_usable(now));
        assert!(!k.is_usable(k.expires_at_ms));
    }

    #[test]
    fn maas_key_rejects_empty() {
        let body = serde_json::json!({ "base_url": "x" });
        assert!(MaasKey::from_mint_response(&body).is_err());
    }
}
