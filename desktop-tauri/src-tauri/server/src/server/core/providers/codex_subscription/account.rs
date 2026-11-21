//! Codex auth.json 的主动导入与标准账号投影。
//! 依据 Magpie account.go/codexAuth、signin.go/codexLogin；不发现或读取本机登录文件。

use agent2api_provider_protocol::{NormalizeInput, NormalizeSource, NormalizedAccount};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::{resolve_account_proxy, ResolvedProxy};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::ID;

const MAX_CREDENTIAL_BYTES: usize = 256 * 1024;
const CLAIMS: &str = "https://api.openai.com/auth";

// 不派生 Debug，防止凭证随日志输出。
#[derive(Clone)]
pub(super) struct Tokens {
    pub access: String,
    pub refresh: String,
    pub id_token: String,
    pub account_id: String,
    pub user_id: String,
    pub email: String,
    pub plan: String,
    pub expires_at: Option<i64>,
}

pub(super) struct Snapshot {
    pub id: String,
    pub secret: Value,
    pub tokens: Tokens,
    pub proxy: Option<ResolvedProxy>,
}

pub(super) fn text<'a>(value: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|key| {
            value
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
        .unwrap_or("")
}

fn credential(value: &Value, keys: &[&str]) -> String {
    text(value, keys).to_string()
}

pub(super) fn jwt_claims(token: &str) -> Option<Value> {
    if token.len() > MAX_CREDENTIAL_BYTES {
        return None;
    }
    let encoded = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| URL_SAFE.decode(encoded))
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims.is_object().then_some(claims)
}

fn claim(claims: &Value, key: &str) -> String {
    claims
        .get(CLAIMS)
        .map(|claims| credential(claims, &[key]))
        .unwrap_or_default()
}

fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 128 * 1024
        && token.is_ascii()
        && !token.chars().any(char::is_whitespace)
        && !token.chars().any(char::is_control)
}

fn timestamp(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(text) = value.as_str() {
        if let Ok(date) = chrono::DateTime::parse_from_rfc3339(text) {
            return Some(date.timestamp_millis());
        }
    }
    let raw = value.as_i64().or_else(|| value.as_str()?.parse().ok())?;
    if raw <= 0 {
        return None;
    }
    let ms = if raw < 100_000_000_000 {
        raw.checked_mul(1000)?
    } else {
        raw
    };
    (ms > 0).then_some(ms)
}

impl Tokens {
    pub(super) fn read(value: &Value) -> Result<Self, GatewayError> {
        let value = value
            .get("secret")
            .or_else(|| value.get("pluginSecret"))
            .unwrap_or(value);
        if !value.is_object() {
            return Err(GatewayError::bad_request("Codex 凭据必须是 JSON 对象"));
        }
        let mode = text(value, &["auth_mode", "authMode"]);
        if !mode.is_empty() && mode != "chatgpt" {
            return Err(GatewayError::bad_request(
                "这里只接收 ChatGPT 订阅的 Codex 凭据，不接收 API Key 或其他登录方式",
            ));
        }
        let tokens = value
            .get("tokens")
            .filter(|item| item.is_object())
            .or_else(|| value.get("auth").filter(|item| item.is_object()))
            .unwrap_or(value);
        let access = credential(tokens, &["accessToken", "access_token"]);
        let refresh = credential(tokens, &["refreshToken", "refresh_token"]);
        let id_token = credential(tokens, &["idToken", "id_token"]);
        if !valid_token(&access) || access.starts_with("sk-") {
            return Err(GatewayError::bad_request("请导入 Codex 的 ChatGPT 登录凭据，其中需要有效的 access_token；API Key 不能用于订阅通道"));
        }
        if !refresh.is_empty() && !valid_token(&refresh)
            || !id_token.is_empty() && !valid_token(&id_token)
        {
            return Err(GatewayError::bad_request(
                "Codex 凭据格式无效，请重新导入完整文件",
            ));
        }
        // JWT 仅用于提取显示信息及到期时间；授权有效性仍由上游验证。
        let id_claims = jwt_claims(&id_token).unwrap_or(Value::Null);
        let access_claims = jwt_claims(&access).unwrap_or(Value::Null);
        let mut account_id = credential(tokens, &["accountId", "account_id", "chatgptAccountId"]);
        if account_id.is_empty() {
            account_id = credential(value, &["accountId", "account_id"]);
        }
        let id_account = claim(&id_claims, "chatgpt_account_id");
        let access_account = claim(&access_claims, "chatgpt_account_id");
        if !id_account.is_empty() && !access_account.is_empty() && id_account != access_account {
            return Err(GatewayError::bad_request(
                "Codex 的身份令牌和访问令牌不属于同一账号，请重新导入完整凭据",
            ));
        }
        let claimed_account = if id_account.is_empty() {
            access_account
        } else {
            id_account
        };
        if !account_id.is_empty() && !claimed_account.is_empty() && account_id != claimed_account {
            return Err(GatewayError::bad_request(
                "Codex 凭据里的账号标识与登录令牌不一致，请重新导入同一份完整凭据",
            ));
        }
        if account_id.is_empty() {
            account_id = claimed_account;
        }
        if account_id.is_empty()
            || account_id.len() > 512
            || !account_id
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
        {
            return Err(GatewayError::bad_request(
                "Codex 凭据缺少有效的 account_id，请导入完整 auth.json 或重新授权",
            ));
        }
        let user_id = [
            claim(&id_claims, "chatgpt_user_id"),
            claim(&access_claims, "chatgpt_user_id"),
            claim(&id_claims, "user_id"),
            claim(&access_claims, "user_id"),
            credential(&id_claims, &["sub"]),
            credential(&access_claims, &["sub"]),
            credential(value, &["userId"]),
        ]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_default();
        let email = [
            credential(&id_claims, &["email"]),
            access_claims
                .get("https://api.openai.com/profile")
                .map(|item| credential(item, &["email"]))
                .unwrap_or_default(),
            credential(value, &["email"]),
        ]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_default();
        let plan = [
            claim(&id_claims, "chatgpt_plan_type"),
            claim(&access_claims, "chatgpt_plan_type"),
            credential(value, &["planType", "plan"]),
        ]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_default();
        let expires_at = timestamp(access_claims.get("exp"))
            .or_else(|| timestamp(tokens.get("expiresAt")))
            .or_else(|| timestamp(value.get("expiresAt")));
        Ok(Self {
            access,
            refresh,
            id_token,
            account_id,
            user_id,
            email,
            plan,
            expires_at,
        })
    }

