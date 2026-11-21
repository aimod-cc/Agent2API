use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use super::{cancellation, connections::ConnectionGuard, rotate, route_context::RequestRouting,
    usage::RequestTelemetry, RouteTarget, UpstreamService};
use crate::server::errors::GatewayError;

pub(super) async fn select(service: &UpstreamService, providers: &[&str], keys: &rotate::CooldownKeys<'_>,
    tried: &[String], pinned: Option<&str>, conversation: &str, routing: &RequestRouting,
    connection: &mut ConnectionGuard, telemetry: &Arc<RequestTelemetry>,
) -> Result<RouteTarget, GatewayError> {
    let wait_seconds = crate::server::core::runtime_capacity::limits().queue_wait_seconds;
    let wait_started = Instant::now();
    let deadline = wait_started + Duration::from_secs(wait_seconds);
    let mut excluded = tried.to_vec();
    // 兜底最多走 8 次：每次兜底都把一个账号写进 `excluded`，正常几轮就扫完了；
    // 这个计数器只防「`attempt_key` 为空时 excluded 不增长」的死循环。
    let mut fallback_left = 8usize;
    let notify = service.connections().changed_notify();
    loop {
        let wake = notify.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        if telemetry.is_cancelled() { return Err(cancellation::cancelled_error()); }
        let selected = rotate::select_target_account(service, providers, keys, &excluded, pinned, conversation, routing).await;
        match selected {
            Ok(target) if target.account_id.is_none() && excluded.len() > tried.len() => {},
            Ok(target) => {
                let limit = target.account.as_ref().map(crate::server::core::routing::max_concurrent_of).unwrap_or(0);
                if connection.try_rebind(target.account_id.clone(), limit) { return Ok(target); }
                if pinned.is_none() {
                    if let Some(key) = target.attempt_key() { excluded.push(key); }
                    if excluded.len().saturating_sub(tried.len()) < 32 { continue; }
                }
            }
            Err(error) if error.code.as_deref() == Some("account_busy") || excluded.len() > tried.len() => {},
            Err(error) => {
                // ── 兜底：常规选路一个候选都没给出来 ─────────────────────
                // 走到这里说明 `select_target_account` 直接判定「无对象可发」：
                // 全在冷却（`routing_cooldown`）、全被限制器拦、全被禁用、
                // 全忙…… 这些判据**全是我们自己记的账**（冷却时长、限制器读数、
                // 并发计数）。用它把整条请求判死，等于自己的账本把自己的活掐了。
                //
                // 于是按配置里的原始顺序再走一遍（`fallback_sweep`），只跳过
                // 禁用、取不到凭证、以及本请求已经试过的。撞一次总比直接 429
                // / 503 让下游中断好 —— 而且真失败了，下次冷却估算会更长，
                // 不会在同一个账号上原地打转。
                if fallback_left == 0 { return Err(error); }
                fallback_left -= 1;
                let Some(account) = rotate::fallback_sweep(service, providers, keys, &excluded, pinned, routing) else {
                    return Err(error);
                };
                let Some(target) = rotate::target_from_account(service, account, None) else {
                    return Err(error);
                };
                crate::server::logging::log_with_level(
                    "[Upstream]",
                    &format!("⚠️ 常规选路报错（HTTP {} {}），改按兜底顺序尝试 {}/{}",
                        error.status_code, error.message, target.provider,
                        target.account_id.as_deref().unwrap_or("-")),
                    "warn",
                );
                let limit = target.account.as_ref().map(crate::server::core::routing::max_concurrent_of).unwrap_or(0);
                if connection.try_rebind(target.account_id.clone(), limit) { return Ok(target); }
                if let Some(key) = target.attempt_key() { excluded.push(key); }
                continue;
            }
        }
        connection.rebind(None);
        if Instant::now() >= deadline {
            return Err(busy_timeout(providers, tried.len(), wait_started.elapsed(), wait_seconds));
        }
        let token = telemetry.cancel_token();
        tokio::select! {
            biased;
            _ = async { match token { Some(token) => token.cancelled().await, None => std::future::pending().await } } => {
                return Err(cancellation::cancelled_error());
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err(busy_timeout(providers, tried.len(), wait_started.elapsed(), wait_seconds));
            }
            _ = &mut wake => {},
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
        excluded.clear();
        excluded.extend_from_slice(tried);
    }
}

fn busy_timeout(providers: &[&str], tried_count: usize, waited: Duration, wait_seconds: u64) -> GatewayError {
    crate::server::logging::log_with_level(
        "[Upstream]",
        &format!(
            "⚠️ 路由内可用账号均忙，排队超时 (503 account_busy): 提供商=[{}] 已尝试排除={} 等待={}ms 上限={}s",
            providers.join(","),
            tried_count,
            waited.as_millis(),
            wait_seconds,
        ),
        "warn",
    );
    busy()
}

fn busy() -> GatewayError {
    GatewayError::with_status(503, "当前路由内的账号均忙，排队等待超时，请稍后重试")
        .with_code("account_busy").with_reason("account_busy").with_retry_after_seconds(1)
}

pub(super) fn all_busy(accounts: &[Value], counts: &std::collections::HashMap<String, usize>, tried: &[String]) -> bool {
    let mut found = false;
    for account in accounts {
        let Some(id) = crate::server::core::routing::account_id(account) else { continue; };
        if account["enabled"] == false || account["hasCredentials"] == false
            || tried.contains(&crate::server::core::routing_groups::attempt_key(account)) { continue; }
        let limit = crate::server::core::routing::max_concurrent_of(account);
        if limit == 0 || counts.get(id).copied().unwrap_or(0) < limit as usize { return false; }
        found = true;
    }
    found
}
