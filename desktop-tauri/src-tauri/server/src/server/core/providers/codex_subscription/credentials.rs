//! Codex 刷新令牌会轮换：进程内 refresh_flight 单飞、跨进程任务租约、落库 CAS。
//! Magpie account.go/codexRefresh：JSON 刷新、无 scope、主动临期/401 恢复；
//! 请求一旦发送即在独立任务内读完并落库，不随客户端断开丢掉新的刷新令牌。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::refresh_flight::{self, Join, Table};
use crate::server::core::task_state::{self, Claim, ManualBackoff};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{
    account::{self, Snapshot, Tokens},
    http, ID,
};

static FLIGHTS: OnceLock<Table<String>> = OnceLock::new();

pub(super) fn expiring_within(store: &AccountStore, id: &str, min_lead_ms: i64) -> bool {
    account::snapshot(store, id).ok().is_some_and(|account| {
        account.secret.get("needsRelogin").and_then(Value::as_bool) != Some(true)
            && !account.tokens.refresh.is_empty()
            && account.tokens.expiring_within(min_lead_ms)
    })
}

pub(super) fn expiring(store: &AccountStore, id: &str) -> bool {
    expiring_within(store, id, 0)
}

pub(super) async fn ensure(
    store: &AccountStore,
    id: &str,
    force: bool,
) -> Result<String, GatewayError> {
    let initial = account::snapshot(store, id)?;
    if initial.secret.get("needsRelogin").and_then(Value::as_bool) == Some(true) {
        return Err(relogin());
    }
    if !force && !initial.tokens.expiring() {
        return Ok(initial.tokens.access);
    }
    if initial.tokens.refresh.is_empty() {
        if !force
            && initial
                .tokens
                .expires_at
                .is_none_or(|expires| expires > logging::now_ms())
        {
            return Ok(initial.tokens.access);
        }
        return Err(GatewayError::with_status(
            401,
            "Codex 凭据已到期或被拒绝，且没有刷新令牌，请重新授权或导入",
        )
        .with_code("codex_relogin_required"));
    }
    let key = format!(
        "{ID}:{}:{}:{}",
        refresh_flight::fingerprint(&store.file_string()),
        initial.id,
        refresh_flight::fingerprint(&initial.tokens.refresh)
    );
    match FLIGHTS.get_or_init(Table::new).join(&key) {
        Join::Waiter(waiter) => {
            // 结果只充当本次完成信号；返回前重读，不能把重新登录前的旧令牌交给后续请求。
            waiter.wait().await?;
            usable(account::snapshot(store, &initial.id)?)
        }
        Join::Leader(leader) => {
            let store = store.clone();
            // JoinHandle 被调用者丢弃不取消真实刷新；leader 由任务持有，落库后才唤醒。
            crate::spawn_task(async move {
                let result = refresh_serialized(&store, initial, force).await;
                leader.finish(result.clone());
                result
            })
            .await
            .map_err(|_| {
                GatewayError::with_status(500, "Codex 刷新任务异常中断，请重新检查登录状态")
            })?
        }
    }
}

async fn refresh_serialized(
    store: &AccountStore,
    initial: Snapshot,
    force: bool,
) -> Result<String, GatewayError> {
    let lease_key = format!(
        "credentialRefresh:{ID}:{}:{}",
        refresh_flight::fingerprint(&store.file_string()),
        initial.id
    );
    let started = Instant::now();
    let guard = loop {
        let current = account::snapshot(store, &initial.id)?;
        if current.tokens.refresh != initial.tokens.refresh
            || current.tokens.access != initial.tokens.access
        {
            return usable(current);
        }
        if current.secret.get("needsRelogin").and_then(Value::as_bool) == Some(true) {
            return Err(relogin());
        }
        match task_state::claim(&lease_key, 0, true, ManualBackoff::Bypass, 0)
            .map_err(|_| GatewayError::with_status(500, "Codex 刷新锁无法保存，未发送刷新请求"))?
        {
            Claim::Acquired(guard) => break guard,
            Claim::Deferred(_) if started.elapsed() < Duration::from_secs(25) => {
                tokio::time::sleep(Duration::from_millis(150)).await
            }
            Claim::Deferred(_) => {
                return Err(GatewayError::with_status(
                    409,
                    "Codex 凭据正在其他进程刷新，请稍后重试",
                ))
            }
        }
    };
    let outcome = refresh_locked(store, initial, force).await;
    let success = outcome.is_ok();
    let finished = guard.finish(
        success,
        if success {
            "Codex 凭据已续期"
        } else {
            "Codex 凭据续期未完成"
        }
        .into(),
        None,
        0,
        0,
    );
    if finished.is_err() && success {
        return Err(GatewayError::with_status(
            500,
            "Codex 新凭据已保存，但刷新锁收尾失败，请稍后重试",
        ));
    }
    outcome
}

fn usable(account: Snapshot) -> Result<String, GatewayError> {
    if account.secret.get("needsRelogin").and_then(Value::as_bool) == Some(true) {
        return Err(relogin());
    }
    if account
        .tokens
        .expires_at
        .is_some_and(|expires| expires <= logging::now_ms())
    {
        return Err(GatewayError::with_status(
            409,
            "Codex 凭据已被更新但仍已过期，请稍后重新刷新",
        ));
    }
    Ok(account.tokens.access)
}

