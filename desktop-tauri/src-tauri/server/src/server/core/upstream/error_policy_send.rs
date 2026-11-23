use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::server::config;
use crate::server::core::providers::adapter::{ProviderAdapter, RetryDecision, UpstreamErrorClass};
use crate::server::core::providers::images::ImageResponseKind;
use crate::server::core::providers::policy::{ErrorPolicy, NetworkPolicy};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::error_policy;
use super::provider_loop::{OutboundFailure, RetryBudget};
use super::request::{read_upstream_error, TransportRequest};
use super::usage::{LogPhase, RequestTelemetry};

pub(super) async fn send_with_retry(
    adapter: &dyn ProviderAdapter,
    transport: &TransportRequest,
    budget: &mut RetryBudget,
    capture: Option<&crate::server::core::debug_traffic::TrafficCapture>,
    telemetry: &Arc<RequestTelemetry>,
    images: Option<ImageResponseKind>,
    network: Option<&NetworkPolicy>,
) -> Result<super::images::ImageResponse, OutboundFailure> {
    let mut network = super::network_fallback::Fallback::new(transport, if images.is_some() { None } else { network });
    let mut last_error = None;
    loop {
        if telemetry.replay_guard().blocked() || telemetry.response_started() {
            return Err(stopped(last_error.unwrap_or_else(super::replay_guard::blocked_error)));
        }
        if telemetry.is_cancelled() { return Err(stopped(super::cancellation::cancelled_error())); }
        telemetry.note_phase(LogPhase::Waiting);
        network.prepare_attempt();
        if let Some(capture) = capture {
            let actual = network.transport();
            if let Ok(body) = serde_json::from_str::<Value>(&actual.payload) {
                capture.reset_request(&actual.url, adapter.provider_id(), &actual.headers, &body);
            }
        }
        let response = match send_or_cancel(&mut network, telemetry).await {
            Ok(response) => { telemetry.note_attempt_response(); response }
            Err(error) => {
                let mut gateway = error.to_gateway_error();
                if telemetry.replay_guard().blocked() || telemetry.is_cancelled() {
                    return Err(stopped(if telemetry.is_cancelled() { super::cancellation::cancelled_error() } else { gateway }));
                }
                let decision = error_policy::for_error_with(&budget.policy, adapter.provider_id(), &budget.model, &gateway, !error.safe_to_retry);
                error_policy::attach(&mut gateway, &decision);
                error_policy::note(telemetry, &decision, None);
                if decision.hard_stop {
                    telemetry.note_attempt_notice("上游接收情况不确定，未重复提交或换号，以免重复执行或计费");
                    return Err(failure(error_policy::gateway_class(&gateway), gateway, true, ErrorPolicy::default()));
                }
                let delay = decision.retry_delay(0);
                if decision.retries() && budget.can_wait(delay) {
                    let fallback = network.after_transport(&error).await;
                    if telemetry.replay_guard().blocked() || telemetry.response_started() { return Err(stopped(gateway)); }
                    if budget.take_retry(delay) {
                        let reason = fallback.unwrap_or(error.reason);
                        note_retry(telemetry, budget, &reason, None, delay);
                        sleep_or_cancel(telemetry, delay).await;
                        last_error = Some(gateway);
                        continue;
                    }
                }
                return Err(failure(error_policy::gateway_class(&gateway), gateway, false, ErrorPolicy::default()));
            }
        };
        if let Some(kind) = images.filter(|_| response.status().is_success()) {
            match super::images::collect(response, kind, transport, telemetry).await {
                Ok(body) => return Ok(super::images::ImageResponse::Complete(body)),
                Err(result) => {
                    let class = if result.body["code"] == "content_policy_violation" {
                        UpstreamErrorClass::ContentBlocked { status: 400, message: "图片内容被上游拒绝".into(), upstream_code: None }
                    } else { adapter.classify_error_async(result.status, &result.body).await };
                    let mut error = error_policy::classified_error(&class, &result.body);
                    let uncertain = result.status >= 500 && matches!(class, UpstreamErrorClass::Fatal { .. });
                    let decision = error_policy::decide_with(&budget.policy, adapter.provider_id(), &budget.model,
                        &class, result.status, &result.body, false, uncertain);
                    error_policy::attach(&mut error, &decision);
                    error_policy::note(telemetry, &decision, None);
                    if uncertain { telemetry.note_attempt_notice("生图结果不确定，未自动重发，以免重复消耗额度"); }
                    return Err(failure(class, error, decision.hard_stop, ErrorPolicy::default()));
                }
            }
        }
        let status = response.status().as_u16();
        let is_error = adapter.is_error_response_async(status, response.headers()).await.map_err(stopped)?;
        if !is_error {
            network.success();
            return Ok(super::images::ImageResponse::Live(response));
        }
        let header_reset = super::retry_after::reset_at(response.headers());
        let timeout = Duration::from_millis(config::timeout_settings().body_ms());
        let read = tokio::time::timeout(timeout, read_upstream_error(response, capture));
        let detail = if let Some(cancel) = telemetry.cancel_token() {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(stopped(super::cancellation::cancelled_error())),
                result = read => result,
            }
        } else { read.await };
        let detail = detail.unwrap_or_else(|_| super::request::UpstreamErrorDetail {
            code: None, message: format!("非流式响应超时({}秒)", timeout.as_secs()), payload: None,
        });
        let body = detail.to_value();
        let mut class = adapter.classify_error_async(status, &body).await;
        super::retry_after::apply(&mut class, header_reset);
        let policy = adapter.error_policy_async(status, &body).await;
        let uncertain = images.is_some() && matches!(class, UpstreamErrorClass::Fatal { .. })
            && (status >= 500 || matches!(status, 408 | 409 | 425));
        let vision = telemetry.vision_enabled() && super::vision_compat::rejection_message(&detail.message);
        let stop = error_policy::plugin_stops(adapter.provider_id(), &policy, &class, status, &body) || uncertain || vision;
        let mut decision = error_policy::decide_with(&budget.policy, adapter.provider_id(), &budget.model,
            &class, status, &body, false, stop);
        if let Some(reset) = header_reset { decision.reset_at = Some(decision.reset_at.unwrap_or(reset).max(reset)); }
        let mut error = error_policy::classified_error(&class, &body);
        error_policy::attach(&mut error, &decision);
        error_policy::note(telemetry, &decision, None);
        if telemetry.replay_guard().blocked() || telemetry.response_started() { return Err(failure(class, error, true, policy)); }
        if uncertain { telemetry.note_attempt_notice("图片请求结果不确定，未自动重发，以免重复消耗额度"); }
        if decision.retries() && !policy.suspend_account && budget.remaining > 0 {
            let advice = adapter.retry_decision_async(status, &body, budget.used(), budget.total).await;
            let retry = match advice {
                RetryDecision::Retry { delay_ms, reason } => Some((decision.retry_delay(delay_ms), reason)),
                RetryDecision::Default | RetryDecision::Stop { .. } => Some((decision.retry_delay(0), "按统一错误规则在原账号重发".into())),
            };
            if let Some((delay, reason)) = retry {
                if budget.can_wait(delay) {
                    let fallback = network.after_status(status, &body, matches!(class, UpstreamErrorClass::Fatal { .. })).await;
                    if telemetry.replay_guard().blocked() || telemetry.response_started() { return Err(failure(class, error, true, policy)); }
                    if budget.take_retry(delay) {
                        note_retry(telemetry, budget, fallback.as_deref().unwrap_or(&reason), Some(i64::from(status)), delay);
                        sleep_or_cancel(telemetry, delay).await;
                        last_error = Some(error);
                        continue;
                    }
                }
                error_policy::note(telemetry, &decision, Some("重发次数或累计等待已用尽，不缩短上游恢复时间"));
            }
        }
        logging::console_line("[Upstream]", &format!("上游错误 HTTP {status}: {}", error.message));
        return Err(failure(class, error, decision.hard_stop, policy));
    }
}

