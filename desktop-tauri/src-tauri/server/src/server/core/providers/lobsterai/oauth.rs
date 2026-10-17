//! LobsterAI 登录与续期：**浏览器授权 + 本机 loopback 回调 + 换码**。
//!
//! ── 四步 ────────────────────────────────────────────────────
//! ```text
//!   1. 本网关按 state 登记一轮登录，产出授权地址：
//!        {portal}/portal#/login?source=electron
//!                    &redirect_uri=http://127.0.0.1:<本网关端口>/auth/callback
//!                    &state=<state>
//!   2. 用户在浏览器里授权（手机号 / 微信），门户 302 回本机 loopback：
//!        GET /auth/callback?code=<code>&state=<state>
//!   3. POST {api}/api/auth/exchange   **不带 Authorization**
//!        体 { authCode, firstKeyfrom, latestKeyfrom, uuid, version }
//!   4. 落账号（把 uuid 与两个 keyfrom 一起存 —— 见 `credentials.rs` 的模块头）
//! ```
//!
//! ── 回调路径为什么不用 `-lobsterai` 后缀 ──────────────────────
//! 本仓其余几家自作主张的回调路径都带家名后缀（`/auth/callback-accio` 等），
//! 一眼能看出归属。这一家**例外**：授权地址里的 `redirect_uri` 是我们拼给门户的，
//! 门户会按它回落 —— 参考实现用的就是裸 `/auth/callback`（`lobsterai.ts:47`），
//! 而门户是否对回调路径有白名单**没有实测支撑**。所以这里跟随参考实现那条
//! 已验证的形状，不为了命名风格去赌一次未经验证的兼容性。该路径目前在本仓未被占用。
//!
//! ── 续期：**必须原样回传载荷件，且 `latestKeyfrom` 永不更新** ─────
//! `POST /api/auth/refresh` 同样**不带 Authorization**，体是
//! `{ firstKeyfrom, latestKeyfrom, version, [uuid], [userId], refreshToken }`。
//! 响应带新 `refreshToken` 时才轮换；`latestKeyfrom` **不覆盖**（照实测的形态，
//! 见 `credentials.rs` 的模块头）。
//!
//! ── 两种「成功状态下的失败」 ──────────────────────────────────
//!   1. 业务信封 `code != 0` → 按错误处理；
//!   2. 信封 `code == 0` 但 `data` 为 null → **令牌已失效**（参考实现实测）。
//!      这一条最容易漏：只看 HTTP 状态或只看 `code` 都会把它当成成功。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use crate::server::core::auth_http::send_raw;
use crate::server::core::upstream::request::new_request_id;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::credentials::LobsterCredentials;
use super::endpoints::{
    CALLBACK_PATH, EXCHANGE_PATH, REQUEST_TIMEOUT_MS, REFRESH_PATH, USER_AGENT, api_base,
    portal_base, urlencode,
};

/// 本网关的 loopback 回调路径（见模块头：跟随参考实现的裸路径）
pub const LOOPBACK_CALLBACK_PATH: &str = CALLBACK_PATH;

/// pending 表有效期：与本仓登录任务的 5 分钟超时同一量级
const PENDING_TTL_MS: i64 = 6 * 60 * 1000;

/// 本网关监听端口（`ServerState::bootstrap` 时写入一次）。
///
/// `ProviderAdapter::build_login_url()` 是同步无参的（trait 契约），拿不到
/// `ServerState`；而授权地址里必须拼本机回调地址。端口在进程生命周期内不变，
/// 因此开机写一次、之后只读 —— 与 `accio::oauth` 同一手法，理由也同一份。
static LOOPBACK_PORT: OnceLock<u16> = OnceLock::new();

/// 记录本进程监听端口（重复调用无害）
pub fn set_loopback_port(port: u16) {
    let _ = LOOPBACK_PORT.set(port);
}

/// 本机回调基址（端口未知时 None，错误由上层文案说清）
pub fn loopback_base() -> Option<String> {
    LOOPBACK_PORT
        .get()
        .map(|port| format!("http://127.0.0.1:{port}"))
}

/// 本机回调完整地址（换码时要原样回传的 `redirect_uri`）
pub fn redirect_uri() -> Option<String> {
    loopback_base().map(|base| format!("{base}{LOOPBACK_CALLBACK_PATH}"))
}