    pub(super) fn identity(&self) -> String {
        let person = if self.user_id.is_empty() {
            self.email.to_ascii_lowercase()
        } else {
            self.user_id.clone()
        };
        let digest = Sha256::digest(format!("{}\0{person}", self.account_id).as_bytes());
        format!("codex-{:x}", digest)
    }

    pub(super) fn expiring_within(&self, min_lead_ms: i64) -> bool {
        let margin = 300_000_i64.max(min_lead_ms.max(0));
        self.expires_at
            .is_some_and(|expires| expires <= logging::now_ms().saturating_add(margin))
    }

    pub(super) fn expiring(&self) -> bool {
        self.expiring_within(0)
    }

    pub(super) fn public(&self, name: Option<&str>, needs_relogin: bool) -> Value {
        let fallback = if self.email.is_empty() {
            "Codex 订阅"
        } else {
            &self.email
        };
        let name = name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(fallback);
        let mut public = json!({
            "uid": self.identity(), "accountId": self.account_id, "email": self.email,
            "name": name, "loginName": self.email, "planName": self.plan,
            "authMode": "chatgpt", "hasAccessToken": !self.access.is_empty(),
            "hasRefreshToken": !self.refresh.is_empty(), "canRefresh": !self.refresh.is_empty(),
            "needsRelogin": needs_relogin,
            "needsReloginReason": "",
            "tokenTail": if self.access.len() > 6 {
                self.access.chars().rev().take(6).collect::<String>().chars().rev().collect::<String>()
            } else { String::new() },
        });
        if let Some(expires) = self.expires_at {
            public["expiresAt"] = json!(expires);
            public["accessTokenExpiresAt"] = json!(expires);
        }
        if needs_relogin {
            public["needsReloginReason"] = json!("Codex 登录授权已失效，请重新授权或导入新的凭据");
        }
        if self.refresh.is_empty() {
            public["credentialWarning"] = json!("这份凭据没有刷新令牌，到期后需要重新授权或导入");
        }
        public
    }

    pub(super) fn secret(&self) -> Value {
        json!({ "authMode": "chatgpt", "accessToken": self.access, "refreshToken": self.refresh,
            "idToken": self.id_token, "accountId": self.account_id, "userId": self.user_id,
            "uid": self.identity(), "email": self.email, "planType": self.plan, "expiresAt": self.expires_at })
    }
}

fn parse_json(content: &str) -> Result<Value, GatewayError> {
    if content.len() > MAX_CREDENTIAL_BYTES {
        return Err(GatewayError::bad_request("Codex 凭据文件不能超过 256 KiB"));
    }
    serde_json::from_str(content.trim().trim_start_matches('\u{feff}'))
        .map_err(|_| GatewayError::bad_request("Codex 凭据不是有效 JSON，请导入完整 auth.json"))
}

