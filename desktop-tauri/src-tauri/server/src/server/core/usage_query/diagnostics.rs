use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::network::FailureKind;
use crate::server::core::runtime_capacity::Allocation;

const MAX_REQUESTS: usize = 32;
const MAX_FAILURES: usize = 11;

pub(crate) struct Trace {
    provider: String,
    account: String,
    timeout_seconds: u64,
    started: Instant,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    attempts: u64,
    attempt_started: Option<Instant>,
    failures: Vec<Failure>,
    requests: Vec<Request>,
    omitted_requests: u64,
}

struct Failure {
    attempt: u64,
    duration_ms: u64,
    message: String,
    code: Option<String>,
    status: i32,
    kind: Option<FailureKind>,
    _memory: Allocation,
}

struct Request {
    attempt: u64,
    method: String,
    url: String,
    route: &'static str,
    proxy: Option<String>,
    started: Instant,
    finished: Option<Instant>,
    phase: &'static str,
    status: Option<u16>,
    kind: Option<FailureKind>,
    detail: Option<String>,
    _memory: Allocation,
}

pub(crate) struct RequestTrace {
    trace: std::sync::Weak<Trace>,
    index: usize,
}

tokio::task_local! {
    static CURRENT: Option<Arc<Trace>>;
}

pub(crate) async fn scope<F: std::future::Future>(trace: Option<Arc<Trace>>, work: F) -> F::Output {
    CURRENT.scope(trace, work).await
}

pub(crate) fn capture(provider: &str, account: &str) -> Option<Arc<Trace>> {
    current().filter(|trace| trace.provider == provider && trace.account == account)
}

pub(crate) fn current() -> Option<Arc<Trace>> {
    CURRENT.try_with(Clone::clone).ok().flatten()
}

pub(crate) fn bounded(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

pub(crate) fn safe_detail(text: &str) -> String {
    bounded(&crate::server::core::plugin_host::redact_diagnostic(text), 1024)
}

pub(crate) fn safe_url(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw) else { return String::new(); };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    bounded(&crate::server::core::debug_traffic::redact_url(parsed.as_str()), 1024)
}

impl Trace {
    pub(super) fn new(provider: &str, account: &str, timeout_seconds: u64) -> Arc<Self> {
        Arc::new(Self { provider: provider.to_string(), account: account.to_string(), timeout_seconds,
            started: Instant::now(), state: Mutex::new(State::default()) })
    }

    pub(super) fn start_attempt(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.attempts += 1;
        state.attempt_started = Some(Instant::now());
    }

    pub(super) fn failed(&self, message: &str, code: Option<&str>, status: i32) {
        let Ok(memory) = Allocation::new(8192) else { return; };
        let message = safe_detail(message);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.failures.len() >= MAX_FAILURES
            && !state.failures.last().is_some_and(|previous| previous.attempt == state.attempts) { return; }
        let attempt = state.attempts;
        let kind = FailureKind::from_code(code).or_else(|| {
            if status < 500 { return None; }
            state.requests.last().filter(|request| request.attempt == attempt).and_then(|request| request.kind)
        });
        let duration_ms = state.attempt_started.map(|started| started.elapsed().as_millis() as u64).unwrap_or(0);
        let failure = Failure { attempt, duration_ms, message,
            code: code.map(|code| bounded(code, 128)), status, kind, _memory: memory };
        if state.failures.last().is_some_and(|previous| previous.attempt == attempt) {
            state.failures.pop();
        }
        state.failures.push(failure);
    }

