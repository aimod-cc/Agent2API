//! 智谱清言凭证：形态判定 / 解析 / 本机登录态读取 / 从账号记录取快照。
//!
//! ── 凭证形态 ────────────────────────────────────────────────
//! 本家有两类凭证，**都在账号里存一对 `(accessToken, refreshToken)`**：
//!
//! ```text
//!   访客（guest）   POST /chatglm/user-api/guest/access（空 body）换发，
//!                   不需要任何前置登录 —— 这是本家「零配置就能用」的入口。
//!                   换发**只依赖 X-Device-Id**：换一个设备号就是一个新访客身份。
//!   登录账号        phone_login / auth_code_login / 粘贴 access_token 得到，
//!                   有真实的积分、余额、每日登录积分与活动领取能力。
//! ```
//!
//! 两类共用同一套转发链路（同一个接口、同一套头），差别只在「有没有积分」：
//! 访客走 `engine/stream` 实测可用，但积分 / 余额接口一律 401（`user needs login`）。
//! 因此**能力位由凭证形态决定**（见 `is_guest`）：签到与余额只对登录账号开放。
//!
//! ── 为什么要存 deviceId（本家特有）────────────────────────────
//! 服务端把 `X-Device-Id` 写进签发的 JWT（`device_id` 声明），而访客身份
//! **按设备号分配**（实测两个 UUID 各自换到不同 `user_id`）。也就是说设备号
//! 不是可丢弃的装饰，而**是身份的一部分**：
//!   - 每次请求现随机 → 每次都在换身份，会话连续性丢失；
//!   - 换设备号 → 上游把它当新访客，旧身份与已累积的免费额度作废。
//! 因此设备号落进账号记录（`deviceId` 字段），随凭证一起持久化；
//! 缺失时才现生成并**写回**（见 `credentials::resolve_device_id`）。
//!
//! ── 本机登录态（`importDesktop`）──────────────────────────────
//! 客户端 `%APPDATA%\chatglm\Network\Cookies` 是 Chromium 的 SQLite。
//! 与 KukuAI 不同，**本家这份是 DPAPI 加密的**（`Local State` 里有
//! `os_crypt.encrypted_key`，cookie 值 `v10` 前缀）—— 因此只有 Windows
//! 且能解 DPAPI 时才可读；解不出就如实告知用户改用粘贴凭证
//! （与 AutoClaw 的处置一致：macOS / 环境不支持时整段收起）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;
use std::path::PathBuf;

use crate::server::errors::GatewayError;

use super::sign;

/// access_token / refresh_token 的最小长度（JWT 实测远大于此；只拦明显残缺的值）
const MIN_TOKEN_LENGTH: usize = 16;

/// 本家两条登录态 cookie 名（桌面端 Cookies 表里找它们）
pub const COOKIE_ACCESS_TOKEN: &str = "chatglm_token";

/// 刷新令牌 cookie 名
pub const COOKIE_REFRESH_TOKEN: &str = "chatglm_refresh_token";

/// 一份智谱清言凭证快照（转发 / 余额 / 签到共用）
#[derive(Clone, Debug)]
pub struct ChatglmCredentials {
    /// access_token（Bearer 的取值）
    pub access_token: String,
    /// refresh_token（`user/refresh` 的入参）
    pub refresh_token: String,
    /// 设备号（**身份的一部分**，见模块头；必须 UUID 形态）
    pub device_id: String,
    /// 是否访客（由 JWT 的 `is_guest` 声明判定；取不到时按「有 refresh 就当真账号」兜底）
    pub is_guest: bool,
    /// 用户 id（JWT 的 `uid` 声明；展示与账号 id 来源）
    pub uid: String,
    /// 展示名（昵称；best-effort）
    pub nickname: String,
}

impl ChatglmCredentials {
    /// 脱敏尾段（展示用；与各家 `tokenTail` 同一口径）
    pub fn token_tail(&self) -> String {
        let token = if !self.access_token.is_empty() {
            &self.access_token
        } else {
            &self.refresh_token
        };
        if token.len() <= 8 {
            return String::new();
        }
        token.chars().skip(token.len() - 8).collect()
    }

    /// 转发用令牌：优先 access_token，为空时退回 refresh_token
    /// （与客户端的兜底同口径 —— 至少能让上游回一句可读的 401）
    pub fn bearer(&self) -> String {
        if !self.access_token.is_empty() {
            return self.access_token.clone();
        }
        self.refresh_token.clone()
    }
}