/// 一轮待完成的登录（回调进来时按 state 取回）
pub struct PendingLogin {
    created_at: i64,
}

fn pending_table() -> &'static Mutex<HashMap<String, PendingLogin>> {
    static TABLE: OnceLock<Mutex<HashMap<String, PendingLogin>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_pending() -> std::sync::MutexGuard<'static, HashMap<String, PendingLogin>> {
    match pending_table().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 清掉过期的 pending（顺手做，不需要独立定时器）
fn sweep(table: &mut HashMap<String, PendingLogin>) {
    let now = logging::now_ms();
    table.retain(|_, entry| now - entry.created_at < PENDING_TTL_MS);
}

/// 授权地址：`{portal}/portal#/login?source=electron&redirect_uri=…&state=…`
pub fn build_authorize_url(return_url: &str, state: &str) -> String {
    format!(
        "{}/portal#/login?source=electron&redirect_uri={}&state={}",
        portal_base(),
        urlencode(return_url),
        urlencode(state)
    )
}

/// 登记一轮登录，返回 `(auth_url, state)`。
///
/// 同一时刻只保证「这一轮」可被回调命中：旧轮次留在表里到期自然清掉（用户可能
/// 开过好几个标签页），但**同一 state 只能被兑一次**（见 [`take_pending`]）。
pub fn begin_login(return_url: &str) -> (String, String) {
    let state = new_request_id().replace('-', "");
    let auth_url = build_authorize_url(return_url, &state);
    let mut table = lock_pending();
    sweep(&mut table);
    table.insert(
        state.clone(),
        PendingLogin {
            created_at: logging::now_ms(),
        },
    );
    (auth_url, state)
}

/// 取走一轮登录（回调进来时调用；取走即失效，重复回调拿不到第二次）
pub fn take_pending(state: &str) -> Option<PendingLogin> {
    let mut table = lock_pending();
    sweep(&mut table);
    table.remove(state)
}

/// 丢弃一轮登录（取消 / 失败收尾）
pub fn drop_pending(state: &str) {
    let mut table = lock_pending();
    table.remove(state);
}

/// 从回调的 query 里取 `(code, state)`。
///
/// 门户可能带回额外参数（微信授权那条链会挂 `scope` 之类），所以按键找而不是按位置切。
pub fn parse_callback(query: &str) -> Option<(String, String)> {
    let mut code = String::new();
    let mut state = String::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        let decoded = urldecode(value);
        match key {
            "code" => code = decoded,
            "state" => state = decoded,
            _ => {}
        }
    }
    if code.is_empty() || state.is_empty() {
        return None;
    }
    Some((code, state))
}

/// 极小的 query 解码（`+` 当空格，`%XX` 还原）。
///
/// 不引依赖：回调只有两三个参数，且值都是可见 ASCII 记号。
fn urldecode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 一次令牌包的解析结果（换码与续期共用）
#[derive(Clone, Debug, Default)]
pub struct TokenBundle {
    /// `accessToken`
    pub access_token: String,
    /// `refreshToken`（可能为空：续期响应不带新 token 时保留旧的）
    pub refresh_token: String,
    /// `expiresIn`（秒）
    pub expires_in: i64,
    /// 上游用户 id（`user.id`）—— 账号 id 与 keyfrom 都要它
    pub uid: String,
    /// `user.userId`
    pub user_id: String,
    /// `user.nickname`
    pub nickname: String,
}

/// 解业务信封 `{code, msg, data}`。
///
/// `code == 0` 且 `data` **非 null** 才算成功 —— `data: null` 配 `code: 0`
/// 是「令牌子失效」的形态（参考实现实测），把它当成功会让上层拿空令牌去发请求。
pub fn unwrap_envelope(payload: &Value, what: &str) -> Result<Value, GatewayError> {
    let code = payload.get("code").and_then(Value::as_i64).unwrap_or(-1);
    let message = payload
        .get("msg")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if code != 0 {
        let detail = if message.is_empty() {
            format!("code={code}")
        } else {
            message.to_string()
        };
        return Err(GatewayError::with_status(
            502,
            format!("LobsterAI {what}失败：{detail}"),
        ));
    }
    match payload.get("data") {
        Some(Value::Null) | None => Err(GatewayError::with_status(
            401,
            format!("LobsterAI {what}未返回数据（登录态可能已失效），请重新登录"),
        )),
        Some(data) => Ok(data.clone()),
    }
}

