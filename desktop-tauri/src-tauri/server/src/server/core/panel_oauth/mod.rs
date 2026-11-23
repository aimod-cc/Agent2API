//! 管理面板 OAuth 第三方登录配置（Google 与 GitHub）。
//!
//! 支持两种授权绑定方式：
//! 1. 直接在「设置 → 账户」填写允许登录的 Google 邮箱 / GitHub 用户名白名单；
//! 2. 在已登录管理员状态下点击「跳转绑定当前账号」，走 OAuth 回调自动记录身份。
//!
//! Client Secret 使用 `SecretScope::PanelCaptcha` 加密存储，不随云同步外传，也不回传明文。

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

use crate::server::config;
use crate::server::core::crypto::{self, SecretScope};

pub mod flow;

pub const KEY_PANEL_OAUTH: &str = "panelOauth";
const SCOPE: SecretScope = SecretScope::PanelCaptcha;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundOauthAccount {
    pub id: String,
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bound_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct GoogleOauthConfig {
    pub enabled: bool,
    pub client_id: String,
    pub(crate) client_secret_enc: String,
    pub allowed_emails: Vec<String>,
    pub bound_accounts: Vec<BoundOauthAccount>,
}

#[derive(Clone, Debug, Default)]
pub struct GithubOauthConfig {
    pub enabled: bool,
    pub client_id: String,
    pub(crate) client_secret_enc: String,
    pub allowed_users: Vec<String>,
    pub bound_accounts: Vec<BoundOauthAccount>,
}

#[derive(Clone, Debug, Default)]
pub struct PanelOauthSettings {
    pub public_base_url: String,
    pub google: GoogleOauthConfig,
    pub github: GithubOauthConfig,
}

impl PanelOauthSettings {
    pub fn current() -> Self {
        config::with_raw(Self::from_raw).unwrap_or_default()
    }

    pub fn from_raw(raw: &Map<String, Value>) -> Self {
        let Some(root) = raw.get(KEY_PANEL_OAUTH).and_then(Value::as_object) else {
            return Self::default();
        };
        let public_base_url = root
            .get("publicBaseUrl")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .trim_end_matches('/')
            .to_string();

        let g_obj = root.get("google").and_then(Value::as_object);
        let google = GoogleOauthConfig {
            enabled: g_obj
                .and_then(|m| m.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            client_id: g_obj
                .and_then(|m| m.get("clientId"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            client_secret_enc: g_obj
                .and_then(|m| m.get("clientSecretEnc"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            allowed_emails: g_obj
                .and_then(|m| m.get("allowedEmails"))
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            bound_accounts: g_obj
                .and_then(|m| m.get("boundAccounts"))
                .and_then(|v| serde_json::from_value::<Vec<BoundOauthAccount>>(v.clone()).ok())
                .unwrap_or_default(),
        };

        let gh_obj = root.get("github").and_then(Value::as_object);
        let github = GithubOauthConfig {
            enabled: gh_obj
                .and_then(|m| m.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            client_id: gh_obj
                .and_then(|m| m.get("clientId"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            client_secret_enc: gh_obj
                .and_then(|m| m.get("clientSecretEnc"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            allowed_users: gh_obj
                .and_then(|m| m.get("allowedUsers"))
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            bound_accounts: gh_obj
                .and_then(|m| m.get("boundAccounts"))
                .and_then(|v| serde_json::from_value::<Vec<BoundOauthAccount>>(v.clone()).ok())
                .unwrap_or_default(),
        };

        Self {
            public_base_url,
            google,
            github,
        }
    }

    pub fn to_stored_value(&self) -> Value {
        json!({
            "publicBaseUrl": self.public_base_url,
            "google": {
                "enabled": self.google.enabled,
                "clientId": self.google.client_id,
                "clientSecretEnc": self.google.client_secret_enc,
                "allowedEmails": self.google.allowed_emails,
                "boundAccounts": self.google.bound_accounts,
            },
            "github": {
                "enabled": self.github.enabled,
                "clientId": self.github.client_id,
                "clientSecretEnc": self.github.client_secret_enc,
                "allowedUsers": self.github.allowed_users,
                "boundAccounts": self.github.bound_accounts,
            }
        })
    }

    pub fn public_status_json(&self) -> Value {
        let google_ready = self.google.enabled
            && !self.google.client_id.is_empty()
            && !self.google.client_secret_enc.is_empty();
        let github_ready = self.github.enabled
            && !self.github.client_id.is_empty()
            && !self.github.client_secret_enc.is_empty();
        json!({
            "google": google_ready,
            "github": github_ready,
        })
    }

    pub fn management_json(&self) -> Value {
        json!({
            "publicBaseUrl": self.public_base_url,
            "callbackPath": "/api/panel/oauth/callback",
            "google": {
                "enabled": self.google.enabled,
                "clientId": self.google.client_id,
                "clientSecretSet": !self.google.client_secret_enc.is_empty(),
                "allowedEmails": self.google.allowed_emails,
                "boundAccounts": self.google.bound_accounts,
            },
            "github": {
                "enabled": self.github.enabled,
                "clientId": self.github.client_id,
                "clientSecretSet": !self.github.client_secret_enc.is_empty(),
                "allowedUsers": self.github.allowed_users,
                "boundAccounts": self.github.bound_accounts,
            }
        })
    }

    pub fn google_secret(&self) -> Result<Zeroizing<String>, String> {
        decrypt_secret(&self.google.client_secret_enc, "Google Client Secret")
    }

    pub fn github_secret(&self) -> Result<Zeroizing<String>, String> {
        decrypt_secret(&self.github.client_secret_enc, "GitHub Client Secret")
    }

    pub fn is_google_allowed(&self, sub: &str, email: &str, email_verified: bool) -> bool {
        if !sub.is_empty()
            && self
                .google
                .bound_accounts
                .iter()
                .any(|acc| acc.id == sub)
        {
            return true;
        }
        if email_verified && !email.is_empty() {
            let lower = email.trim().to_ascii_lowercase();
            if self.google.allowed_emails.iter().any(|e| e == &lower) {
                return true;
            }
            if self
                .google
                .bound_accounts
                .iter()
                .any(|acc| acc.email.eq_ignore_ascii_case(&lower))
            {
                return true;
            }
        }
        false
    }

    pub fn is_github_allowed(&self, id: &str, login: &str) -> bool {
        if !id.is_empty()
            && (self.github.bound_accounts.iter().any(|acc| acc.id == id)
                || self.github.allowed_users.iter().any(|u| u == id))
        {
            return true;
        }
        if !login.is_empty() {
            let lower = login.trim().to_ascii_lowercase();
            if self.github.allowed_users.iter().any(|u| u == &lower) {
                return true;
            }
            if self
                .github
                .bound_accounts
                .iter()
                .any(|acc| acc.login.eq_ignore_ascii_case(&lower))
            {
                return true;
            }
        }
        false
    }
}

fn decrypt_secret(enc: &str, label: &str) -> Result<Zeroizing<String>, String> {
    if enc.trim().is_empty() {
        return Err(format!("请先配置 {label}"));
    }
    if !crypto::is_envelope(enc) {
        return Ok(Zeroizing::new(enc.to_string()));
    }
    let bytes = Zeroizing::new(
        crypto::open(enc, SCOPE)
            .map_err(|_| format!("{label} 无法解密，请在账户设置中重新保存"))?,
    );
    String::from_utf8(bytes.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| format!("{label} 格式无效"))
}

fn parse_string_list(value: &Value, label: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    match value {
        Value::Array(items) => {
            for item in items {
                let s = item
                    .as_str()
                    .ok_or_else(|| format!("{label}列表项必须是字符串"))?
                    .trim()
                    .to_ascii_lowercase();
                if !s.is_empty() && !out.contains(&s) {
                    out.push(s);
                }
            }
        }
        Value::String(text) => {
            for part in text.split(['\n', ',', '，', ';', '；']) {
                let s = part.trim().to_ascii_lowercase();
                if !s.is_empty() && !out.contains(&s) {
                    out.push(s);
                }
            }
        }
        _ => return Err(format!("{label}必须是字符串数组或换行分隔文本")),
    }
    if out.len() > 64 {
        return Err(format!("{label}最多支持 64 项"));
    }
    Ok(out)
}

pub fn apply_patch(
    settings: &mut PanelOauthSettings,
    object: &Map<String, Value>,
) -> Result<(), String> {
    if let Some(base) = object.get("publicBaseUrl").and_then(Value::as_str) {
        let trimmed = base.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            settings.public_base_url.clear();
        } else {
            let parsed = url::Url::parse(trimmed)
                .map_err(|_| "外部访问地址格式无效，需包含 http:// 或 https://".to_string())?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err("外部访问地址必须以 http:// 或 https:// 开头".to_string());
            }
            settings.public_base_url = trimmed.to_string();
        }
    }

    if let Some(g) = object.get("google").and_then(Value::as_object) {
        if let Some(enabled) = g.get("enabled") {
            settings.google.enabled = enabled
                .as_bool()
                .ok_or_else(|| "google.enabled 必须是布尔值".to_string())?;
        }
        if let Some(client_id) = g.get("clientId").and_then(Value::as_str) {
            settings.google.client_id = client_id.trim().to_string();
        }
        if g.get("clearClientSecret").and_then(Value::as_bool) == Some(true)
            || g.get("clientSecret").is_some_and(Value::is_null)
        {
            settings.google.client_secret_enc.clear();
        } else if let Some(secret) = g.get("clientSecret").and_then(Value::as_str) {
            let trimmed = secret.trim();
            if !trimmed.is_empty() {
                settings.google.client_secret_enc = crypto::seal(trimmed.as_bytes(), SCOPE)?;
            }
        }
        if let Some(emails) = g.get("allowedEmails") {
            settings.google.allowed_emails = parse_string_list(emails, "Google 允许邮箱")?;
        }
        if let Some(unbind_id) = g.get("unbindId").and_then(Value::as_str) {
            let target = unbind_id.trim();
            settings.google.bound_accounts.retain(|acc| acc.id != target && !acc.email.eq_ignore_ascii_case(target));
        }
        if g.get("clearBoundAccounts").and_then(Value::as_bool) == Some(true) {
            settings.google.bound_accounts.clear();
        }
    }

    if let Some(gh) = object.get("github").and_then(Value::as_object) {
        if let Some(enabled) = gh.get("enabled") {
            settings.github.enabled = enabled
                .as_bool()
                .ok_or_else(|| "github.enabled 必须是布尔值".to_string())?;
        }
        if let Some(client_id) = gh.get("clientId").and_then(Value::as_str) {
            settings.github.client_id = client_id.trim().to_string();
        }
        if gh.get("clearClientSecret").and_then(Value::as_bool) == Some(true)
            || gh.get("clientSecret").is_some_and(Value::is_null)
        {
            settings.github.client_secret_enc.clear();
        } else if let Some(secret) = gh.get("clientSecret").and_then(Value::as_str) {
            let trimmed = secret.trim();
            if !trimmed.is_empty() {
                settings.github.client_secret_enc = crypto::seal(trimmed.as_bytes(), SCOPE)?;
            }
        }
        if let Some(users) = gh.get("allowedUsers") {
            settings.github.allowed_users = parse_string_list(users, "GitHub 允许用户名")?;
        }
        if let Some(unbind_id) = gh.get("unbindId").and_then(Value::as_str) {
            let target = unbind_id.trim();
            settings.github.bound_accounts.retain(|acc| acc.id != target && !acc.login.eq_ignore_ascii_case(target));
        }
        if gh.get("clearBoundAccounts").and_then(Value::as_bool) == Some(true) {
            settings.github.bound_accounts.clear();
        }
    }

    if settings.google.enabled {
        if settings.google.client_id.is_empty() {
            return Err("启用 Google 登录前请填写 Client ID".to_string());
        }
        if settings.google.client_secret_enc.is_empty() {
            return Err("启用 Google 登录前请填写 Client Secret".to_string());
        }
    }
    if settings.github.enabled {
        if settings.github.client_id.is_empty() {
            return Err("启用 GitHub 登录前请填写 Client ID".to_string());
        }
        if settings.github.client_secret_enc.is_empty() {
            return Err("启用 GitHub 登录前请填写 Client Secret".to_string());
        }
    }
    Ok(())
}

pub fn save(object: &Map<String, Value>) -> Result<PanelOauthSettings, (i32, String)> {
    config::try_update_raw_field(
        KEY_PANEL_OAUTH,
        |slot| {
            let mut raw_map = Map::new();
            if !slot.is_null() {
                raw_map.insert(KEY_PANEL_OAUTH.to_string(), slot.clone());
            }
            let mut settings = PanelOauthSettings::from_raw(&raw_map);
            apply_patch(&mut settings, object).map_err(|msg| (400, msg))?;
            *slot = settings.to_stored_value();
            Ok(settings)
        },
        || (500, "写入 OAuth 登录设置失败，请重试".to_string()),
    )
}

pub fn record_bound_google(sub: &str, email: &str, name: &str) -> Result<PanelOauthSettings, String> {
    let lower_email = email.trim().to_ascii_lowercase();
    let sub = sub.trim().to_string();
    let name = name.trim().to_string();
    let now = crate::server::logging::now_ms();
    config::try_update_raw_field(
        KEY_PANEL_OAUTH,
        |slot| {
            let mut raw_map = Map::new();
            if !slot.is_null() {
                raw_map.insert(KEY_PANEL_OAUTH.to_string(), slot.clone());
            }
            let mut settings = PanelOauthSettings::from_raw(&raw_map);
            if !lower_email.is_empty() && !settings.google.allowed_emails.contains(&lower_email) {
                settings.google.allowed_emails.push(lower_email.clone());
            }
            settings
                .google
                .bound_accounts
                .retain(|acc| acc.id != sub && (lower_email.is_empty() || !acc.email.eq_ignore_ascii_case(&lower_email)));
            settings.google.bound_accounts.push(BoundOauthAccount {
                id: sub,
                login: String::new(),
                email: lower_email,
                name,
                bound_at: now,
            });
            *slot = settings.to_stored_value();
            Ok(settings)
        },
        || "保存绑定的 Google 账号失败".to_string(),
    )
}

pub fn record_bound_github(
    id: &str,
    login: &str,
    email: &str,
    name: &str,
) -> Result<PanelOauthSettings, String> {
    let lower_login = login.trim().to_ascii_lowercase();
    let id = id.trim().to_string();
    let email = email.trim().to_string();
    let name = name.trim().to_string();
    let now = crate::server::logging::now_ms();
    config::try_update_raw_field(
        KEY_PANEL_OAUTH,
        |slot| {
            let mut raw_map = Map::new();
            if !slot.is_null() {
                raw_map.insert(KEY_PANEL_OAUTH.to_string(), slot.clone());
            }
            let mut settings = PanelOauthSettings::from_raw(&raw_map);
            if !lower_login.is_empty() && !settings.github.allowed_users.contains(&lower_login) {
                settings.github.allowed_users.push(lower_login.clone());
            }
            settings
                .github
                .bound_accounts
                .retain(|acc| acc.id != id && (lower_login.is_empty() || !acc.login.eq_ignore_ascii_case(&lower_login)));
            settings.github.bound_accounts.push(BoundOauthAccount {
                id,
                login: login.trim().to_string(),
                email,
                name,
                bound_at: now,
            });
            *slot = settings.to_stored_value();
            Ok(settings)
        },
        || "保存绑定的 GitHub 账号失败".to_string(),
    )
}