/// 从 JWT 的载荷段里读一个字符串字段（不解签，只读声明）。
///
/// 本家的 access_token 是标准三段 JWT，`is_guest` / `uid` / `device_id` 都在
/// 载荷里 —— 客户端自己也是这么读的（`auth-headers.js` 只看 cookie，
/// 而网页侧读的就是 JWT 声明）。解码失败一律返回 None：凭证可能被用户改坏，
/// 不该因此 panic。
fn jwt_claim(token: &str, key: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64_url_decode(payload)?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get(key).cloned()
}

/// base64url（无填充）解码 —— JWT 载荷段的形态
fn base64_url_decode(text: &str) -> Option<Vec<u8>> {
    let mut buffer: Vec<u8> = Vec::with_capacity(text.len());
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buffer.push(((accumulator >> bits) & 0xff) as u8);
        }
    }
    Some(buffer)
}

/// 判定一个 access_token 是不是访客令牌（JWT 的 `is_guest` 声明）。
///
/// 解不出声明时按 `fallback` 走 —— 调用方按语境给默认值：粘贴入口给
/// `false`（用户粘的多半是登录令牌），guest 换发入口直接给 `true`。
pub fn token_is_guest(token: &str, fallback: bool) -> bool {
    match jwt_claim(token, "is_guest") {
        Some(Value::Bool(flag)) => flag,
        _ => fallback,
    }
}

/// 从 access_token 里读用户 id（JWT 的 `uid` 声明；取不到给空串）
pub fn token_uid(token: &str) -> String {
    jwt_claim(token, "uid")
        .or_else(|| jwt_claim(token, "sub"))
        .map(|value| match value {
            Value::String(text) => text,
            other => other.to_string(),
        })
        .unwrap_or_default()
}

/// 提前续期的窗口。
///
/// ── 为什么是 1 小时（实测定的，别凭直觉改大）────────────────────
/// **访客令牌的有效期只有 24 小时**（实测 JWT：`exp - iat = 86400`，见
/// `live_tests`），登录令牌更长但同样以天计。窗口若按「一天」设，刚换到的
/// 访客令牌会**立刻**被判成临期 —— 后果是每轮请求前都去续期一次，
/// 白白多打一次上游（`credential_maintenance` 会遍历所有账号问一遍）。
/// 1 小时既留足了「续期失败还有重试余量」，又不会让新令牌一上来就被判临期。
const REFRESH_WINDOW_SECONDS: i64 = 60 * 60;

/// 令牌是否即将过期（维护任务据此提前续期）。
///
/// 读 JWT 的 `exp` 声明（秒级 Unix 时间戳）—— 这是**上游自己写的**有效期，
/// 不靠猜也不靠本地缓存的时间。`exp` 缺失 / 解不出来一律返回 false
/// （宁可漏刷也不要凭猜测去打上游，与契约里那条判据同口径）。
pub fn token_expiring_soon(token: &str) -> bool {
    let Some(exp) = jwt_claim(token, "exp").and_then(|value| match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }) else {
        return false;
    };
    let now = crate::server::logging::now_ms() / 1000;
    // exp 已过 → 也算「临期」（维护任务该去续期；续不成会报出来）
    exp - now <= REFRESH_WINDOW_SECONDS
}

/// 取一个可长期使用的设备号：优先用账号里记着的，缺失时新生成一个。
///
/// ── 为什么必须「复用」而不是每次现生成（本家特有，见模块头）──────
/// 设备号进 JWT、且访客身份按它分配：每次现生成 = 每次换身份。
/// 因此只要账号里存过就沿用它（`present` 非空即认为可用，仅做形态校验）。
pub fn resolve_device_id(present: &str) -> String {
    let value = present.trim();
    if is_uuid_shaped(value) {
        return value.to_string();
    }
    sign::uuid_v4()
}

