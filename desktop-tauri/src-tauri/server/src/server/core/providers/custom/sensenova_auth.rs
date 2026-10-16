//! 商汤日日新（SenseNova）**账号密码登录**：OAuth2 授权码 + PKCE 换 JWT。
//!
//! ── 为什么余额查询要自己登录（这一家的特别之处）──────────────
//! 余额接口（`platform.sensenova.cn` 的 tokenplan/pool-usage）只认控制台的
//! Bearer JWT，而**推理用的 API Key 换不到它** —— 两者是两套平面（开放平台
//! API Key 打推理，控制台 JWT 打账户/积分）。用户手上能提供的就是控制台账号
//! 密码，所以查询链路里带着一段登录（移植来源：OmniProxy 的
//! `server/src/utils/sensenovaAuth.ts`，逐步对读）。
//!
//! ── 流程（四步，与源实现逐条对齐）────────────────────────────
//!   1. `GET {platform}/oauth2/auth?...`（PKCE + state）—— 跟随跳转直到不带
//!      location，从**最终地址**的 query 里取 `login_challenge`；
//!   2. `POST {iam}/iam/authn/v1/auth/nova/login` `{username, password, challenge}`
//!      → 响应体里的 `redirect`；
//!   3. 从该 redirect 起继续跟随（带 Cookie），直到地址里出现 `code=`；
//!   4. `POST {platform}/oauth2/token`（authorization_code + code_verifier）
//!      → `access_token`（JWT）。
//! 站点常量照抄源实现的「网页端配置」：signinUrl / callbackUrl 都是
//! `platform.sensenova.cn`，IAM 域是 `iam.sensecoreapi.cn`，clientId = `nova`。
//!
//! ── 为什么自己跟跳转、自己管 Cookie ──────────────────────────
//! `auth_http::send_raw` 走 reqwest 的默认跳转策略（自动跟，但**读不到
//! Set-Cookie**），而这条链路每一步都要带着会话 Cookie 走下一跳。源实现用
//! `redirect: 'manual'` + 一个极简 Cookie Jar（只认 `name=value`，不处理
//! Domain / Path / 过期）——那套在这里逐条复刻：这条流程里的 Cookie 全是
//! 同一站点的会话票，按「同名覆盖」存即可，引入完整 Cookie 引擎得不偿失。
//!
//! ── JWT 的两个读数（过期判定与 tenant_id）────────────────────
//! 余额接口要 `?account_id=<JWT payload 的 ext.tenant_id>`，缓存判定要
//! `exp`（提前 60 秒视为过期，与源实现同口径）。两者都只读 payload 段，
//! **不校验签名**：这是网关自己拿到的票，不是需要防伪造的入口数据。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap / expect / panic。
//! 失败一律返回可读的 `String`（调用方拼进余额查询的错误文案）。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::server::core::egress;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::logging;

/// 控制台（平台）域：授权、换码、余额接口的**同一站点**
pub(super) const PLATFORM_BASE: &str = "https://platform.sensenova.cn";
/// 余额接口的基址（`/lite` 一级，与 token 平面不同路径）
pub(super) const PLATFORM_LITE_BASE: &str = "https://platform.sensenova.cn/lite";
/// IAM 域：账号密码登录接口
const IAM_BASE: &str = "https://iam.sensecoreapi.cn";
/// OAuth 客户端标识（网页端配置里的 clientId，逐字照抄）
const CLIENT_ID: &str = "nova";
/// 授权 scope（离线刷新那两项是网页端的原值）
const SCOPE: &str = "openid offline offline_access";
/// 跟跳转的步数上限（源实现 12）
const MAX_REDIRECTS: usize = 12;
/// 单步请求超时：登录链路是案例式动作（不像余额每轮都跑），给 20 秒
const STEP_TIMEOUT_MS: u64 = 20_000;
/// UA：登录页面对非常规 UA 有风控，照源实现固定一个普通浏览器 UA
const USER_AGENT: &str = "Mozilla/5.0";
/// 过期判定的提前量（秒）：与源实现同口径，避开边界竞态
const EXPIRY_SKEW_SECONDS: f64 = 60.0;

/// 一次登录的结果（只留余额链路要用的两样）
pub(super) struct LoginResult {
    /// 控制台 JWT
    pub access_token: String,
    /// 到期时刻（毫秒；解析不出 exp 时给 0 = 不确定，调用方当它不能缓存）
    pub expires_at_ms: i64,
}