pub(super) fn normalize(input: NormalizeInput) -> Result<NormalizedAccount, GatewayError> {
    if input.provider != ID {
        return Err(GatewayError::bad_request("Codex 凭据的提供商不匹配"));
    }
    let value = match input.source {
        NormalizeSource::FileText { content, .. } => parse_json(&content)?,
        NormalizeSource::FileBase64 { content, .. } => {
            if content.len() > MAX_CREDENTIAL_BYTES * 2 {
                return Err(GatewayError::bad_request("Codex 凭据文件过大"));
            }
            let bytes = STANDARD
                .decode(content.trim())
                .map_err(|_| GatewayError::bad_request("Codex 凭据文件编码无效"))?;
            let content = String::from_utf8(bytes)
                .map_err(|_| GatewayError::bad_request("Codex 凭据文件必须是 UTF-8 JSON"))?;
            parse_json(&content)?
        }
        NormalizeSource::Manual { payload } => {
            if let Some(method) = payload.get("methodIndex") {
                if method.as_u64() != Some(1) {
                    return Err(GatewayError::bad_request(
                        "请选择导入 Codex 凭据，设备授权请从登录入口开始",
                    ));
                }
            }
            let fields = payload.get("inputs").unwrap_or(&payload);
            if let Some(content) = fields
                .get("credentials")
                .and_then(Value::as_str)
                .or_else(|| fields.get("authJson").and_then(Value::as_str))
                .or_else(|| fields.get("authFileContent").and_then(Value::as_str))
            {
                parse_json(content)?
            } else {
                fields.clone()
            }
        }
        NormalizeSource::Desktop => {
            return Err(GatewayError::bad_request(
                "不会自动读取本机 Codex 登录文件，请主动选择文件导入或使用设备授权",
            ))
        }
    };
    let tokens = Tokens::read(&value)?;
    if !tokens.refresh.is_empty() && tokens.user_id.is_empty() && tokens.email.is_empty() {
        return Err(GatewayError::bad_request(
            "可续期的 Codex 凭据缺少稳定的用户身份，请导入含 id_token 的完整 auth.json",
        ));
    }
    if tokens.refresh.is_empty() && tokens.expires_at.is_some_and(|at| at <= logging::now_ms()) {
        return Err(GatewayError::bad_request(
            "这份 Codex 凭据已过期且没有刷新令牌，请重新授权",
        ));
    }
    let mut secret = tokens.secret();
    secret["lastRefreshAt"] = json!(
        timestamp(value.get("last_refresh")).or_else(|| timestamp(value.get("lastRefreshAt")))
    );
    Ok(NormalizedAccount {
        secret,
        public: tokens.public(input.name.as_deref(), false),
        identity: tokens.identity(),
        desktop: false,
    })
}

pub(super) fn snapshot(store: &AccountStore, account_id: &str) -> Result<Snapshot, GatewayError> {
    let id = if account_id.is_empty() {
        store
            .accounts_for_provider(ID)
            .iter()
            .find_map(|account| {
                account
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .ok_or_else(|| GatewayError::with_status(401, "请先添加并启用 Codex 订阅账号"))?
    } else {
        account_id.to_string()
    };
    let account = store
        .plugin_account(&id)
        .ok_or_else(|| GatewayError::with_status(404, "Codex 账号不存在"))?;
    if account.get("provider").and_then(Value::as_str) != Some(ID) {
        return Err(GatewayError::bad_request("指定账号不属于 Codex 订阅"));
    }
    let secret = account
        .get("secret")
        .cloned()
        .ok_or_else(|| GatewayError::with_status(401, "Codex 账号没有凭据"))?;
    let tokens = Tokens::read(&secret)
        .map_err(|_| GatewayError::with_status(401, "Codex 登录凭据不完整，请重新授权或导入"))?;
    let proxy = account_proxy(account.get("proxy").unwrap_or(&Value::Null))?;
    Ok(Snapshot {
        id,
        secret,
        tokens,
        proxy,
    })
}

fn account_proxy(value: &Value) -> Result<Option<ResolvedProxy>, GatewayError> {
    let failed = || {
        GatewayError::with_status(502, "Codex 账号代理不可用，未回退直连")
            .with_code("account_proxy_unavailable")
    };
    match value.get("state").and_then(Value::as_str) {
        Some("failed") => Err(failed()),
        Some("unset") => Ok(None),
        Some("resolved") => ResolvedProxy::from_json(value.get("proxy").unwrap_or(&Value::Null))
            .map_err(|_| failed())?
            .map(Some)
            .ok_or_else(failed),
        Some("static") => {
            let config = value.get("config").ok_or_else(failed)?;
            if config.get("source").and_then(Value::as_str) == Some("direct") {
                return Ok(None);
            }
            let resolved = resolve_account_proxy(Some(config)).ok_or_else(failed)?;
            if resolved.error().is_some() {
                return Err(failed());
            }
            ResolvedProxy::from_json(&resolved.to_json()).map_err(|_| failed())
        }
        _ if value.is_null() => Ok(None),
        _ => ResolvedProxy::from_json(value).map_err(|_| failed()),
    }
}