/// 从令牌信封的 `data` 里解出 [`TokenBundle`]。
pub fn parse_token_bundle(data: &Value) -> Result<TokenBundle, GatewayError> {
    let access_token = data
        .get("accessToken")
        .or_else(|| data.get("access_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if access_token.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "LobsterAI 换码响应缺少 accessToken",
        ));
    }
    let user = data.get("user").cloned().unwrap_or(Value::Null);
    let str_of = |value: &Value, key: &str| -> String {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    Ok(TokenBundle {
        access_token,
        refresh_token: {
            let token = str_of(data, "refreshToken");
            if token.is_empty() {
                str_of(data, "refresh_token")
            } else {
                token
            }
        },
        expires_in: data
            .get("expiresIn")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .unwrap_or(0),
        uid: {
            let id = str_of(&user, "id");
            if id.is_empty() {
                str_of(&user, "yid")
            } else {
                id
            }
        },
        user_id: str_of(&user, "userId"),
        nickname: str_of(&user, "nickname"),
    })
}

/// 用授权码换令牌（**不带 Authorization**）。
///
/// 体固定五个字段：`authCode` + 两个 keyfrom + `uuid` + `version`。
/// `uuid` 由调用方生成并**落盘**（续期要它）—— 它不是凭据，是这一台「客户端」的标识。
pub async fn exchange(
    auth_code: &str,
    uuid: &str,
    first_keyfrom: &str,
    latest_keyfrom: &str,
    client_version: &str,
) -> Result<TokenBundle, GatewayError> {
    let body = json!({
        "authCode": auth_code,
        "firstKeyfrom": first_keyfrom,
        "latestKeyfrom": latest_keyfrom,
        "uuid": uuid,
        "version": client_version,
    });
    let payload = post_anonymous(EXCHANGE_PATH, &body, "换码").await?;
    let data = unwrap_envelope(&payload, "换码")?;
    parse_token_bundle(&data)
}

/// 续期（**不带 Authorization**）：载荷件原样回传 + `refreshToken`。
pub async fn refresh(
    credentials: &LobsterCredentials,
    client_version: &str,
) -> Result<TokenBundle, GatewayError> {
    if !credentials.can_refresh() {
        return Err(GatewayError::with_status(
            401,
            "LobsterAI 账号缺少续期所需的载荷件（uuid / keyfrom 之一为空），请重新登录",
        ));
    }
    let mut body = serde_json::Map::new();
    for (key, value) in credentials.keyfrom_fields(client_version) {
        body.insert(key.to_string(), Value::String(value));
    }
    body.insert(
        "refreshToken".to_string(),
        Value::String(credentials.refresh_token.clone()),
    );
    let payload = post_anonymous(REFRESH_PATH, &Value::Object(body), "续期").await?;
    let data = unwrap_envelope(&payload, "续期")?;
    parse_token_bundle(&data)
}