/// 形态校验：UUID（`8-4-4-4-12`，十六进制）。非 UUID 会被上游 40004 拒绝
/// （实测 32 位 hex / 短串 / 纯时间戳都不行）。
pub fn is_uuid_shaped(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let lengths = [8usize, 4, 4, 4, 12];
    for (part, expected) in parts.iter().zip(lengths.iter()) {
        if part.len() != *expected || !part.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

/// 令牌形态是否明显可用（长度下限；只拦残缺值，不做格式强校验）
pub fn token_shape_ok(token: &str) -> bool {
    token.trim().len() >= MIN_TOKEN_LENGTH
}

/// 从账号记录取凭证快照。
///
/// `account_id` 为空 → 取本家组内优先级最小的启用账号（与转发选路同一判据）；
/// 找不到返回 Err（调用方据此报「没有可用凭证」）。
///
/// ── 令牌读**顶层**键（全仓的存储契约）──────────────────────────
/// `accessToken` / `refreshToken` 在记录顶层，与其余各家一致 —— 这也是
/// `StoredAccount::access_token()` 与 `has_credentials()` 读的位置。嵌套的
/// `auth.accessToken`（参考实现的形态）**不认**：认了它等于给「有没有凭证」
/// 这条判据开第二条读法，而那个判据是转发选路 / 余额查询 / 界面三处共用的
/// 事实来源（见 `chatglm_accounts.rs` 的说明）。
pub fn from_record(record: Option<&Value>) -> Result<ChatglmCredentials, GatewayError> {
    let Some(record) = record else {
        return Err(GatewayError::with_status(
            401,
            "没有可用的智谱清言凭证：请先在账号页添加（访客模式或登录账号）",
        ));
    };
    // 兼容一层历史数据：早期版本把令牌写在 `auth` 里（现已改为顶层），
    // 读到旧记录时仍认，免得用户重装前后那条账号凭空失效
    let field_of = |key: &str| -> String {
        record
            .get(key)
            .and_then(Value::as_str)
            .or_else(|| {
                record
                    .get("auth")
                    .and_then(|auth| auth.get(key))
                    .and_then(Value::as_str)
            })
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    let access_token = field_of("accessToken");
    let refresh_token = field_of("refreshToken");
    if access_token.is_empty() && refresh_token.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "智谱清言账号缺少令牌，请重新添加（访客模式或登录账号）",
        ));
    }
    // 设备号：记录里有就复用，没有则现生成（上层负责写回，见账号层）
    let device_id = resolve_device_id(
        record
            .get("deviceId")
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    // 访客判定：先看记录里显式标的（添加时判定过），再退回读 JWT 声明
    let is_guest = match record.get("isGuest").and_then(Value::as_bool) {
        Some(flag) => flag,
        None => {
            let fallback = refresh_token.is_empty();
            token_is_guest(&access_token, fallback)
        }
    };
    let uid = record
        .get("uid")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let uid = if uid.is_empty() {
        token_uid(&access_token)
    } else {
        uid
    };
    let nickname = record
        .get("nickname")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    Ok(ChatglmCredentials {
        access_token,
        refresh_token,
        device_id,
        is_guest,
        uid,
        nickname,
    })
}

/// 本机客户端 userData 目录候选（找不到返回空列表）。
///
/// 固定形态（Electron 默认）：`%APPDATA%\chatglm`；另有厂商目录名不同的
/// 通配兜底（与 KukuAI 同一处置：靠名字猜不到就别只认一个名字）。
pub fn user_data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for base in ["APPDATA", "HOME"] {
        let Some(value) = std::env::var_os(base) else {
            continue;
        };
        let base = PathBuf::from(value);
        for name in ["chatglm", "智谱清言", "ZPHZ"] {
            let direct = base.join(name);
            if direct.is_dir() && !dirs.contains(&direct) {
                dirs.push(direct);
            }
        }
    }
    dirs
}

