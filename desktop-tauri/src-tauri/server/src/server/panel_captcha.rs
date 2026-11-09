use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::server::config::{self, captcha::CaptchaSettings};
use crate::server::logging;

const SITEVERIFY: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
pub(crate) const ACTION: &str = "agent2api_panel";

pub(crate) async fn widget() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (axum::http::header::CACHE_CONTROL, "no-store"),
            (axum::http::header::REFERRER_POLICY, "no-referrer"),
            (axum::http::header::CONTENT_SECURITY_POLICY, "default-src 'none'; script-src 'unsafe-inline' https://challenges.cloudflare.com; style-src 'unsafe-inline'; frame-src https://challenges.cloudflare.com; connect-src 'self' https://challenges.cloudflare.com; base-uri 'none'; form-action 'none'"),
        ],
        include_str!("panel_captcha/turnstile.html"),
    ).into_response()
}

#[derive(Deserialize)]
struct Verification {
    success: bool,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    action: String,
    #[serde(default, rename = "error-codes")]
    errors: Vec<String>,
}

fn client() -> Result<&'static reqwest::Client, (i32, String)> {
    static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
        })
        .as_ref()
        .map_err(|_| (503, "Turnstile 验证服务初始化失败，请联系管理员".into()))
}

pub(crate) async fn verify(payload: &Value, local_panel: bool) -> Result<(), (i32, String)> {
    let config = config::current();
    if !config.captcha_enabled(local_panel) {
        return Ok(());
    }
    let settings = CaptchaSettings::from_config(&config);
    if settings.validate().is_err() {
        return Err((503, "机器人校验配置不完整，请联系管理员".into()));
    }
    let token = payload
        .get("captcha")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| (400, "请完成人机验证后重试".to_string()))?;
    if settings.provider == "altcha" {
        return crate::server::altcha::verify(token).map_err(|error| (400, error));
    }
    if token.len() > 2048 {
        return Err((400, "Turnstile 验证令牌无效，请重新验证".into()));
    }
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let _permit = SLOTS
        .get_or_init(|| Arc::new(Semaphore::new(32)))
        .try_acquire()
        .map_err(|_| (429, "机器人校验请求较多，请稍后重试".to_string()))?;
    let secret = settings
        .secret()
        .map_err(|_| (503, "Turnstile 密钥不可用，请联系管理员".to_string()))?;
    let response = client()?
        .post(SITEVERIFY)
        .form(&[("secret", secret.as_str()), ("response", token)])
        .send()
        .await
        .map_err(|error| {
            let reason = if error.is_timeout() {
                "请求超时"
            } else {
                "网络连接失败"
            };
            logging::log("[Security]", &format!("Turnstile 服务端验证{reason}"));
            (
                503,
                format!("无法连接 Turnstile 验证服务（{reason}），请稍后重新验证"),
            )
        })?;
    if !response.status().is_success() {
        logging::log(
            "[Security]",
            &format!("Turnstile 验证服务返回 HTTP {}", response.status().as_u16()),
        );
        return Err((503, "Turnstile 验证服务暂不可用，请稍后重新验证".into()));
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| (503, "读取 Turnstile 验证结果失败，请重新验证".to_string()))?
    {
        if bytes.len().saturating_add(chunk.len()) > 64 * 1024 {
            return Err((503, "Turnstile 验证结果异常，请联系管理员".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let result: Verification = serde_json::from_slice(&bytes)
        .map_err(|_| (503, "Turnstile 验证结果异常，请联系管理员".to_string()))?;
    if !result.success {
        if result
            .errors
            .iter()
            .any(|code| code == "timeout-or-duplicate")
        {
            return Err((400, "人机验证已过期或已使用，请重新验证".into()));
        }
        if result.errors.iter().any(|code| {
            matches!(
                code.as_str(),
                "missing-input-secret" | "invalid-input-secret"
            )
        }) {
            logging::log(
                "[Security]",
                "Turnstile 拒绝服务端密钥，请检查机器人校验设置",
            );
            return Err((503, "Turnstile 服务端密钥配置有误，请联系管理员".into()));
        }
        if result.errors.iter().any(|code| code == "internal-error") {
            return Err((503, "Turnstile 验证服务暂不可用，请稍后重新验证".into()));
        }
        return Err((400, "Turnstile 人机验证未通过，请重新验证".into()));
    }
    let hostname = config::captcha::normalize_hostname(&result.hostname).ok();
    if result.action != ACTION
        || !hostname.is_some_and(|hostname| settings.hostnames.contains(&hostname))
    {
        logging::log("[Security]", "Turnstile 验证来源或操作标记不匹配");
        return Err((
            400,
            "Turnstile 验证来源不匹配，请检查允许的域名并重新验证".into(),
        ));
    }
    let current = config::current();
    let active = CaptchaSettings::from_config(&current);
    if current.raw().get(config::captcha::KEY_PANEL_CAPTCHA)
        != config.raw().get(config::captcha::KEY_PANEL_CAPTCHA)
        || active.provider != settings.provider
    {
        return Err((409, "机器人校验设置已变更，请刷新页面后重新验证".into()));
    }
    Ok(())
}
