use serde_json::Value;

use super::{truncate_chars, LogPhase, RequestTelemetry, TelemetrySnapshot};

pub const MAX_ATTEMPT_DETAILS: usize = 24;
pub const MAX_ATTEMPT_ERROR_CHARS: usize = 200;
pub const MAX_ATTEMPT_ACCOUNT_CHARS: usize = 60;
pub const MAX_ATTEMPT_RETRY_REASON_CHARS: usize = 120;
pub const MAX_ATTEMPT_NOTICE_CHARS: usize = 200;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AttemptDetail {
    pub provider: String,
    #[serde(default)]
    pub account: String,
    #[serde(rename = "accountId", default)]
    pub account_id: String,
    #[serde(rename = "candidateId", default)]
    pub candidate_id: String,
    #[serde(rename = "memberId", default)]
    pub member_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub reason: String,
    #[serde(rename = "stickyHit", default)]
    pub sticky_hit: bool,
    #[serde(rename = "startedAt", default)]
    pub started_at: Option<i64>,
    /// 响应头或会话式转发接收结果，不代表流式响应已经结束。
    #[serde(rename = "responseAt", default)]
    pub response_at: Option<i64>,
    #[serde(rename = "finishedAt", default)]
    pub finished_at: Option<i64>,
    #[serde(default)]
    pub status: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub retries: Vec<RetryEvent>,
    #[serde(default)]
    pub notice: Option<String>,
    #[serde(rename = "bodyBytes", default)]
    pub body_bytes: Option<i64>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RetryEvent {
    pub reason: String,
    #[serde(default)]
    pub status: Option<i64>,
    #[serde(rename = "delayMs", default)]
    pub delay_ms: u64,
}

impl TelemetrySnapshot {
    pub(super) fn current_attempt_mut(&mut self) -> Option<&mut AttemptDetail> {
        if self.attempts > MAX_ATTEMPT_DETAILS as i64 { return None; }
        self.attempts_detail.last_mut()
    }

    pub fn finish_trace(&mut self, finished_at: i64, error: Option<&str>) {
        let Some(last) = self.current_attempt_mut() else { return };
        if last.finished_at.is_none() {
            last.finished_at = Some(finished_at.max(last.started_at.unwrap_or(finished_at)));
        }
        if let Some(error) = error.filter(|text| !text.is_empty()) {
            if last.error.is_none() {
                last.error = Some(truncate_chars(error, MAX_ATTEMPT_ERROR_CHARS));
            }
        }
    }
}

impl RequestTelemetry {
    pub fn set_route_decision(&self, decision: Value) {
        let mut guard = self.lock();
        if guard.route.as_ref() == Some(&decision) { return; }
        guard.route = Some(decision);
        guard.route_revision = guard.route_revision.wrapping_add(1);
        self.flush_live(&guard);
    }

    pub fn route_decision(&self) -> Option<Value> {
        self.lock().route.clone()
    }

    pub fn note_attempt_response(&self) {
        let mut guard = self.lock();
        let Some(last) = guard.current_attempt_mut() else { return };
        if last.response_at.is_none() {
            last.response_at = Some(crate::server::logging::now_ms());
            self.flush_live(&guard);
        }
    }

    pub fn note_attempt_started(&self, provider: &str, account: &str) {
        let mut guard = self.lock();
        if guard.attempts_detail.len() >= MAX_ATTEMPT_DETAILS { return; }
        let account_id = guard.account_id.clone();
        let candidate_id = guard.route.as_ref().and_then(|route| route.get("pickedCandidate")).and_then(Value::as_str).unwrap_or(&account_id).to_string();
        let member_id = crate::server::core::routing_groups::member_key(&candidate_id).unwrap_or("").to_string();
        let reason = guard.route.as_ref()
            .and_then(|route| route.get("ordered")).and_then(Value::as_array)
            .and_then(|entries| entries.iter().find(|entry| entry.get("candidateId").and_then(Value::as_str).unwrap_or_else(|| entry.get("id").and_then(Value::as_str).unwrap_or("")) == candidate_id))
            .and_then(|entry| entry.get("reason")).and_then(Value::as_str).unwrap_or("");
        let reason = truncate_chars(reason, MAX_ATTEMPT_NOTICE_CHARS);
        let sticky_hit = guard.route.as_ref().and_then(|route| route.get("stickyHit")).and_then(Value::as_bool).unwrap_or(false);
        guard.attempts_detail.push(AttemptDetail {
            provider: provider.to_string(), account: truncate_chars(account, MAX_ATTEMPT_ACCOUNT_CHARS),
            account_id, candidate_id, member_id, model: String::new(), reasoning: String::new(), reason, sticky_hit,
            started_at: Some(crate::server::logging::now_ms()), response_at: None, finished_at: None,
            status: None, error: None, retries: Vec::new(), notice: None, body_bytes: None,
        });
        self.enter_phase(&mut guard, LogPhase::Waiting);
        self.flush_live(&guard);
    }
}