/// 发一次匿名 POST（换码与续期都不带 Authorization）
async fn post_anonymous(path: &str, body: &Value, what: &str) -> Result<Value, GatewayError> {
    let url = format!("{}{}", api_base(), path);
    let response = send_raw(
        "POST",
        &url,
        Some(body),
        &[
            ("accept".to_string(), "application/json".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            ("user-agent".to_string(), USER_AGENT.to_string()),
        ],
        None,
        Some(REQUEST_TIMEOUT_MS),
    )
    .await
    .map_err(|error| {
        if error.is_timeout() {
            GatewayError::with_status(504, format!("LobsterAI {what}超时"))
        } else {
            GatewayError::with_status(502, format!("LobsterAI {what}请求失败: {error}"))
        }
    })?;
    let payload = response.payload.ok_or_else(|| {
        GatewayError::with_status(502, format!("LobsterAI {what}响应不是 JSON"))
    })?;
    // 401/403 直接归成登录态问题，别让信封里的文案把方向带偏
    if response.status == 401 || response.status == 403 {
        return Err(GatewayError::with_status(
            401,
            format!("LobsterAI {what}被拒（HTTP {}），请重新登录", response.status),
        ));
    }
    if !response.ok {
        return Err(GatewayError::with_status(
            response.status as i32,
            format!("LobsterAI {what}返回 HTTP {}", response.status),
        ));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 授权地址：门户的 hash 路由 + 两个参数都要转义。
    #[test]
    fn authorize_url_points_at_the_portal_hash_route() {
        let url = build_authorize_url("http://127.0.0.1:3066/auth/callback", "abc123");
        assert!(url.starts_with(&format!("{}/portal#/login?", portal_base())), "{url}");
        assert!(url.contains("source=electron"), "{url}");
        assert!(
            url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A3066%2Fauth%2Fcallback"),
            "{url}"
        );
        assert!(url.contains("state=abc123"), "{url}");
    }

    /// 回调解析：按键找（门户可能带额外参数），两个都必须有。
    #[test]
    fn callback_parsing_is_key_based() {
        assert_eq!(
            parse_callback("code=C1&state=S1"),
            Some(("C1".to_string(), "S1".to_string()))
        );
        // 顺序不同 + 多带参数
        assert_eq!(
            parse_callback("scope=all&state=S1&code=C1&lang=zh"),
            Some(("C1".to_string(), "S1".to_string()))
        );
        // 少一个就不认（宁可报「回调不完整」，也不要拿半个参数去换码）
        assert_eq!(parse_callback("code=C1"), None);
        assert_eq!(parse_callback("state=S1"), None);
        assert_eq!(parse_callback(""), None);
        // 转义还原
        assert_eq!(
            parse_callback("code=a%2Bb&state=s%20x"),
            Some(("a+b".to_string(), "s x".to_string()))
        );
    }

    /// 信封：`code != 0` 是失败；**`code == 0` 但 `data` 为 null 也是失败**
    /// （令牌已失效的形态，最容易被漏成成功）。
    #[test]
    fn envelope_rejects_both_non_zero_code_and_null_data() {
        assert!(unwrap_envelope(&json!({"code": 0, "data": {"a": 1}}), "换码").is_ok());

        let error = unwrap_envelope(&json!({"code": 7, "msg": "签名不对"}), "换码")
            .expect_err("code!=0 应报错");
        assert!(error.message.contains("签名不对"), "{:?}", error.message);

        let error = unwrap_envelope(&json!({"code": 0, "data": null}), "续期")
            .expect_err("data=null 应报错");
        assert_eq!(error.status_code, 401);
        assert!(error.message.contains("重新登录"));

        // 缺 data 键同样按失效处理，不 panic
        assert!(unwrap_envelope(&json!({"code": 0}), "续期").is_err());
        // 连 code 都没有 ⇒ 不当成功
        assert!(unwrap_envelope(&json!({"data": {}}), "续期").is_err());
    }

    /// 令牌包：缺 accessToken 报错；refreshToken 两种拼法都认；uid 退回 yid。
    #[test]
    fn token_bundle_parsing_is_tolerant_but_requires_access_token() {
        let data = json!({
            "accessToken": "at", "refreshToken": "rt", "expiresIn": 2592000,
            "user": {"id": "u1", "userId": "42", "nickname": "虾"}
        });
        let bundle = parse_token_bundle(&data).expect("应可解析");
        assert_eq!(bundle.access_token, "at");
        assert_eq!(bundle.refresh_token, "rt");
        assert_eq!(bundle.expires_in, 2_592_000);
        assert_eq!(bundle.uid, "u1");
        assert_eq!(bundle.user_id, "42");
        assert_eq!(bundle.nickname, "虾");

        // 没有 user.id 时退回 yid
        let bundle = parse_token_bundle(&json!({"accessToken": "at", "user": {"yid": "y9"}}))
            .expect("应可解析");
        assert_eq!(bundle.uid, "y9");
        assert!(bundle.refresh_token.is_empty());

        assert!(parse_token_bundle(&json!({"refreshToken": "rt"})).is_err());
    }

    /// pending 表：取走即失效（重复回调拿不到第二次）。
    #[test]
    fn pending_is_single_use() {
        let (_url, state) = begin_login("http://127.0.0.1:1/auth/callback");
        assert!(take_pending(&state).is_some());
        assert!(take_pending(&state).is_none(), "同一 state 不该能被兑第二次");
        // 丢弃幂等
        drop_pending("never-existed");
    }
}
