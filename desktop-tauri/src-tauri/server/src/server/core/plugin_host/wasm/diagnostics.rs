use std::sync::{Arc, Mutex};

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::{context::Context, fuel::Budget, limits::Limits};

pub(super) struct Invocation {
    provider: String,
    method: String,
    module: String,
    started: std::time::Instant,
    state: Mutex<State>,
}

struct State {
    phase: &'static str,
    account: String,
    request: Option<String>,
    snapshot: Value,
    reason: Option<&'static str>,
}

impl Invocation {
    pub(super) fn new(provider: &str, method: &str, module: &str) -> Arc<Self> {
        Arc::new(Self { provider: provider.to_string(), method: method.to_string(), module: module.to_string(),
            started: std::time::Instant::now(), state: Mutex::new(State { phase: "queued", account: String::new(),
                request: None, snapshot: Value::Null, reason: None }) })
    }

    pub(super) fn phase(&self, phase: &'static str) {
        self.state.lock().unwrap_or_else(|error| error.into_inner()).phase = phase;
    }

    pub(super) fn capture(&self, budget: &Budget, remaining: Option<u64>, context: &Context,
        limits: &Limits, input_bytes: usize, reason: Option<&'static str>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.account = context.account.clone();
        state.request = context.conversation.as_ref().map(|call| call.request_id().to_string());
        let mut snapshot = budget.snapshot(remaining);
        snapshot["inputBytes"] = json!(input_bytes);
        snapshot["upstreamPayloadBytes"] = json!(context.progress.upstream_bytes());
        snapshot["outputBytes"] = json!(context.progress.output_bytes());
        snapshot["hostCalls"] = json!(context.calls);
        snapshot["memoryBytes"] = json!(limits.linear_bytes);
        snapshot["memoryLimitBytes"] = json!(context.shared.options.manifest.memory_mib * 1024 * 1024);
        snapshot["tableBytes"] = json!(limits.table_bytes);
        snapshot["stackLimitBytes"] = json!(super::compilation::MAX_WASM_STACK_BYTES);
        state.snapshot = snapshot;
        if let Some(reason) = reason { state.reason = Some(reason); }
    }

    pub(super) fn finish(&self, result: &Result<Value, proto::WireError>) {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let error = result.as_ref().err();
        let outcome = if error.is_some() { "interrupted" }
            else if result.as_ref().is_ok_and(|value| value.get("error").is_some()) { "provider_error" }
            else { "returned" };
        let stop_reason = error.and_then(|error| match error.code.as_deref() {
            Some("timeout") => Some("timeout"),
            Some(proto::WireError::ABORTED) => Some("cancelled"),
            Some("plugin_unavailable") => Some("unavailable"),
            _ => None,
        });
        let reason = stop_reason.or(state.reason).or_else(|| error.map(|error| match error.code.as_deref() {
            Some("plugin_busy") => "capacity",
            Some("plugin_resource_limit") => "resource",
            _ => "host_error",
        }));
        let message = match reason {
            Some("fuel_window") => "Wasm 插件计算额度耗尽（处理窗口）",
            Some("fuel_total") => "Wasm 插件计算额度耗尽（单次总额）",
            Some("stack") => "Wasm 插件执行栈溢出",
            Some("memory") => "Wasm 插件内存超限",
            Some("table") => "Wasm 插件表容量超限",
            Some("host_memory") => "Wasm 插件共享内存预算超限",
            Some("timeout") => "Wasm 插件调用超时",
            Some("cancelled") => "Wasm 插件调用已取消",
            Some("unavailable") => "Wasm 插件调用已关闭",
            Some(_) => "Wasm 插件执行未完成",
            None => "Wasm 插件计算用量",
        };
        let level = if matches!(reason, Some("cancelled" | "unavailable")) { "info" }
            else if error.is_some() { "warn" }
            else if self.method == proto::methods::gateway::CONVERSATION { "info" }
            else { "debug" };
        let data = json!({ "provider": self.provider, "action": self.method, "moduleDigest": self.module,
            "account": state.account, "requestId": state.request, "phase": state.phase,
            "outcome": outcome, "reason": reason, "code": error.and_then(|error| error.code.as_deref()),
            "durationMs": self.started.elapsed().as_millis() as u64, "compute": state.snapshot });
        drop(state);
        crate::server::logging::log_event(level, "plugin", message, Some(data));
    }
}
