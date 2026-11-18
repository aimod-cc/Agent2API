//! Magpie siwcFresh：临期三分钟续期、轮换 refresh token、永久失效要求重新授权。
//! 真实刷新用 refresh_flight 单飞；leader 脱离请求取消，确保轮换结果有机会落库。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::refresh_flight::{self, Join, Table};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{account, error, http, oidc, RESOURCE};
use account::Credentials;

pub(super) const MARGIN_MS: i64 = 3 * 60 * 1000;

fn flights() -> &'static Table<Credentials> {
    static FLIGHTS: OnceLock<Table<Credentials>> = OnceLock::new();
    FLIGHTS.get_or_init(Table::new)
}

pub(super) fn expiring(store: &AccountStore, account_id: &str) -> bool {
    account::load(store, account_id).is_ok_and(|(_, credentials)| {
        !credentials.needs_relogin
            && (credentials.identity_pending
                || credentials.expires_at <= logging::now_ms() + MARGIN_MS)
    })
}

pub(super) async fn fresh(
    store: &AccountStore,
    requested: &str,
    force: bool,
) -> Result<Credentials, GatewayError> {
    let account_id = account::select_id(store, requested)?;
    let (_, current) = account::load(store, &account_id)?;
    if current.needs_relogin {
        return Err(relogin());
    }
    if !force && !current.identity_pending && current.expires_at > logging::now_ms() + MARGIN_MS {
        return Ok(current);
    }
    let key = format!(
        "chatgpt:{}:{}:{}",
        refresh_flight::fingerprint(&store.file_string()),
        account_id,
        refresh_flight::fingerprint(&current.refresh_token)
    );
    let result = match flights().join(&key) {
        Join::Waiter(waiter) => waiter.wait().await,
        Join::Leader(leader) => {
            let store = store.clone();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            // 不继承下游取消：服务器可能已经接受 refresh，丢掉 replacement 会使账号失效。
            crate::spawn_task(async move {
                let result = refresh_serialized(&store, &account_id, current).await;
                leader.finish(result.clone());
                let _ = sender.send(result);
            });
            receiver
                .await
                .map_err(|_| GatewayError::with_status(503, "ChatGPT 凭证刷新任务已结束，请重试"))?
        }
    };
    // 临期的暂时网络失败可继续用尚未过期的旧 token；强制刷新绝不复用被拒令牌。
    match result {
        Err(error) if !force && matches!(error.status_code, 429 | 500..=599) => {
            let id = account::select_id(store, requested)?;
            let (_, current) = account::load(store, &id)?;
            if !current.needs_relogin
                && !current.identity_pending
                && current.expires_at > logging::now_ms()
            {
                Ok(current)
            } else {
                Err(error)
            }
        }
        other => other,
    }
}

fn relogin() -> GatewayError {
    GatewayError::with_status(401, "ChatGPT 授权已失效，请重新登录").with_code("reauth_required")
}