/// JWT 是否还能用（`exp` 有效且距到期还有 [`EXPIRY_SKEW_SECONDS`] 以上）。
/// 解析不出 payload / 没有 `exp` 一律视为不能用 —— 宁多重登一次。
pub(super) fn jwt_reusable(jwt: &str) -> bool {
    let Some(payload) = jwt_payload(jwt) else { return false };
    let Some(exp) = payload.get("exp").and_then(Value::as_f64) else { return false };
    if !(exp > 0.0) {
        return false;
    }
    let now = logging::now_ms() as f64 / 1000.0;
    now < exp - EXPIRY_SKEW_SECONDS
}

/// JWT → `exp` 的毫秒时刻（解析不出给 0）。
pub(super) fn jwt_expires_at_ms(jwt: &str) -> i64 {
    jwt_payload(jwt)
        .and_then(|payload| payload.get("exp").and_then(Value::as_f64))
        .filter(|exp| *exp > 0.0)
        .map(|exp| (exp * 1000.0) as i64)
        .unwrap_or(0)
}

/// JWT → 账户 id（`ext.tenant_id`，回落顶层 `tenant_id`）。
///
/// 余额接口的 `account_id` 参数就是它 —— 缺了这一项查询没法发出去，
/// 所以调用方拿到 `None` 时按「登录成功但票不可用」处理。
pub(super) fn jwt_tenant_id(jwt: &str) -> Option<String> {
    let payload = jwt_payload(jwt)?;
    let candidate = payload
        .get("ext")
        .and_then(|ext| ext.get("tenant_id"))
        .or_else(|| payload.get("tenant_id"))?;
    let text = match candidate {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    };
    (!text.is_empty()).then_some(text)
}

/// JWT payload 段 → JSON（base64url，容错末尾 `=`；坏值给 None）
fn jwt_payload(jwt: &str) -> Option<Value> {
    let segment = jwt.split('.').nth(1)?;
    let text = segment.trim();
    if text.is_empty() {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('=').as_bytes())
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 账号密码登录，换回控制台 JWT（失败给人类可读的原因）。
pub(super) async fn login(
    username: &str,
    password: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<LoginResult, String> {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return Err("商汤日日新的账号 / 密码未配置".to_string());
    }
    let verifier = random_secret()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_hex()?;
    // 不自动跟转的出口：这条链路的每一步都要自己读 Location / Set-Cookie
    // （见 `egress::client_without_redirects`）
    let client = egress::client_without_redirects(proxy);
    let mut jar = CookieJar::default();

    // 1) 发起授权，跟随跳转直到页面停在登录页，取 login_challenge
    let auth_url = authorize_url(&challenge, &state)?;
    let followed = follow(&client, &auth_url, &mut jar).await?;
    if followed.status >= 400 {
        return Err(format!("商汤授权页返回 HTTP {}", followed.status));
    }
    let login_challenge = query_param(&followed.url, "login_challenge")
        .ok_or_else(|| "未能从商汤授权页取到登录 challenge".to_string())?;

    // 2) 密码登录（IAM 域）
    let body = json!({
        "username": username,
        "password": password,
        "challenge": login_challenge,
    });
    let response = client
        .post(format!("{IAM_BASE}/iam/authn/v1/auth/nova/login"))
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .header("Origin", PLATFORM_BASE)
        .header("Referer", format!("{PLATFORM_BASE}/login"))
        .timeout(std::time::Duration::from_millis(STEP_TIMEOUT_MS))
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("商汤登录接口请求失败：{}", egress::describe_error_detail(&error)))?;
    let status = response.status().as_u16();
    let payload = response_json(response).await;
    if status >= 400 || payload.is_none() {
        let message = payload
            .as_ref()
            .and_then(|value| value.get("message").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(format!("商汤账号登录失败：{}", shorten(&message)));
    }
    let redirect = payload
        .as_ref()
        .and_then(|value| value.get("redirect").and_then(Value::as_str))
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if redirect.is_empty() {
        return Err("商汤账号登录没有返回跳转地址".to_string());
    }

    // 3) 跟着 redirect 回到回调地址，取一次性 code
    let start = Url::parse(PLATFORM_BASE)
        .ok()
        .and_then(|base| base.join(&redirect).ok())
        .map(|url| url.to_string())
        .unwrap_or(redirect);
    let followed = follow(&client, &start, &mut jar).await?;
    let code = query_param(&followed.url, "code")
        .ok_or_else(|| "商汤登录没有拿到一次性授权码".to_string())?;

    // 4) 用 code 换 access_token
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("code", &code)
        .append_pair("redirect_uri", PLATFORM_BASE)
        .append_pair("code_verifier", &verifier)
        .append_pair("state", &state)
        .finish();
    let response = client
        .post(format!("{PLATFORM_BASE}/oauth2/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_millis(STEP_TIMEOUT_MS))
        .body(form)
        .send()
        .await
        .map_err(|error| format!("商汤换码请求失败：{}", egress::describe_error_detail(&error)))?;
    let status = response.status().as_u16();
    let payload = response_json(response).await;
    let access_token = payload
        .as_ref()
        .and_then(|value| value.get("access_token").and_then(Value::as_str))
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if status >= 400 || access_token.is_empty() {
        let message = payload
            .as_ref()
            .and_then(|value| {
                value
                    .get("error_description")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("error").and_then(Value::as_str))
            })
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(format!("商汤换取登录票失败：{}", shorten(&message)));
    }
    Ok(LoginResult {
        expires_at_ms: jwt_expires_at_ms(&access_token),
        access_token,
    })
}

/// 授权地址（参数照源实现：response_type / client_id / PKCE / redirect_uri /
/// scope / state / lang）。
fn authorize_url(challenge: &str, state: &str) -> Result<String, String> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", challenge)
        .append_pair("redirect_uri", PLATFORM_BASE)
        .append_pair("scope", SCOPE)
        .append_pair("state", state)
        .append_pair("lang", "zh-CN")
        .finish();
    Ok(format!("{PLATFORM_BASE}/oauth2/auth?{query}"))
}

/// 跟着 3xx 走到底（每步都带 Cookie、每步都收 Set-Cookie）。
///
/// 返回最终那一步的状态码与地址；**不做**「非 2xx 即错」的判定 ——
/// 调用方按自己的语义读状态（授权页停在 4xx 与正常停页要分开报）。
async fn follow(
    client: &reqwest::Client,
    start: &str,
    jar: &mut CookieJar,
) -> Result<Followed, String> {
    let mut current = start.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let mut request = client
            .get(&current)
            .header("User-Agent", USER_AGENT)
            .timeout(std::time::Duration::from_millis(STEP_TIMEOUT_MS));
        let cookie = jar.header();
        if !cookie.is_empty() {
            request = request.header("Cookie", cookie);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("商汤登录跳转请求失败：{}", egress::describe_error_detail(&error)))?;
        jar.capture(&response);
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        match location {
            Some(location) if (300..400).contains(&status) => {
                current = resolve_url(&current, &location)?;
            }
            _ => {
                let url = Url::parse(&current).ok();
                return Ok(Followed { status, url });
            }
        }
    }
    Err("商汤登录跳转次数过多（已超过上限）".to_string())
}

