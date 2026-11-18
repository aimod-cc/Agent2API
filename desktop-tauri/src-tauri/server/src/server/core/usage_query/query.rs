use serde_json::{json, Value};

use super::{diagnostics, network::FailureKind};

pub(super) enum UsageFailure {
    NoCredentials,
    Request(String, Option<String>, i32),
}

pub(super) async fn query_usage_for(
    store: &crate::server::core::account_store::AccountStore,
    account: &Value,
) -> Value {
    let id = account["id"].as_str().unwrap_or("");
    let provider = account["provider"].as_str().unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
    let trace = diagnostics::current();
    let has_credentials = account.get("hasCredentials").and_then(Value::as_bool) != Some(false)
        || account.get("balanceQuery").and_then(|query| query.get("scheme")).is_some();
    let retry_limit = crate::server::core::usage_records::query_retries_of(account.get("usageQuery"));
    let mut stop_reason = None;
    let outcome = if has_credentials {
        let mut refresh_available = true;
        let mut retries = 0;
        loop {
            if super::current_query_deadline().is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                stop_reason = Some("deadline");
                break Err(UsageFailure::Request("余额查询超时（已达到本轮时间上限，可重新查询）".to_string(),
                    Some("usage_timeout".to_string()), 504));
            }
            if let Some(trace) = &trace { trace.start_attempt(); }
            let result = super::query_usage_inner(store, provider, id, &mut refresh_available).await;
            if matches!(&result, Err(UsageFailure::Request(_, code, _))
                if matches!(code.as_deref(), Some("usage_timeout" | "timeout")))
                && super::current_query_deadline().is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                stop_reason = Some("deadline");
                break result;
            }
            if let (Some(trace), Err(UsageFailure::Request(message, code, status))) = (&trace, &result) {
                trace.failed(message, code.as_deref(), *status);
            }
            let retryable = matches!(&result, Err(UsageFailure::Request(_, code, status))
                if matches!(*status, 408 | 429 | 500..=599) && !super::no_usage_retry(code.as_deref()));
            if !retryable || retries >= retry_limit { break result; }
            let delay = std::time::Duration::from_secs(1 << retries.min(2));
            if !diagnostics::retry_fits(delay) { stop_reason = Some("retry_budget"); break result; }
            tokio::time::sleep(delay).await;
            if !diagnostics::retry_fits(std::time::Duration::ZERO) {
                stop_reason = Some("retry_budget"); break result;
            }
            retries += 1;
        }
    } else { Err(UsageFailure::NoCredentials) };
    let mut row = match outcome {
        Ok(usage) => json!({ "id": id, "name": account["name"], "usage": usage, "error": null }),
        Err(UsageFailure::NoCredentials) => json!({ "id": id, "usage": null, "error": "没有可用凭证" }),
        Err(UsageFailure::Request(message, code, _)) => {
            let kind = FailureKind::from_code(code.as_deref());
            json!({ "id": id, "name": account["name"], "usage": null,
                "error": kind.map(|kind| kind.message().to_string()).unwrap_or_else(|| diagnostics::safe_detail(&message)),
                "code": code })
        }
    };
    if let Some(trace) = trace {
        if stop_reason == Some("deadline") { row = trace.failure_row(id, &account["name"]); }
        trace.attach(&mut row, stop_reason);
    }
    row
}
