use serde_json::{json, Value};

use super::RequestTelemetry;
use crate::server::core::upstream::vision_compat::VisionCall;

impl RequestTelemetry {
    pub(crate) fn set_vision_continuation(&self, value: std::sync::Arc<crate::server::core::upstream::vision_compat::VisionContinuation>) {
        *self.vision_continuation.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value);
    }

    pub(crate) fn vision_continuation(&self) -> Option<std::sync::Arc<crate::server::core::upstream::vision_compat::VisionContinuation>> {
        self.vision_continuation.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub(crate) fn set_vision_enabled(&self, enabled: bool) {
        self.vision_enabled.store(enabled, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn vision_enabled(&self) -> bool {
        self.vision_enabled.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn note_vision(&self, detail: Value) {
        let mut guard = self.lock();
        let route = guard.route.get_or_insert_with(|| json!({}));
        if let Some(object) = route.as_object_mut() { object.insert("visionCompatibility".into(), detail); }
        guard.route_revision = guard.route_revision.wrapping_add(1);
        self.flush_live(&guard);
    }

    pub(crate) fn note_vision_call(&self, call: VisionCall) {
        let mut guard = self.lock();
        if guard.vision_calls.len() < 20 { guard.vision_calls.push(call); }
        let calls: Vec<_> = guard.vision_calls.iter().map(|call| json!({
            "id": call.snapshot.id, "provider": call.snapshot.provider, "model": call.snapshot.upstream_model,
            "account": call.snapshot.account_name, "durationMs": call.finished_at.saturating_sub(call.started_at),
            "totalTokens": call.snapshot.total_tokens, "status": call.status, "error": call.error,
        })).collect();
        let input: i64 = guard.vision_calls.iter().map(|call| call.snapshot.prompt_tokens).sum();
        let output: i64 = guard.vision_calls.iter().map(|call| call.snapshot.completion_tokens).sum();
        let total: i64 = guard.vision_calls.iter().map(|call| call.snapshot.total_tokens).sum();
        if let Some(detail) = guard.route.as_mut().and_then(|route| route.get_mut("visionCompatibility")) {
            detail["calls"] = json!(calls);
            detail["promptTokens"] = input.into();
            detail["completionTokens"] = output.into();
            detail["totalTokens"] = total.into();
        }
        guard.route_revision = guard.route_revision.wrapping_add(1);
        self.flush_live(&guard);
    }

    pub(crate) fn take_vision_calls(&self) -> Vec<VisionCall> {
        std::mem::take(&mut self.lock().vision_calls)
    }

    pub(crate) fn prepare_vision_retry(&self) {
        let mut guard = self.lock();
        guard.error = None;
        guard.first_response_at = None;
        guard.prompt_tokens = 0;
        guard.completion_tokens = 0;
        guard.total_tokens = 0;
        guard.cache_read_tokens = 0;
        guard.credit = None;
        self.enter_phase(&mut guard, super::LogPhase::Connecting);
        self.flush_live(&guard);
    }
}
