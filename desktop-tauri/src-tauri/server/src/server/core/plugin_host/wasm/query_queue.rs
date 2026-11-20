use std::sync::Arc;

use agent2api_provider_protocol as proto;
use tokio::sync::OwnedMutexGuard;

use super::context::Shared;
use super::super::activity::UsageLease;
use crate::server::errors::GatewayError;

pub(crate) const WAIT_SECONDS: u64 = 300;

pub(crate) struct Reservation {
    shared: Arc<Shared>,
    account: String,
    _account: OwnedMutexGuard<()>,
    _ticket: crate::server::core::runtime_capacity::Ticket,
    _lease: Arc<UsageLease>,
}

tokio::task_local! {
    static CURRENT: Option<Arc<Reservation>>;
}

pub(crate) async fn scope<F: std::future::Future>(reservation: Option<Arc<Reservation>>, work: F) -> F::Output {
    CURRENT.scope(reservation, work).await
}

fn query_method(method: &str) -> bool {
    matches!(method, proto::methods::gateway::USAGE_QUERY | proto::methods::gateway::TOKEN_ENSURE
        | proto::methods::gateway::TOKEN_REFRESH)
}

pub(in crate::server::core::plugin_host) fn capture(provider: &str, account: &str, method: &str) -> Option<Arc<Reservation>> {
    if !query_method(method) || !crate::server::core::usage_query::is_account_query(provider, account) { return None; }
    CURRENT.try_with(|current| current.as_ref().filter(|reservation|
        reservation.shared.options.provider == provider && reservation.account == account).cloned()).ok().flatten()
}

pub(super) fn reserved(shared: &Arc<Shared>, account: &str, method: &str) -> Result<bool, proto::WireError> {
    let Some(reservation) = capture(&shared.options.provider, account, method) else { return Ok(false); };
    if !Arc::ptr_eq(&reservation.shared, shared) || shared.cancel.is_cancelled() {
        return Err(proto::WireError::with_code("余额查询所属的插件运行实例已变化", "plugin_unavailable"));
    }
    Ok(true)
}

fn unavailable() -> GatewayError {
    GatewayError::with_status(503, "提供商插件当前不可用，请检查插件状态").with_code("plugin_unavailable")
}

pub(crate) async fn reserve(provider: &str, account: &str) -> Result<Option<Arc<Reservation>>, GatewayError> {
    let Some(host) = super::super::host_for_provider(provider).filter(|host| host.options.wasm.is_some()) else {
        return Ok(None);
    };
    let runtime = {
        let _lease = host.activity.acquire().ok_or_else(unavailable)?;
        host.ensure().await.map_err(|_| unavailable())?;
        host.embedded.runtime.read().await.clone().ok_or_else(unavailable)?
    };
    let shared = runtime.shared.clone();
    if shared.options.provider != provider || account.is_empty() { return Err(unavailable()); }
    // 等待时不持使用租约，也不创建WASM实例；真实槽位就绪后才保护执行代次。
    let (account_guard, ticket) = tokio::select! {
        biased;
        _ = shared.cancel.cancelled() => return Err(unavailable()),
        result = async {
            use crate::server::core::runtime_capacity::{self as capacity, AcquireError, Class};
            let guard = shared.account_lock(&format!("{provider}:{account}:usage-query")).lock_owned().await;
            let ticket = match capacity::acquire_detailed(Class::Background, provider, &capacity::caller(), 512,
                tokio::time::Instant::now() + std::time::Duration::from_secs(WAIT_SECONDS), &shared.cancel).await {
                Ok(ticket) => ticket,
                Err(AcquireError::Cancelled) => return Err(unavailable()),
                Err(AcquireError::Rejected(rejection)) => {
                    crate::server::logging::log_with_level(
                        "[Plugin]",
                        &format!(
                            "⚠️ 余额查询容量排队拒绝 (503 plugin_busy): 提供商={provider} {}",
                            rejection.describe(),
                        ),
                        "warn",
                    );
                    return Err(GatewayError::with_status(503, rejection.message())
                        .with_code("plugin_busy")
                        .with_reason(rejection.reason.code())
                        .with_retry_after_seconds(rejection.retry_after_seconds()));
                }
            };
            Ok::<_, GatewayError>((guard, ticket))
        } => result?,
    };
    let lease = host.activity.acquire().ok_or_else(unavailable)?;
    let current = host.embedded.runtime.try_read().map_err(|_| unavailable())?;
    if !host.ready() || shared.cancel.is_cancelled()
        || current.as_ref().is_none_or(|current| !Arc::ptr_eq(current, &runtime))
        || super::super::host_for_provider(provider).is_none_or(|current| !Arc::ptr_eq(&current, &host)) {
        return Err(unavailable());
    }
    Ok(Some(Arc::new(Reservation { shared, account: account.to_string(),
        _account: account_guard, _ticket: ticket, _lease: lease })))
}