    pub(crate) fn request(self: &Arc<Self>, method: &str, url: &str,
        proxy: Option<&crate::server::core::proxies::ResolvedProxy>, system_proxy: bool) -> Option<RequestTrace> {
        let Ok(memory) = Allocation::new(16 * 1024) else { return None; };
        let url = safe_url(url);
        let route = if proxy.is_some() { "proxy" } else if system_proxy { "system" } else { "direct" };
        let proxy = proxy.map(|proxy| {
            let host = if proxy.host.contains(':') { format!("[{}]", proxy.host) } else { proxy.host.clone() };
            safe_url(&format!("{}://{host}:{}", proxy.protocol, proxy.port.unwrap_or(0)))
        });
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.requests.len() >= MAX_REQUESTS { state.omitted_requests += 1; return None; }
        let index = state.requests.len();
        let attempt = state.attempts;
        state.requests.push(Request { attempt, method: bounded(method, 16), url, route, proxy,
            started: Instant::now(), finished: None, phase: "connecting", status: None,
            kind: None, detail: None, _memory: memory });
        Some(RequestTrace { trace: Arc::downgrade(self), index })
    }

    pub(super) fn failure_row(&self, id: &str, name: &Value) -> Value {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let failure = state.failures.last();
        let request = state.requests.iter().rev().find(|request| request.kind.is_some());
        let newer_request = request.filter(|request| failure.is_none_or(|failure| request.attempt > failure.attempt));
        let kind = newer_request.and_then(|request| request.kind).or_else(|| failure.and_then(|failure| failure.kind));
        let code = if newer_request.is_some() { kind.map(|kind| kind.code().to_string()) } else {
            failure.and_then(|failure| failure.code.clone())
                .filter(|code| !matches!(code.as_str(), "timeout" | "usage_timeout"))
                .or_else(|| kind.map(|kind| kind.code().to_string()))
        };
        let message = kind.map(|kind| kind.message().to_string())
            .or_else(|| failure.map(|failure| failure.message.clone()));
        json!({ "id": id, "name": name, "usage": null,
            "error": message.unwrap_or_else(|| "余额查询超时（已达到本轮时间上限，可重新查询）".to_string()),
            "code": code.unwrap_or_else(|| if failure.is_some() { "usage_request_failed" } else { "usage_timeout" }.to_string()) })
    }

    pub(super) fn snapshot(&self, stop_reason: Option<&str>) -> Value {
        let now = Instant::now();
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let failure = state.failures.last();
        let request = state.requests.iter().rev().find(|request| request.kind.is_some());
        let kind = request.filter(|request| failure.is_none_or(|failure| request.attempt > failure.attempt))
            .and_then(|request| request.kind).or_else(|| failure.and_then(|failure| failure.kind));
        let requests = state.requests.iter().map(|request| json!({
            "attempt": request.attempt, "method": request.method, "url": request.url,
            "route": request.route, "proxy": request.proxy, "phase": request.phase,
            "status": request.status, "reason": request.kind.map(FailureKind::name), "detail": request.detail,
            "durationMs": request.finished.unwrap_or(now).saturating_duration_since(request.started).as_millis() as u64,
        })).collect::<Vec<_>>();
        let failures = state.failures.iter().map(|failure| json!({
            "attempt": failure.attempt, "durationMs": failure.duration_ms,
            "error": failure.message, "code": failure.code, "status": failure.status,
            "reason": failure.kind.map(FailureKind::name),
        })).collect::<Vec<_>>();
        json!({ "reason": kind.map(FailureKind::name), "stopReason": stop_reason,
            "timeoutSeconds": self.timeout_seconds, "attempts": state.attempts,
            "retries": state.attempts.saturating_sub(1), "durationMs": now.saturating_duration_since(self.started).as_millis() as u64,
            "requests": requests, "failures": failures, "omittedRequests": state.omitted_requests })
    }

    pub(super) fn attach(&self, row: &mut Value, stop_reason: Option<&str>) {
        row["diagnostics"] = self.snapshot(stop_reason);
        if row["error"].is_null() {
            row["diagnostics"]["reason"] = Value::Null;
            return;
        }
        if let Some(kind) = row["diagnostics"]["reason"].as_str().and_then(FailureKind::from_name) {
            if row["code"].as_str().is_none_or(|code| matches!(code, "internal" | "timeout" | "usage_timeout")
                || code.starts_with("usage_network_")) {
                row["error"] = json!(kind.message());
                row["code"] = json!(kind.code());
            }
        }
    }
}

