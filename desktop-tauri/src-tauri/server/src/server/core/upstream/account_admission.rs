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
    // 最后一轮选路拿到的决策快照：排队超时也要把「当时谁可试、谁被什么挡住」
    // 带回转发层落库，否则请求日志里只剩一句「账号均忙」，看不出是并发满还是
    // 冷却/熔断/模型权限把它挡掉的。
    let mut last_snapshot: Option<Value> = None;
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
                if let Some(decision) = &target.decision { last_snapshot = Some(decision.clone()); }
                let limit = target.account.as_ref().map(crate::server::core::routing::max_concurrent_of).unwrap_or(0);
                if connection.try_rebind(target.account_id.clone(), limit) { return Ok(target); }
                if pinned.is_none() {
                    if let Some(key) = target.attempt_key() { excluded.push(key); }
                    if excluded.len().saturating_sub(tried.len()) < 32 { continue; }
                }
            }
            Err(error) => {
                if let Some(snapshot) = error.route_snapshot() { last_snapshot = Some(snapshot.clone()); }
                if !(error.code.as_deref() == Some("account_busy") || excluded.len() > tried.len()) {
                    return Err(error);
                }
            }
        }
        connection.rebind(None);
        let timed_out = || {
            let mut error = busy_timeout(providers, tried.len(), wait_started.elapsed(), wait_seconds);
            if let Some(snapshot) = &last_snapshot { error = error.with_route_snapshot(snapshot.clone()); }
            error
        };
        if Instant::now() >= deadline { return Err(timed_out()); }
        let token = telemetry.cancel_token();
        tokio::select! {
            biased;
            _ = async { match token { Some(token) => token.cancelled().await, None => std::future::pending().await } } => {
                return Err(cancellation::cancelled_error());
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err(timed_out());
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