fn failure(class: UpstreamErrorClass, error: GatewayError, stop: bool, policy: ErrorPolicy) -> OutboundFailure {
    OutboundFailure { class, error, stop, policy }
}

fn stopped(error: GatewayError) -> OutboundFailure {
    failure(UpstreamErrorClass::Fatal { status: error.status_code as u16, message: error.message.clone(), upstream_code: error.upstream_code },
        error, true, ErrorPolicy::default())
}

fn note_retry(telemetry: &RequestTelemetry, budget: &RetryBudget, reason: &str, status: Option<i64>, delay: u64) {
    telemetry.note_attempt_retry(reason, status, delay);
    logging::console_line("[Upstream]", &format!("{reason}，等待 {} 秒（重发 {}/{}）", delay as f64 / 1000.0, budget.used(), budget.total));
}

async fn send_or_cancel(network: &mut super::network_fallback::Fallback, telemetry: &RequestTelemetry)
    -> Result<reqwest::Response, super::request::UpstreamRequestError> {
    if let Some(cancel) = telemetry.cancel_token() {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(super::request::UpstreamRequestError {
                message: super::cancellation::MANUAL_TERMINATED.into(), reason: super::cancellation::MANUAL_TERMINATED.into(),
                safe_to_retry: false, network_failure: false,
            }),
            result = network.send() => result,
        }
    } else { network.send().await }
}

pub(super) async fn sleep_or_cancel(telemetry: &RequestTelemetry, delay_ms: u64) {
    let sleep = tokio::time::sleep(Duration::from_millis(delay_ms));
    if let Some(cancel) = telemetry.cancel_token() {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            _ = sleep => {},
        }
    } else { sleep.await }
}