impl RequestTrace {
    fn update(&self, change: impl FnOnce(&mut Request)) {
        let Some(trace) = self.trace.upgrade() else { return; };
        let mut state = trace.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(request) = state.requests.get_mut(self.index) { change(request); }
    }

    pub(crate) fn response(&self, status: u16) {
        self.update(|request| { request.status = Some(status); request.phase = "reading"; });
    }

    pub(crate) fn failed(&self, kind: FailureKind, detail: &str) {
        let detail = safe_detail(detail);
        self.update(|request| {
            request.kind = Some(kind); request.detail = Some(detail);
            request.phase = "failed"; request.finished = Some(Instant::now());
        });
    }

    pub(crate) fn finish(&self) {
        self.update(|request| { request.phase = "complete"; request.finished = Some(Instant::now()); });
    }
}

pub(crate) fn request_timeout(default_ms: Option<u64>) -> Option<u64> {
    let Some(deadline) = super::current_query_deadline().filter(|_| current().is_some()) else { return default_ms; };
    let remaining = Some(deadline.saturating_duration_since(tokio::time::Instant::now())
        .as_millis().min(u64::MAX as u128) as u64);
    match (default_ms, remaining) {
        (Some(default), Some(remaining)) => Some(default.min(remaining)),
        (default, remaining) => default.or(remaining),
    }
}

pub(super) fn retry_fits(delay: Duration) -> bool {
    super::current_query_deadline().is_none_or(|deadline|
        deadline.saturating_duration_since(tokio::time::Instant::now()) > delay + Duration::from_secs(1))
}

pub(super) fn log_result(job: &super::jobs::Job, row: &Value) {
    let provider = job.account["provider"].as_str().unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
    let code = row["code"].as_str();
    let error = row["error"].as_str();
    let not_configured = code == Some("usage_not_configured") || error == Some("没有可用凭证");
    let unavailable = code == Some("usage_unavailable");
    let waiting = super::recovery::requires_action(code);
    let timeout = matches!(code, Some("usage_timeout" | "usage_queue_timeout"));
    let reason = row["diagnostics"]["reason"].as_str().and_then(FailureKind::from_name);
    let summary = if error.is_none() { "余额查询完成".to_string() }
        else if not_configured { "余额查询未配置".to_string() }
        else if unavailable { "当前账号暂无可查额度".to_string() }
        else if waiting { "余额查询等待账号设置".to_string() }
        else if code == Some("usage_queue_timeout") { "余额查询排队超时".to_string() }
        else if timeout { "余额查询超时".to_string() }
        else if let Some(reason) = reason.filter(|_| code.is_some_and(|code| code.starts_with("usage_network_"))) {
            format!("余额查询失败：{}", reason.label())
        }
        else { "余额查询失败".to_string() };
    let level = if error.is_none() || not_configured || unavailable || waiting { "debug" } else { "warn" };
    let outcome = if error.is_none() { "success" } else if unavailable { "unavailable" }
        else if waiting { "needs_action" } else if timeout { "timeout" } else { "failed" };
    crate::server::logging::log_event(level, "account", &summary, Some(json!({
        "provider": provider, "providerLabel": crate::server::core::providers::label_of(provider),
        "account": job.id, "accountName": job.account["name"], "requestId": job.request_id,
        "action": "usage.query", "summary": summary, "outcome": outcome, "detail": error,
        "code": code, "timeoutSeconds": job.timeout_seconds,
        "durationMs": crate::server::logging::now_ms().saturating_sub(job.started_at),
        "retries": row["diagnostics"]["retries"],
        "retryLimit": crate::server::core::usage_records::query_retries_of(job.account.get("usageQuery")),
        "diagnostics": row["diagnostics"],
    })));
}
