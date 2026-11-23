//! 管理面板 OAuth 授权状态机、PKCE、令牌交换、用户信息拉取与一次性会话票据。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{record_bound_github, record_bound_google, PanelOauthSettings};
use crate::server::access::{self, IssuedSession};
use crate::server::core::egress;
use crate::server::core::notifications;
use crate::server::logging;

const STATE_TTL: Duration = Duration::from_secs(10 * 60);
const TICKET_TTL: Duration = Duration::from_secs(90);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OauthProvider {
    Google,
    Github,
}

impl OauthProvider {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "google" => Some(Self::Google),
            "github" => Some(Self::Github),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::Github => "GitHub",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OauthMode {
    Login,
    Bind,
}

#[derive(Clone, Debug)]
pub enum OauthFlowOutcome {
    Pending,
    Done {
        ticket: Option<String>,
        identity: String,
        provider: &'static str,
    },
    Error(String),
}

#[derive(Clone, Debug)]
struct OauthFlowEntry {
    provider: OauthProvider,
    mode: OauthMode,
    popup: bool,
    next: String,
    redirect_uri: String,
    code_verifier: String,
    created_at: Instant,
    outcome: OauthFlowOutcome,
}

static OAUTH_STATES: OnceLock<Mutex<HashMap<String, OauthFlowEntry>>> = OnceLock::new();
static OAUTH_TICKETS: OnceLock<Mutex<HashMap<String, (IssuedSession, Instant)>>> = OnceLock::new();

fn states_map() -> &'static Mutex<HashMap<String, OauthFlowEntry>> {
    OAUTH_STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn tickets_map() -> &'static Mutex<HashMap<String, (IssuedSession, Instant)>> {
    OAUTH_TICKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn generate_pkce() -> (String, String) {
    let mut random_bytes = [0u8; 32];
    let _ = getrandom::getrandom(&mut random_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

pub fn resolve_callback_url(settings: &PanelOauthSettings, request_origin: &str) -> Result<String, String> {
    let base = if !settings.public_base_url.trim().is_empty() {
        settings.public_base_url.trim().trim_end_matches('/').to_string()
    } else {
        request_origin.trim().trim_end_matches('/').to_string()
    };
    if base.is_empty() || (!base.starts_with("http://") && !base.starts_with("https://")) {
        return Err("无法确定 OAuth 回调地址，请在「设置 → 账户 → 第三方登录」中填写面板外部访问地址".to_string());
    }
    Ok(format!("{base}/api/panel/oauth/callback"))
}

pub fn start_flow(
    provider: OauthProvider,
    mode: OauthMode,
    popup: bool,
    next: &str,
    request_origin: &str,
) -> Result<(String, String), String> {
    if mode == OauthMode::Login && !access::admin_registered() {
        return Err("尚未注册管理员账号，请先完成首次管理员注册".to_string());
    }
    let settings = PanelOauthSettings::current();
    let redirect_uri = resolve_callback_url(&settings, request_origin)?;
    let (verifier, challenge) = generate_pkce();
    let state = access::random_hex(20);

    let authorize_url = match provider {
        OauthProvider::Google => {
            if mode == OauthMode::Login && !settings.google.enabled {
                return Err("Google 登录未启用".to_string());
            }
            if settings.google.client_id.is_empty() {
                return Err("请先填写 Google Client ID".to_string());
            }
            let _ = settings.google_secret()?;
            format!(
                "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&code_challenge={}&code_challenge_method=S256&prompt=select_account",
                url_encode(&settings.google.client_id),
                url_encode(&redirect_uri),
                url_encode("openid email profile"),
                url_encode(&state),
                url_encode(&challenge),
            )
        }
        OauthProvider::Github => {
            if mode == OauthMode::Login && !settings.github.enabled {
                return Err("GitHub 登录未启用".to_string());
            }
            if settings.github.client_id.is_empty() {
                return Err("请先填写 GitHub Client ID".to_string());
            }
            let _ = settings.github_secret()?;
            format!(
                "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
                url_encode(&settings.github.client_id),
                url_encode(&redirect_uri),
                url_encode("read:user user:email"),
                url_encode(&state),
                url_encode(&challenge),
            )
        }
    };

    let safe_next = if next == "/m" { "/m".to_string() } else { String::new() };
    let mut guard = states_map().lock().unwrap_or_else(|p| p.into_inner());
    guard.retain(|_, entry| entry.created_at.elapsed() < STATE_TTL);
    guard.insert(
        state.clone(),
        OauthFlowEntry {
            provider,
            mode,
            popup,
            next: safe_next,
            redirect_uri,
            code_verifier: verifier,
            created_at: Instant::now(),
            outcome: OauthFlowOutcome::Pending,
        },
    );
    Ok((state, authorize_url))
}

pub fn poll_state(state: &str) -> Value {
    let mut guard = states_map().lock().unwrap_or_else(|p| p.into_inner());
    guard.retain(|_, entry| entry.created_at.elapsed() < STATE_TTL);
    let Some(entry) = guard.get(state) else {
        return json!({
            "status": "error",
            "error": "授权会话已过期或不存在，请重新发起",
        });
    };
    match &entry.outcome {
        OauthFlowOutcome::Pending => json!({ "status": "pending" }),
        OauthFlowOutcome::Done {
            ticket,
            identity,
            provider,
        } => json!({
            "status": "done",
            "ticket": ticket,
            "identity": identity,
            "provider": provider,
        }),
        OauthFlowOutcome::Error(err) => json!({
            "status": "error",
            "error": err,
        }),
    }
}

pub fn take_ticket(ticket: &str) -> Option<IssuedSession> {
    let mut guard = tickets_map().lock().unwrap_or_else(|p| p.into_inner());
    guard.retain(|_, (_, created)| created.elapsed() < TICKET_TTL);
    guard.remove(ticket).map(|(session, _)| session)
}

fn issue_ticket(session: IssuedSession) -> String {
    let ticket = access::random_hex(24);
    let mut guard = tickets_map().lock().unwrap_or_else(|p| p.into_inner());
    guard.retain(|_, (_, created)| created.elapsed() < TICKET_TTL);
    guard.insert(ticket.clone(), (session, Instant::now()));
    ticket
}

pub struct CallbackResult {
    pub mode: OauthMode,
    pub popup: bool,
    pub next: String,
    pub provider_label: &'static str,
    pub session: Option<IssuedSession>,
    pub ticket: Option<String>,
    pub identity: Option<String>,
    pub error: Option<String>,
}

pub async fn complete_callback(
    state: &str,
    code: Option<&str>,
    oauth_error: Option<&str>,
    ip: &str,
) -> CallbackResult {
    let entry = {
        let mut guard = states_map().lock().unwrap_or_else(|p| p.into_inner());
        guard.retain(|_, e| e.created_at.elapsed() < STATE_TTL);
        guard.get(state).cloned()
    };
    let Some(entry) = entry else {
        return CallbackResult {
            mode: OauthMode::Login,
            popup: false,
            next: String::new(),
            provider_label: "OAuth",
            session: None,
            ticket: None,
            identity: None,
            error: Some("OAuth 授权状态无效或已过期，请返回重新发起".to_string()),
        };
    };

    let set_outcome = |outcome: OauthFlowOutcome| {
        let mut guard = states_map().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(slot) = guard.get_mut(state) {
            slot.outcome = outcome;
        }
    };

    if let Some(err) = oauth_error.filter(|s| !s.trim().is_empty()) {
        let msg = format!("{} 授权已取消或被拒绝：{err}", entry.provider.label());
        set_outcome(OauthFlowOutcome::Error(msg.clone()));
        return CallbackResult {
            mode: entry.mode,
            popup: entry.popup,
            next: entry.next,
            provider_label: entry.provider.label(),
            session: None,
            ticket: None,
            identity: None,
            error: Some(msg),
        };
    }

    let Some(code) = code.map(str::trim).filter(|s| !s.is_empty()) else {
        let msg = format!("{} 回调缺少授权码（code）", entry.provider.label());
        set_outcome(OauthFlowOutcome::Error(msg.clone()));
        return CallbackResult {
            mode: entry.mode,
            popup: entry.popup,
            next: entry.next,
            provider_label: entry.provider.label(),
            session: None,
            ticket: None,
            identity: None,
            error: Some(msg),
        };
    };

    let settings = PanelOauthSettings::current();
    match entry.provider {
        OauthProvider::Google => {
            match exchange_google(&settings, code, &entry.redirect_uri, &entry.code_verifier).await {
                Ok(profile) => {
                    let display = if !profile.email.is_empty() {
                        profile.email.clone()
                    } else if !profile.name.is_empty() {
                        profile.name.clone()
                    } else {
                        profile.sub.clone()
                    };
                    match entry.mode {
                        OauthMode::Bind => match record_bound_google(&profile.sub, &profile.email, &profile.name) {
                            Ok(_) => {
                                logging::log("[Security]", &format!("✅ 已绑定 Google 登录账号：{display}"));
                                set_outcome(OauthFlowOutcome::Done {
                                    ticket: None,
                                    identity: display.clone(),
                                    provider: "Google",
                                });
                                CallbackResult {
                                    mode: entry.mode,
                                    popup: entry.popup,
                                    next: entry.next,
                                    provider_label: "Google",
                                    session: None,
                                    ticket: None,
                                    identity: Some(display),
                                    error: None,
                                }
                            }
                            Err(err) => {
                                set_outcome(OauthFlowOutcome::Error(err.clone()));
                                CallbackResult {
                                    mode: entry.mode,
                                    popup: entry.popup,
                                    next: entry.next,
                                    provider_label: "Google",
                                    session: None,
                                    ticket: None,
                                    identity: None,
                                    error: Some(err),
                                }
                            }
                        },
                        OauthMode::Login => {
                            if !access::admin_registered() {
                                let msg = "尚未注册管理员账号，请先完成首次注册".to_string();
                                set_outcome(OauthFlowOutcome::Error(msg.clone()));
                                return CallbackResult {
                                    mode: entry.mode,
                                    popup: entry.popup,
                                    next: entry.next,
                                    provider_label: "Google",
                                    session: None,
                                    ticket: None,
                                    identity: Some(display),
                                    error: Some(msg),
                                };
                            }
                            if !settings.is_google_allowed(&profile.sub, &profile.email, profile.email_verified) {
                                let msg = format!("Google 账号「{display}」未在允许登录白名单或已绑定列表中");
                                logging::log("[Security]", &format!("❌ Google 登录被拒（{ip}）：{msg}"));
                                notifications::notify_login_failure("Google", &display, ip, &msg);
                                set_outcome(OauthFlowOutcome::Error(msg.clone()));
                                return CallbackResult {
                                    mode: entry.mode,
                                    popup: entry.popup,
                                    next: entry.next,
                                    provider_label: "Google",
                                    session: None,
                                    ticket: None,
                                    identity: Some(display),
                                    error: Some(msg),
                                };
                            }
                            let session = IssuedSession::new_session();
                            let ticket = issue_ticket(session.clone());
                            logging::log("[Security]", &format!("✅ Google 面板登录成功（{display} / {ip}）"));
                            notifications::notify_login_success("Google", &display, ip);
                            set_outcome(OauthFlowOutcome::Done {
                                ticket: Some(ticket.clone()),
                                identity: display.clone(),
                                provider: "Google",
                            });
                            CallbackResult {
                                mode: entry.mode,
                                popup: entry.popup,
                                next: entry.next,
                                provider_label: "Google",
                                session: Some(session),
                                ticket: Some(ticket),
                                identity: Some(display),
                                error: None,
                            }
                        }
                    }
                }
                Err(err) => {
                    if entry.mode == OauthMode::Login {
                        notifications::notify_login_failure("Google", "未知", ip, &err);
                    }
                    set_outcome(OauthFlowOutcome::Error(err.clone()));
                    CallbackResult {
                        mode: entry.mode,
                        popup: entry.popup,
                        next: entry.next,
                        provider_label: "Google",
                        session: None,
                        ticket: None,
                        identity: None,
                        error: Some(err),
                    }
                }
            }
        }
        OauthProvider::Github => {
            match exchange_github(&settings, code, &entry.redirect_uri, &entry.code_verifier).await {
                Ok(profile) => {
                    let display = if !profile.login.is_empty() {
                        profile.login.clone()
                    } else if !profile.email.is_empty() {
                        profile.email.clone()
                    } else {
                        profile.id.clone()
                    };
                    match entry.mode {
                        OauthMode::Bind => match record_bound_github(&profile.id, &profile.login, &profile.email, &profile.name) {
                            Ok(_) => {
                                logging::log("[Security]", &format!("✅ 已绑定 GitHub 登录账号：{display}"));
                                set_outcome(OauthFlowOutcome::Done {
                                    ticket: None,
                                    identity: display.clone(),
                                    provider: "GitHub",
                                });
                                CallbackResult {
                                    mode: entry.mode,
                                    popup: entry.popup,
                                    next: entry.next,
                                    provider_label: "GitHub",
                                    session: None,
                                    ticket: None,
                                    identity: Some(display),
                                    error: None,
                                }
                            }
                            Err(err) => {
                                set_outcome(OauthFlowOutcome::Error(err.clone()));
                                CallbackResult {
                                    mode: entry.mode,
                                    popup: entry.popup,
                                    next: entry.next,
                                    provider_label: "GitHub",
                                    session: None,
                                    ticket: None,
                                    identity: None,
                                    error: Some(err),
                                }
                            }
                        },
                        OauthMode::Login => {
                            if !access::admin_registered() {
                                let msg = "尚未注册管理员账号，请先完成首次注册".to_string();
                                set_outcome(OauthFlowOutcome::Error(msg.clone()));
                                return CallbackResult {
                                    mode: entry.mode,
                                    popup: entry.popup,
                                    next: entry.next,
                                    provider_label: "GitHub",
                                    session: None,
                                    ticket: None,
                                    identity: Some(display),
                                    error: Some(msg),
                                };
                            }
                            if !settings.is_github_allowed(&profile.id, &profile.login) {
                                let msg = format!("GitHub 账号「{display}」未在允许登录白名单或已绑定列表中");
                                logging::log("[Security]", &format!("❌ GitHub 登录被拒（{ip}）：{msg}"));
                                notifications::notify_login_failure("GitHub", &display, ip, &msg);
                                set_outcome(OauthFlowOutcome::Error(msg.clone()));
                                return CallbackResult {
                                    mode: entry.mode,
                                    popup: entry.popup,
                                    next: entry.next,
                                    provider_label: "GitHub",
                                    session: None,
                                    ticket: None,
                                    identity: Some(display),
                                    error: Some(msg),
                                };
                            }
                            let session = IssuedSession::new_session();
                            let ticket = issue_ticket(session.clone());
                            logging::log("[Security]", &format!("✅ GitHub 面板登录成功（{display} / {ip}）"));
                            notifications::notify_login_success("GitHub", &display, ip);
                            set_outcome(OauthFlowOutcome::Done {
                                ticket: Some(ticket.clone()),
                                identity: display.clone(),
                                provider: "GitHub",
                            });
                            CallbackResult {
                                mode: entry.mode,
                                popup: entry.popup,
                                next: entry.next,
                                provider_label: "GitHub",
                                session: Some(session),
                                ticket: Some(ticket),
                                identity: Some(display),
                                error: None,
                            }
                        }
                    }
                }
                Err(err) => {
                    if entry.mode == OauthMode::Login {
                        notifications::notify_login_failure("GitHub", "未知", ip, &err);
                    }
                    set_outcome(OauthFlowOutcome::Error(err.clone()));
                    CallbackResult {
                        mode: entry.mode,
                        popup: entry.popup,
                        next: entry.next,
                        provider_label: "GitHub",
                        session: None,
                        ticket: None,
                        identity: None,
                        error: Some(err),
                    }
                }
            }
        }
    }
}

struct GoogleProfile {
    sub: String,
    email: String,
    email_verified: bool,
    name: String,
}

async fn exchange_google(
    settings: &PanelOauthSettings,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<GoogleProfile, String> {
    let secret = settings.google_secret()?;
    let client = egress::client_for_system_proxy();
    let form = [
        ("client_id", settings.google.client_id.as_str()),
        ("client_secret", secret.as_str()),
        ("code", code),
        ("code_verifier", code_verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    let token_resp = client
        .post("https://oauth2.googleapis.com/token")
        .timeout(HTTP_TIMEOUT)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("连接 Google 令牌服务失败：{}", egress::describe_error_detail(&e)))?;
    let status = token_resp.status();
    let token_json: Value = token_resp
        .json()
        .await
        .map_err(|_| format!("Google 令牌服务返回无效响应（HTTP {status}）"))?;
    let Some(access_token) = token_json.get("access_token").and_then(Value::as_str) else {
        let desc = token_json
            .get("error_description")
            .or_else(|| token_json.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("未能获取 access_token");
        return Err(format!("Google 令牌交换失败：{desc}"));
    };

    let info_resp = client
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .timeout(HTTP_TIMEOUT)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("读取 Google 账号信息失败：{}", egress::describe_error_detail(&e)))?;
    let info_json: Value = info_resp
        .json()
        .await
        .map_err(|_| "解析 Google 账号信息失败".to_string())?;
    let sub = info_json
        .get("sub")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let email = info_json
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let email_verified = info_json
        .get("email_verified")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = info_json
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if sub.is_empty() && email.is_empty() {
        return Err("Google 未返回有效的用户标识或邮箱".to_string());
    }
    Ok(GoogleProfile {
        sub,
        email,
        email_verified,
        name,
    })
}

struct GithubProfile {
    id: String,
    login: String,
    email: String,
    name: String,
}

async fn exchange_github(
    settings: &PanelOauthSettings,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<GithubProfile, String> {
    let secret = settings.github_secret()?;
    let client = egress::client_for_system_proxy();
    let token_resp = client
        .post("https://github.com/login/oauth/access_token")
        .timeout(HTTP_TIMEOUT)
        .header("Accept", "application/json")
        .json(&json!({
            "client_id": settings.github.client_id,
            "client_secret": secret.as_str(),
            "code": code,
            "redirect_uri": redirect_uri,
            "code_verifier": code_verifier,
        }))
        .send()
        .await
        .map_err(|e| format!("连接 GitHub 令牌服务失败：{}", egress::describe_error_detail(&e)))?;
    let status = token_resp.status();
    let token_json: Value = token_resp
        .json()
        .await
        .map_err(|_| format!("GitHub 令牌服务返回无效响应（HTTP {status}）"))?;
    let Some(access_token) = token_json.get("access_token").and_then(Value::as_str) else {
        let desc = token_json
            .get("error_description")
            .or_else(|| token_json.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("未能获取 access_token");
        return Err(format!("GitHub 令牌交换失败：{desc}"));
    };

    let user_resp = client
        .get("https://api.github.com/user")
        .timeout(HTTP_TIMEOUT)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Agent2API")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("读取 GitHub 账号信息失败：{}", egress::describe_error_detail(&e)))?;
    let user_json: Value = user_resp
        .json()
        .await
        .map_err(|_| "解析 GitHub 账号信息失败".to_string())?;
    let id = match user_json.get("id") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    };
    let login = user_json
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let name = user_json
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let email = user_json
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() && login.is_empty() {
        let msg = user_json
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("GitHub 未返回有效的用户信息");
        return Err(format!("读取 GitHub 用户失败：{msg}"));
    }
    Ok(GithubProfile {
        id,
        login,
        email,
        name,
    })
}