async fn refresh_serialized(
    store: &AccountStore,
    id: &str,
    original: Credentials,
) -> Result<Credentials, GatewayError> {
    use crate::server::core::task_state::{self, Claim, ManualBackoff};
    let key = format!(
        "credentialRefresh:chatgpt:{}:{id}",
        refresh_flight::fingerprint(&store.file_string())
    );
    let started = Instant::now();
    let guard = loop {
        let (_, current) = account::load(store, id)?;
        if current.refresh_token != original.refresh_token
            || current.access_token != original.access_token
        {
            return read_newer(store, id);
        }
        if current.needs_relogin {
            return Err(relogin());
        }
        match task_state::claim(&key, 0, true, ManualBackoff::Bypass, 0)
            .map_err(|_| GatewayError::new("ChatGPT 刷新锁无法保存，未发送刷新请求"))?
        {
            Claim::Acquired(guard) => break guard,
            Claim::Deferred(_) if started.elapsed() < Duration::from_secs(25) => {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            Claim::Deferred(_) => {
                return Err(GatewayError::with_status(
                    409,
                    "ChatGPT 凭证正在其他进程刷新，请稍后重试",
                ))
            }
        }
    };
    let result = refresh_once(store, id, original).await;
    let success = result.is_ok();
    let finished = guard.finish(
        success,
        if success {
            "ChatGPT 凭证已续期"
        } else {
            "ChatGPT 凭证续期未完成"
        }
        .into(),
        None,
        0,
        0,
    );
    if finished.is_err() && success {
        return Err(GatewayError::new(
            "ChatGPT 新凭证已保存，但刷新状态收尾失败，请稍后重试",
        ));
    }
    result
}

async fn refresh_once(
    store: &AccountStore,
    id: &str,
    original: Credentials,
) -> Result<Credentials, GatewayError> {
    let (expected, mut credentials) = account::load(store, id)?;
    if credentials.needs_relogin {
        return Err(relogin());
    }
    if credentials.refresh_token != original.refresh_token
        && !credentials.identity_pending
        && credentials.expires_at > logging::now_ms()
    {
        return Ok(credentials);
    }
    let proxy = account::proxy(store, id)?;
    let client = http::client(proxy.as_ref())?;
    if credentials.identity_pending {
        return verify_pending(store, id, &expected, credentials, &client).await;
    }
    let response = http::tokens(
        &client,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &credentials.client_id),
            ("refresh_token", &credentials.refresh_token),
            ("resource", RESOURCE),
        ],
    )
    .await;
    let tokens = match response {
        Ok(tokens) => tokens,
        Err(error) if error.code.as_deref().is_some_and(error::requires_login) => {
            credentials.needs_relogin = true;
            let saved = store
                .plugin_update_account(
                    id,
                    &expected,
                    &credentials.secret(Some(&expected))?,
                    Some(&credentials.public()),
                )
                .map_err(|_| GatewayError::new("无法保存 ChatGPT 授权状态"))?;
            if !saved {
                return read_newer(store, id);
            }
            return Err(relogin());
        }
        Err(error) => return Err(error),
    };
    // 令牌端点已轮换 refresh token；先保存 replacement 的隔离态，再进行可能出网的
    // JWKS 验证。公钥网络中断时下次只续做验签，不会拿已作废的旧 refresh 再换一次。
    let previous_id = credentials.id_token.clone();
    let pending_id = tokens.id_token.clone();
    let permission_error = match credentials.apply_tokens(tokens) {
        Ok(()) => None,
        Err(error) if credentials.needs_relogin => Some(error),
        Err(error) => return Err(error),
    };
    if !pending_id.is_empty() {
        credentials.id_token = previous_id;
        credentials.pending_id_token = pending_id;
        credentials.identity_pending = true;
    }
    let next = credentials.secret(Some(&expected))?;
    if !store
        .plugin_update_account(id, &expected, &next, Some(&credentials.public()))
        .map_err(|_| GatewayError::new("ChatGPT 令牌已更新但未能保存，请重新登录"))?
    {
        return read_newer(store, id);
    }
    if let Some(error) = permission_error {
        return Err(error);
    }
    if credentials.identity_pending {
        return verify_pending(store, id, &next, credentials, &client).await;
    }
    Ok(credentials)
}

async fn verify_pending(
    store: &AccountStore,
    id: &str,
    expected: &Value,
    mut credentials: Credentials,
    client: &reqwest::Client,
) -> Result<Credentials, GatewayError> {
    let claims = match oidc::verify(
        client,
        &credentials.pending_id_token,
        &credentials.client_id,
        None,
        Some(&credentials.subject),
    )
    .await
    {
        Ok(claims) => claims,
        Err(error) if error.status_code == 400 => {
            credentials.needs_relogin = true;
            let saved = store
                .plugin_update_account(
                    id,
                    expected,
                    &credentials.secret(Some(expected))?,
                    Some(&credentials.public()),
                )
                .map_err(|_| GatewayError::new("无法保存 ChatGPT 授权校验状态"))?;
            return if saved {
                Err(relogin())
            } else {
                read_newer(store, id)
            };
        }
        Err(error) => return Err(error),
    };
    credentials.apply_identity(&claims)?;
    credentials.id_token = std::mem::take(&mut credentials.pending_id_token);
    credentials.identity_pending = false;
    let mut next = credentials.secret(Some(expected))?;
    if let Some(fields) = next.as_object_mut() {
        fields.remove("pendingIdToken");
    }
    if store
        .plugin_update_account(id, expected, &next, Some(&credentials.public()))
        .map_err(|_| GatewayError::new("无法保存 ChatGPT 授权校验结果"))?
    {
        return Ok(credentials);
    }
    read_newer(store, id)
}

fn read_newer(store: &AccountStore, id: &str) -> Result<Credentials, GatewayError> {
    let (_, current) = account::load(store, id)?;
    if current.needs_relogin {
        return Err(relogin());
    }
    if current.identity_pending || current.expires_at <= logging::now_ms() {
        return Err(GatewayError::with_status(
            409,
            "ChatGPT 凭证已发生变化或正在验证，请重试或重新登录",
        ));
    }
    Ok(current)
}

/// 导入只接受本应用已有的完整 SIWC 注册，不把普通 API Key/Codex 登录态误认成它。
pub(super) async fn validate_import(
    secret: Value,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
) -> Result<Credentials, GatewayError> {
    let mut credentials = Credentials::parse(&secret)?;
    if credentials.id_token.is_empty() {
        return Err(GatewayError::bad_request(
            "ChatGPT 导入需要完整的已签名身份凭证，请改用授权登录",
        ));
    }
    let client = http::client(proxy)?;
    let claims = oidc::verify(
        &client,
        &credentials.id_token,
        &credentials.client_id,
        None,
        Some(&credentials.subject),
    )
    .await?;
    credentials.apply_identity(&claims)?;
    credentials.host_id = super::host::get()?;
    Ok(credentials)
}
