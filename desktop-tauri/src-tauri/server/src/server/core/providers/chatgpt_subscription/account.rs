//! SIWC 凭证与账号公开投影；账号身份为 issuer + 已签发 client_id + sub，绝不按邮箱合并。

use agent2api_provider_protocol::NormalizedAccount;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::core::account_store::{plugin_accounts, AccountStore};
use crate::server::core::proxies::{resolve_account_proxy, ProxyResolution, ResolvedProxy};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{host, http, DIRECT_SCOPE, DYNAMIC_CLIENT, ID, ISSUER, USAGE_URL};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Credentials {
    pub issuer: String,
    pub client_id: String,
    pub host_id: String,
    #[serde(rename = "sub")]
    pub subject: String,
    #[serde(default)]
    pub email: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: String,
    pub expires_at: i64,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub needs_relogin: bool,
    /// 刷新已轮换令牌但新 ID token 还待验签；保留 replacement，不让它提前用于推理。
    #[serde(default)]
    pub identity_pending: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pending_id_token: String,
}

impl Credentials {
    pub fn empty(client_id: String, host_id: String) -> Self {
        Self {
            issuer: ISSUER.to_string(),
            client_id,
            host_id,
            subject: String::new(),
            email: String::new(),
            access_token: String::new(),
            refresh_token: String::new(),
            id_token: String::new(),
            expires_at: 0,
            scopes: Vec::new(),
            plan: String::new(),
            needs_relogin: false,
            identity_pending: false,
            pending_id_token: String::new(),
        }
    }

    pub fn parse(secret: &Value) -> Result<Self, GatewayError> {
        let value: Self = serde_json::from_value(secret.clone())
            .map_err(|_| GatewayError::bad_request("ChatGPT 凭证不完整，请重新授权登录"))?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), GatewayError> {
        if self.issuer != ISSUER
            || self.client_id.is_empty()
            || self.client_id == DYNAMIC_CLIENT
            || self.client_id.len() > 512
            || self.subject.is_empty()
            || self.subject.len() > 512
            || !host::valid(&self.host_id)
            || !token_valid(&self.access_token)
            || !token_valid(&self.refresh_token)
            || self.expires_at <= 0
        {
            return Err(GatewayError::bad_request(
                "ChatGPT 凭证缺少完整的注册信息或令牌，请重新登录",
            ));
        }
        if !self.scopes.iter().any(|scope| scope == DIRECT_SCOPE) && !self.needs_relogin {
            return Err(GatewayError::bad_request(
                "ChatGPT 尚未授予本应用使用套餐的权限，请重新授权",
            ));
        }
        Ok(())
    }

    pub fn apply_tokens(&mut self, tokens: http::Tokens) -> Result<(), GatewayError> {
        if !token_valid(&tokens.access_token)
            || !token_valid(&tokens.refresh_token)
            || !tokens.expires_in.is_finite()
            || tokens.expires_in <= 0.0
            || tokens.expires_in > 365.0 * 24.0 * 3600.0
        {
            return Err(GatewayError::with_status(
                502,
                "OpenAI 返回的令牌或有效期无效",
            ));
        }
        let scopes: Vec<String> = tokens
            .scope
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let authorized = scopes.iter().any(|scope| scope == DIRECT_SCOPE);
        self.access_token = tokens.access_token;
        self.refresh_token = tokens.refresh_token;
        self.expires_at = logging::now_ms().saturating_add((tokens.expires_in * 1000.0) as i64);
        self.scopes = scopes;
        if !tokens.id_token.is_empty() {
            self.id_token = tokens.id_token;
        }
        self.needs_relogin = !authorized;
        if !authorized {
            return Err(GatewayError::with_status(
                403,
                "OpenAI 未授予 ChatGPT 套餐使用权限，请重新授权",
            )
            .with_code("subscription_sharing_user_not_eligible"));
        }
        Ok(())
    }

    /// claims 必须已由 oidc::verify 验签；不从 access token 解码未验证身份。
    pub fn apply_identity(&mut self, claims: &Value) -> Result<(), GatewayError> {
        self.subject = claims
            .get("sub")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        self.email = claims
            .get("email")
            .and_then(Value::as_str)
            .map(safe_text)
            .unwrap_or_default();
        self.plan = claims
            .get("https://api.openai.com/auth")
            .and_then(|auth| auth.get("chatgpt_plan_type"))
            .and_then(Value::as_str)
            .map(safe_text)
            .unwrap_or_default();
        self.validate()
    }