/// 定位本机客户端的 Cookies 文件（找不到返回 None）
pub fn cookies_path() -> Option<PathBuf> {
    for dir in user_data_dirs() {
        let path = dir.join("Network").join("Cookies");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// 本机登录态是否**可能**可读（决定「导入本机登录态」入口是否展示）。
///
/// 与 KukuAI 的差别：本家 Cookies 是 DPAPI 加密的，只有 Windows 能解；
/// 非 Windows 一律返回 false（界面层据此刻意收起入口，而不是让人点了才失败）。
pub fn desktop_import_supported() -> bool {
    cfg!(windows) && cookies_path().is_some()
}

/// 从 Cookies SQLite 里读登录态，返回 `(access_token, refresh_token)`。
///
/// ── 与本机其他家的两处关键差别 ──────────────────────────────────
///   1. **值是 DPAPI 加密的**：与 KukuAI（明文 `value` 列）不同，本家
///      `encrypted_value` 是 Chromium os_crypt 密文（`v10` 前缀 + AES-256-GCM），
///      密钥在 `Local State` 的 `os_crypt.encrypted_key` 里、由 DPAPI 保护。
///      解密链**不重写**：直接复用 AutoClaw 已实现并经实测的那一份
///      （`autoclaw::crypto`，见那边的模块头）—— 两家的加密方案是同一个
///      Chromium os_crypt，抄第二份只会多一处会漂移的知识；
///   2. **客户端运行时会独占 Cookies 文件**：读不到时给出「请先关闭客户端」
///      这类可执行文案（与 KukuAI 同一处置）。
///
/// 返回值三态：
///   - `Ok(Some((access, refresh)))` 读到了；
///   - `Ok(None)` 文件在但**没有本家的登录 cookie**（未登录 / 只剩访客）；
///   - `Err(文案)` 读取或解密失败（客户端占用 / 非 Windows / 密钥不匹配）。
#[cfg(windows)]
pub fn read_desktop_credentials() -> Result<Option<(String, String)>, String> {
    let Some(cookies) = cookies_path() else {
        return Ok(None);
    };
    let Some(user_data) = cookies.parent().and_then(|path| path.parent()) else {
        return Err("客户端数据目录结构不认识（找不到 Network 的上级目录）".to_string());
    };
    // ① os_crypt 密钥（DPAPI 解 `Local State`；AutoClaw 那份带进程级缓存）
    let local_state = user_data.join("Local State");
    let aes_key = crate::server::core::providers::autoclaw::crypto::os_crypt_aes_key(&local_state)
        .map_err(|reason| format!("读取客户端加密密钥失败：{reason}"))?;
    // ② 读 Cookies 表（客户端在跑时 SQLite 可能被独占 —— 如实说明）
    let mut rows: Vec<(String, String, Vec<u8>)> = Vec::new();
    {
        let connection = rusqlite::Connection::open_with_flags(
            &cookies,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| {
            format!(
                "打不开客户端 Cookies（{error}）。若智谱清言客户端正在运行，\
                 请先退出它再试"
            )
        })?;
        let mut statement = connection
            .prepare("SELECT name, value, encrypted_value FROM cookies WHERE name LIKE 'chatglm%'")
            .map_err(|error| format!("读取客户端 Cookies 失败：{error}"))?;
        let mapped = statement
            .query_map([], |row| {
                let name: String = row.get(0)?;
                let value: String = row.get(1).unwrap_or_default();
                let encrypted: Vec<u8> = row.get(2).unwrap_or_default();
                Ok((name, value, encrypted))
            })
            .map_err(|error| format!("读取客户端 Cookies 失败：{error}"))?;
        for item in mapped.flatten() {
            rows.push(item);
        }
    }
    // ③ 逐条解出明文值（明文列优先；空则解 os_crypt 密文）
    let mut values: Vec<(String, String)> = Vec::new();
    for (name, plain, encrypted) in rows {
        if !plain.trim().is_empty() {
            values.push((name, plain));
            continue;
        }
        if encrypted.is_empty() {
            continue;
        }
        // Chromium 密文形态：`v10` + nonce + ciphertext + tag，
        // 与 AutoClaw 的 `enc:` 字段同一拼法 —— 加前缀走它那条解密链
        let encoded = base64_encode(&encrypted);
        match crate::server::core::providers::autoclaw::crypto::decrypt_enc_value(
            &format!("enc:{encoded}"),
            Some(&aes_key),
        ) {
            Ok(text) => values.push((name, text)),
            Err(reason) => {
                return Err(format!("解密客户端登录态失败：{reason}"));
            }
        }
    }
    let pick = |target: &str| {
        values
            .iter()
            .find(|(name, _)| name == target)
            .map(|(_, value)| value.clone())
            .filter(|value| !value.trim().is_empty())
    };
    let access = pick(COOKIE_ACCESS_TOKEN);
    let refresh = pick(COOKIE_REFRESH_TOKEN);
    match (access, refresh) {
        (Some(access), refresh) => Ok(Some((access, refresh.unwrap_or_default()))),
        // 只有 refresh 也能用（`user/refresh` 能换出 access）
        (None, Some(refresh)) => Ok(Some((String::new(), refresh))),
        (None, None) => Ok(None),
    }
}

/// 非 Windows：DPAPI 不可用 —— 如实报不可导入（界面层已收起入口）
#[cfg(not(windows))]
pub fn read_desktop_credentials() -> Result<Option<(String, String)>, String> {
    Err("导入本机登录态仅在 Windows 可用（客户端登录态是 DPAPI 加密的）".to_string())
}

/// 标准 base64 编码（Chromium 的 `encrypted_value` 是无前缀二进制，
/// 拼回 `enc:` 形态时需要它；与 AutoClaw 的解码引擎互为逆操作）
#[cfg(windows)]
fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 设备号形态校验：只认 UUID（上游对非 UUID 回 40004）
    #[test]
    fn device_id_shape() {
        assert!(is_uuid_shaped("c9074ab1-208b-4255-8f71-09cc2ce32278"));
        assert!(!is_uuid_shaped("c9074ab1208b42558f7109cc2ce32278"));
        assert!(!is_uuid_shaped("abc"));
        assert!(!is_uuid_shaped(""));
        assert!(!is_uuid_shaped("1792175060134"));
        assert!(!is_uuid_shaped("{c9074ab1-208b-4255-8f71-09cc2ce32278}"));
    }

    /// 记录里有合法设备号就复用（身份连续性的前提）
    #[test]
    fn reuses_stored_device_id() {
        let stored = "c9074ab1-208b-4255-8f71-09cc2ce32278";
        assert_eq!(resolve_device_id(stored), stored);
        // 非法/缺失才新生成，且新生成的是 UUID 形态
        let generated = resolve_device_id("");
        assert!(is_uuid_shaped(&generated));
        let generated = resolve_device_id("not-a-uuid");
        assert!(is_uuid_shaped(&generated));
    }

    /// 从记录取凭证：读得到令牌与访客标记
    #[test]
    fn reads_credentials_from_record() {
        let record = json!({
            "id": "chatglm-abc",
            "provider": "chatglm",
            "auth": { "accessToken": "aaaaaaaaaaaaaaaaaaaa", "refreshToken": "bbbbbbbbbbbbbbbbbbbb" },
            "deviceId": "c9074ab1-208b-4255-8f71-09cc2ce32278",
            "isGuest": true,
            "uid": "6ad26bd46976b1c1ab1c2cb2",
            "nickname": "访客_573360"
        });
        let credentials = from_record(Some(&record)).expect("应当解析成功");
        assert_eq!(credentials.access_token, "aaaaaaaaaaaaaaaaaaaa");
        assert_eq!(credentials.device_id, "c9074ab1-208b-4255-8f71-09cc2ce32278");
        assert!(credentials.is_guest);
        assert_eq!(credentials.uid, "6ad26bd46976b1c1ab1c2cb2");
        // 尾段是令牌最后 8 个字符
        assert_eq!(credentials.token_tail(), "aaaaaaaa");
    }

    /// 记录缺令牌时报可读的 401（而不是 panic 或静默空转发）
    #[test]
    fn errors_when_token_missing() {
        let record = json!({ "id": "x", "auth": {} });
        let error = from_record(Some(&record)).expect_err("应当报错");
        assert_eq!(error.status_code, 401);
        let error = from_record(None).expect_err("应当报错");
        assert_eq!(error.status_code, 401);
    }

    /// JWT 声明读取：真实 token 的载荷形态（base64url，无填充）
    #[test]
    fn reads_jwt_claims() {
        // {"is_guest":true,"uid":"abc123","device_id":"d"} 的 base64url
        let payload = "eyJpc19ndWVzdCI6dHJ1ZSwidWlkIjoiYWJjMTIzIiwiZGV2aWNlX2lkIjoiZCJ9";
        let token = format!("header.{payload}.signature");
        assert!(token_is_guest(&token, false));
        assert_eq!(token_uid(&token), "abc123");
        // 解不出来时走 fallback（不 panic）
        assert!(token_is_guest("garbage", true));
        assert!(!token_is_guest("garbage", false));
        assert_eq!(token_uid("garbage"), "");
    }

    /// 兜底：access_token 为空时用 refresh_token 拼 Bearer
    #[test]
    fn bearer_falls_back_to_refresh() {
        let credentials = ChatglmCredentials {
            access_token: String::new(),
            refresh_token: "rrrrrrrrrrrrrrrrrrrr".to_string(),
            device_id: "c9074ab1-208b-4255-8f71-09cc2ce32278".to_string(),
            is_guest: false,
            uid: String::new(),
            nickname: String::new(),
        };
        assert_eq!(credentials.bearer(), "rrrrrrrrrrrrrrrrrrrr");
    }

    /// token_shape_ok 只拦残缺值
    #[test]
    fn token_shape_threshold() {
        assert!(!token_shape_ok("short"));
        assert!(token_shape_ok("aaaaaaaaaaaaaaaaaaaa"));
    }

    /// 令牌临期判定读的是 JWT 的 `exp` 声明（不靠猜）
    #[test]
    fn expiring_reads_jwt_exp() {
        // {"exp": 100} 的 base64url —— 早已过期 ⇒ 判临期
        let payload = "eyJleHAiOjEwMH0";
        let token = format!("h.{payload}.s");
        assert!(token_expiring_soon(&token));
        // 解不出 exp（畸形令牌）⇒ false（宁可漏刷也不凭猜测打上游）
        assert!(!token_expiring_soon("garbage"));
        assert!(!token_expiring_soon(""));
    }
}