async fn refresh_locked(
    store: &AccountStore,
    initial: Snapshot,
    force: bool,
) -> Result<String, GatewayError> {
    let account = account::snapshot(store, &initial.id)?;
    if account.tokens.refresh != initial.tokens.refresh
        || account.tokens.access != initial.tokens.access
    {
        return usable(account);
    }
    if !force && !account.tokens.expiring() {
        return Ok(account.tokens.access);
    }
    let body = json!({ "client_id": http::CLIENT_ID, "grant_type": "refresh_token",
        "refresh_token": account.tokens.refresh,
        "refresh_reason": if force { "unauthorized_recovery" } else { "proactive_expiry" } });
    let request = http::with_headers(
        http::client(account.proxy.as_ref())?
            .post(http::TOKEN_URL)
            .json(&body),
        &http::client_headers(),
    );
    let reply = http::request(request, 1024 * 1024, "续期").await?;
    if !(200..300).contains(&reply.status) {
        // 先重读，重新授权/另一进程成功时不能把旧令牌失败写成新账号失效。
        let current = account::snapshot(store, &account.id)?;
        if current.tokens.refresh != account.tokens.refresh
            || current.tokens.access != account.tokens.access
        {
            return usable(current);
        }
        if refresh_gone(reply.status, &reply.body) {
            let mut secret = account.secret.clone();
            secret["needsRelogin"] = json!(true);
            let public = account.tokens.public(None, true);
            match store.plugin_update_account(&account.id, &account.secret, &secret, Some(&public))
            {
                Ok(true) => return Err(relogin()),
                Ok(false) => return usable(account::snapshot(store, &account.id)?),
                Err(_) => {
                    return Err(GatewayError::with_status(
                        500,
                        "Codex 授权已被拒绝，但状态保存失败，请重新授权",
                    ))
                }
            }
        }
        return Err(http::response_error("续期", &reply));
    }
    let access = account::text(&reply.body, &["access_token"]);
    if access.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "Codex 续期响应缺少访问令牌，未覆盖现有凭据",
        ));
    }
    // 先把凭据统一为本家平铺结构；原始 auth.json 的轮换旧值不留在嵌套字段里。
    let mut secret = account.secret.clone();
    let fields = secret
        .as_object_mut()
        .ok_or_else(|| GatewayError::with_status(401, "Codex 凭据格式无效"))?;
    for key in [
        "tokens",
        "auth",
        "access_token",
        "refresh_token",
        "id_token",
    ] {
        fields.remove(key);
    }
    if let Some(normalized) = account.tokens.secret().as_object() {
        for (key, value) in normalized {
            fields.insert(key.clone(), value.clone());
        }
    }
    fields.insert("accessToken".into(), json!(access));
    for (source, target) in [("refresh_token", "refreshToken"), ("id_token", "idToken")] {
        let value = account::text(&reply.body, &[source]);
        if !value.is_empty() {
            fields.insert(target.into(), json!(value));
        } else if target == "refreshToken" {
            fields.insert(target.into(), json!(account.tokens.refresh));
        } else {
            fields.insert(target.into(), json!(account.tokens.id_token));
        }
    }
    // 清掉旧 exp，未知时间保持未知，绝不借用上一个访问令牌的有效期。
    fields.remove("expiresAt");
    fields.remove("needsRelogin");
    if let Some(seconds) = reply
        .body
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
    {
        if let Some(expires) = seconds
            .checked_mul(1000)
            .and_then(|value| logging::now_ms().checked_add(value))
        {
            fields.insert("expiresAt".into(), json!(expires));
        }
    }
    let tokens = Tokens::read(&secret)?;
    if tokens.account_id != account.tokens.account_id
        || tokens.identity() != account.tokens.identity()
    {
        return Err(GatewayError::with_status(
            401,
            "Codex 续期返回了不同账号，未覆盖原凭据，请重新授权",
        ));
    }
    for (key, value) in tokens.secret().as_object().into_iter().flatten() {
        secret[key] = value.clone();
    }
    secret["lastRefreshAt"] = json!(logging::now_ms());
    let public = tokens.public(None, false);
    match store.plugin_update_account(&account.id, &account.secret, &secret, Some(&public)) {
        Ok(true) => Ok(tokens.access),
        Ok(false) => usable(account::snapshot(store, &account.id)?),
        Err(_) => Err(GatewayError::with_status(
            500,
            "Codex 新凭据未能保存，已停止继续发送；请检查账号库并重新授权",
        )),
    }
}

fn refresh_gone(status: u16, body: &Value) -> bool {
    let code = http::token_error_code(body)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        code.as_str(),
        "refresh_token_expired" | "refresh_token_reused" | "refresh_token_invalidated"
    ) || status == 400 && code == "invalid_grant"
        || status == 401 && !code.is_empty()
}

fn relogin() -> GatewayError {
    GatewayError::with_status(401, "Codex 登录授权已失效，请重新设备授权或导入新的凭据")
        .with_code("codex_relogin_required")
}