/// 相对 / 绝对 location → 绝对地址（畸形值报错，不静默截断）
fn resolve_url(current: &str, location: &str) -> Result<String, String> {
    let base = Url::parse(current).map_err(|_| "商汤登录跳转地址无法解析".to_string())?;
    if let Ok(absolute) = Url::parse(location) {
        return Ok(absolute.to_string());
    }
    base.join(location)
        .map(|url| url.to_string())
        .map_err(|_| "商汤登录跳转地址不是合法 URL".to_string())
}

/// 读一个响应体里的 JSON（读不出 / 不是 JSON 给 None；状态码由调用方先读走）
async fn response_json(response: reqwest::Response) -> Option<Value> {
    let text = response.text().await.ok()?;
    if text.trim().is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(&text).ok()
}

/// URL 里的一个 query 参数（取不出给 None）。值做一次 `+` → 空格与
/// percent-decode —— `code` / `login_challenge` 里出现转义字符时按原文读。
fn query_param(url: &Option<Url>, name: &str) -> Option<String> {
    let url = url.as_ref()?;
    let value = url
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.to_string())?;
    let text = value.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// 极简 Cookie Jar：只认 `name=value`，同名覆盖（见模块头）。
#[derive(Default)]
struct CookieJar {
    cookies: Vec<(String, String)>,
}

impl CookieJar {
    fn capture(&mut self, response: &reqwest::Response) {
        for header in response.headers().get_all("set-cookie") {
            let Ok(text) = header.to_str() else { continue };
            let pair = text.split(';').next().unwrap_or("");
            let Some((name, value)) = pair.split_once('=') else { continue };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let value = value.trim().to_string();
            match self.cookies.iter_mut().find(|(known, _)| known == name) {
                Some(slot) => slot.1 = value,
                None => self.cookies.push((name.to_string(), value)),
            }
        }
    }

    fn header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// 最终停在哪（状态码 + 地址；地址解析失败时是 None，供 query 参数读取）
struct Followed {
    status: u16,
    url: Option<Url>,
}

/// PKCE code_verifier：32 字节随机 → base64url（43 字符，落在 43~128 之间）
fn random_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| "无法生成安全的随机串（系统随机源不可用）".to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// state：16 字节随机 → hex（源实现 `randomBytes(16).toString('hex')`）
fn random_hex() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| "无法生成安全的随机串（系统随机源不可用）".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// 文案截断（上游的错误说明可能是一整页 HTML）
fn shorten(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 160 {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(157).collect();
    format!("{head}...")
}