    pub fn identity(&self) -> String {
        let mut hash = Sha256::new();
        for part in [&self.issuer, &self.client_id, &self.subject] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part.as_bytes());
        }
        format!("siwc-{:x}", hash.finalize())
    }

    pub fn secret(&self, previous: Option<&Value>) -> Result<Value, GatewayError> {
        let mut fields = previous
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let value =
            serde_json::to_value(self).map_err(|_| GatewayError::new("无法保存 ChatGPT 凭证"))?;
        let Some(owned) = value.as_object() else {
            return Err(GatewayError::new("无法保存 ChatGPT 凭证"));
        };
        fields.extend(owned.clone());
        fields.insert("uid".to_string(), Value::String(self.identity()));
        Ok(Value::Object(fields))
    }

    pub fn public(&self) -> Value {
        let name = if self.email.is_empty() {
            "ChatGPT"
        } else {
            &self.email
        };
        let token_tail: String = if self.access_token.chars().count() > 6 {
            self.access_token
                .chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect()
        } else {
            String::new()
        };
        let mut view = json!({
            "name": name, "uid": self.identity(), "email": self.email, "plan": self.plan,
            "tokenTail": token_tail, "expiresAt": self.expires_at,
            "hasAccessToken": !self.access_token.is_empty(),
            "hasRefreshToken": !self.refresh_token.is_empty(), "canRefresh": !self.needs_relogin,
            "needsRelogin": self.needs_relogin,
            "authStatus": if self.needs_relogin { "reauth_required" } else if self.identity_pending { "verification_pending" } else { "ready" },
            "usageUrl": USAGE_URL,
        });
        view["needsReloginReason"] = json!(if self.needs_relogin {
            "ChatGPT 授权已失效，请重新登录"
        } else {
            ""
        });
        view
    }

    pub fn normalized(&self) -> Result<NormalizedAccount, GatewayError> {
        self.validate()?;
        if self.needs_relogin || self.identity_pending {
            return Err(GatewayError::bad_request(
                "ChatGPT 凭证尚未完成验证，请重新登录",
            ));
        }
        Ok(NormalizedAccount {
            identity: self.identity(),
            secret: self.secret(None)?,
            public: self.public(),
            desktop: false,
        })
    }
}

pub(super) fn safe_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(256)
        .collect::<String>()
        .trim()
        .to_string()
}

fn token_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128 * 1024
        && value.bytes().all(|ch| ch > b' ' && ch != 0x7f)
}

pub(super) fn shared() -> Result<AccountStore, GatewayError> {
    plugin_accounts::shared().ok_or_else(|| GatewayError::with_status(503, "账号存储尚未初始化"))
}

pub(super) fn select_id(store: &AccountStore, requested: &str) -> Result<String, GatewayError> {
    if !requested.is_empty() {
        if store.account_provider_of(requested).as_deref() != Some(ID) {
            return Err(GatewayError::with_status(404, "指定的 ChatGPT 账号不存在"));
        }
        return Ok(requested.to_string());
    }
    store
        .accounts_for_provider(ID)
        .iter()
        .find(|account| {
            account.get("authStatus").and_then(Value::as_str) != Some("reauth_required")
        })
        .and_then(|account| account.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| GatewayError::with_status(401, "请先添加可用的 ChatGPT 授权账号"))
}

pub(super) fn load(store: &AccountStore, id: &str) -> Result<(Value, Credentials), GatewayError> {
    if store.account_provider_of(id).as_deref() != Some(ID) {
        return Err(GatewayError::with_status(404, "指定的 ChatGPT 账号不存在"));
    }
    let account = store
        .plugin_account(id)
        .ok_or_else(|| GatewayError::with_status(404, "ChatGPT 账号不存在"))?;
    let secret = account.get("secret").cloned().unwrap_or(Value::Null);
    let credentials = Credentials::parse(&secret)?;
    Ok((secret, credentials))
}

pub(super) fn proxy(store: &AccountStore, id: &str) -> Result<Option<ResolvedProxy>, GatewayError> {
    if store.account_provider_of(id).as_deref() != Some(ID) {
        return Err(GatewayError::with_status(404, "指定的 ChatGPT 账号不存在"));
    }
    let record = store
        .plugin_account(id)
        .ok_or_else(|| GatewayError::with_status(404, "ChatGPT 账号不存在"))?;
    let proxy = record.get("proxy").unwrap_or(&Value::Null);
    match proxy.get("state").and_then(Value::as_str) {
        Some("unset") => Ok(None),
        Some("resolved") => http::proxy_from_options(proxy.get("proxy")),
        Some("static") => match resolve_account_proxy(proxy.get("config")) {
            None => Ok(None),
            Some(ProxyResolution::Resolved(proxy))
                if !proxy.host.is_empty() && proxy.port.is_some() =>
            {
                Ok(Some(proxy))
            }
            Some(_) => Err(GatewayError::bad_request(
                "ChatGPT 账号代理不可用，未回退直连",
            )),
        },
        _ => Err(GatewayError::bad_request(
            "ChatGPT 账号代理不可用，未回退直连",
        )),
    }
}

pub(super) fn persist(
    account: &NormalizedAccount,
    proxy: Option<&Value>,
) -> Result<String, GatewayError> {
    let credentials = Credentials::parse(&account.secret)?;
    if account.identity != credentials.identity()
        || account.desktop
        || credentials.needs_relogin
        || credentials.identity_pending
    {
        return Err(GatewayError::bad_request(
            "ChatGPT 登录结果的账号归属不一致",
        ));
    }
    let public = credentials.public();
    let name = public
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("ChatGPT");
    let store = shared()?;
    let (saved, _) = store
        .plugin_upsert_account_with_source_and_proxy(
            &account.identity,
            ID,
            name,
            &credentials.secret(None)?,
            &public,
            false,
            "builtin",
            proxy,
        )
        .map_err(|_| GatewayError::new("保存 ChatGPT 账号失败，请检查账号库或磁盘空间"))?;
    let id = saved
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| GatewayError::new("保存 ChatGPT 账号后未返回账号标识"))?
        .to_string();
    Ok(id)
}
