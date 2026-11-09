use std::sync::Arc;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::providers::adapter::{ProviderAdapter, RetryAdvice, RetryDecision, UpstreamErrorClass};
use crate::server::errors::GatewayError;

use super::{ForwardOutcome, usage::RequestTelemetry};

pub(super) async fn forward(
    adapter: &dyn ProviderAdapter,
    store: &AccountStore,
    account: &str,
    body: &Value,
    headers: &HeaderMap,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
    budget: &mut super::provider_loop::RetryBudget,
) -> Result<ForwardOutcome, GatewayError> {
    let mut refreshed = false;
    loop {
        let result = adapter.forward_conversation(store, account, body, headers, proxy.clone(), stream, telemetry).await;
        let Err(error) = result else { return result; };
        if telemetry.is_cancelled() { return Err(super::cancellation::cancelled_error()); }
        if matches!(error.code.as_deref(), Some("plugin_response_started" | "account_model_unavailable"
            | "account_model_permissions_unknown" | "plugin_protocol_error" | "plugin_resource_limit"
            | "plugin_trap" | "forbidden" | "native_helper_not_authorized")) { return Err(error); }
        // 只有无状态且尚未交付响应的调用能刷新后重发；私有持久会话保持原行为。
        if adapter.is_stateful() || !adapter.forwards_response() {
            return Err(error);
        }
        let status = u16::try_from(error.status_code).unwrap_or(502);
        let settings = crate::server::config::retry_settings();
        let class = adapter.classify_conversation_error(&error);
        let detail = serde_json::json!({ "message": error.message, "code": error.code });
        let policy = adapter.error_policy_async(status, &detail).await;
        if policy.stop || settings.no_retry(status) || policy.suspend_account || policy.account_cooldown_until.is_some() {
            return Err(error);
        }
        if !matches!(class, UpstreamErrorClass::TokenExpired { .. }) {
            if budget.remaining == 0 { return Err(error); }
            let decision = adapter.retry_decision_async(status, &detail, budget.used(), budget.total).await;
            let advice = match decision {
                RetryDecision::Retry { delay_ms, reason } => Some(RetryAdvice { delay_ms, reason }),
                RetryDecision::Stop { .. } => None,
                RetryDecision::Default => super::provider_loop::transient_retry_advice(status, budget.remaining)
                    .or_else(|| super::provider_loop::fallback_retry_advice(&class, budget.remaining, status)),
            };
            let Some(advice) = advice else { return Err(error); };
            budget.remaining -= 1;
            let delay = advice.delay_ms.min(300_000);
            telemetry.note_attempt_retry(&advice.reason, Some(i64::from(status)), delay);
            if let Some(cancel) = telemetry.cancel_token() {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(super::cancellation::cancelled_error()),
                    _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {}
                }
            } else { tokio::time::sleep(std::time::Duration::from_millis(delay)).await; }
            continue;
        }
        if refreshed || !adapter.supports_refresh() || account.is_empty() { return Err(error); }
        let refresh = adapter.refresh_access_token(store, account);
        if let Some(cancel) = telemetry.cancel_token() {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(super::cancellation::cancelled_error()),
                result = refresh => { result?; }
            }
        } else { refresh.await?; }
        telemetry.note_attempt_notice("登录凭证已刷新，在原账号重试一次");
        refreshed = true;
    }
}
