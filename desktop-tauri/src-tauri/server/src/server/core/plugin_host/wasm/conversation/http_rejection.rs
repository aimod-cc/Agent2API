//! 宿主侧 HTTP 200 响应体拒单观察器。
//!
//! 仅对已声明的白名单提供商（CodeArts / WorkBuddy 国内版 / 豆包工作），
//! 在宿主自身通过 `http.read` 观察到完整、明确的拒单/限流帧（且未产生任何
//! 回答或受理证据、未超 256 KiB 检查窗口）时，才允许释放当前请求的防重放保护。
//! 绝不信任插件返回值解锁；任何未知响应、正常回答或异常均保持永久锁定。

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::Value;

use crate::server::core::upstream::replay_guard::PreparationRef;

const MAX_INSPECT_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    CodeArts,
    WorkBuddy,
    DoubaoWork,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyMode {
    Sse,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Pending,
    Confirmed,
    Locked,
}

pub(in crate::server::core::plugin_host::wasm) struct HttpRejectionObserver {
    handle_id: String,
    provider: Provider,
    mode: BodyMode,
    state: State,
    buffer: Vec<u8>,
    preparation: PreparationRef,
}

fn header_content_type(result: &Value) -> Option<String> {
    let headers = result.get("headers")?.as_object()?;
    let raw = headers.iter().find_map(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            .then(|| value.as_str())
            .flatten()
    })?;
    let main = raw.split(';').next()?.trim().to_ascii_lowercase();
    (!main.is_empty()).then_some(main)
}

fn resolve_mode(provider: &str, result: &Value) -> Option<(Provider, BodyMode)> {
    if result["status"].as_u64() != Some(200) {
        return None;
    }
    let content_type = header_content_type(result);
    match provider {
        "codearts" => (content_type.as_deref() == Some("text/event-stream"))
            .then_some((Provider::CodeArts, BodyMode::Sse)),
        "workbuddy" => match content_type.as_deref() {
            Some("text/event-stream") => Some((Provider::WorkBuddy, BodyMode::Sse)),
            Some(kind) if kind == "application/json" || kind.ends_with("+json") => {
                Some((Provider::WorkBuddy, BodyMode::Json))
            }
            _ => None,
        },
        "doubaowork" => {
            if content_type.as_deref() == Some("text/event-stream") {
                Some((Provider::DoubaoWork, BodyMode::Sse))
            } else {
                Some((Provider::DoubaoWork, BodyMode::Json))
            }
        }
        _ => None,
    }
}

pub(in crate::server::core::plugin_host::wasm) fn supports_http_200_rejection_inspection(
    provider: &str,
    result: &Value,
) -> bool {
    resolve_mode(provider, result).is_some()
}

impl HttpRejectionObserver {
    pub(super) fn for_response(
        provider: &str,
        handle_id: &str,
        result: &Value,
        preparation: PreparationRef,
    ) -> Option<Self> {
        let (provider, mode) = resolve_mode(provider, result)?;
        Some(Self {
            handle_id: handle_id.to_string(),
            provider,
            mode,
            state: State::Pending,
            buffer: Vec::new(),
            preparation,
        })
    }

    pub(super) fn matches(&self, handle_id: &str) -> bool {
        self.handle_id == handle_id
    }

    pub(super) fn lock(&mut self) {
        self.state = State::Locked;
        self.buffer.clear();
        self.preparation.lock_permanent_replay_block();
    }

    fn confirm(&mut self) {
        self.state = State::Confirmed;
        self.preparation.confirm_unsubmitted_http_rejection();
    }

    pub(super) fn finish_on_close(&mut self) {
        if self.state == State::Pending {
            self.lock();
        }
    }

    pub(super) fn observe(&mut self, result: &Result<Value, proto::WireError>) {
        if self.state == State::Locked {
            return;
        }
        let Ok(value) = result else {
            self.lock();
            return;
        };
        if value["done"] == true {
            match self.mode {
                BodyMode::Json => {
                    if self.state == State::Pending {
                        let matched = serde_json::from_slice::<Value>(&self.buffer)
                            .ok()
                            .is_some_and(|json| self.is_json_rejection(&json));
                        if matched {
                            self.confirm();
                        } else {
                            self.lock();
                        }
                    }
                    self.buffer.clear();
                }
                BodyMode::Sse => {
                    let tail = std::mem::take(&mut self.buffer);
                    if !tail.iter().all(u8::is_ascii_whitespace) {
                        self.process_sse_block(&tail);
                    }
                    if self.state == State::Pending {
                        self.lock();
                    }
                }
            }
            return;
        }
        let Some(encoded) = value["data"].as_str() else {
            self.lock();
            return;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
            self.lock();
            return;
        };
        if self.mode == BodyMode::Json && self.state == State::Confirmed {
            self.lock();
            return;
        }
        if self.buffer.len().saturating_add(bytes.len()) > MAX_INSPECT_BYTES {
            self.lock();
            return;
        }
        self.buffer.extend_from_slice(&bytes);
        if self.mode == BodyMode::Sse {
            self.drain_sse();
        }
    }

    fn drain_sse(&mut self) {
        while self.state != State::Locked {
            let boundary = [
                self.buffer
                    .windows(2)
                    .position(|seq| seq == b"\n\n")
                    .map(|at| (at, 2)),
                self.buffer
                    .windows(4)
                    .position(|seq| seq == b"\r\n\r\n")
                    .map(|at| (at, 4)),
                self.buffer
                    .windows(2)
                    .position(|seq| seq == b"\r\r")
                    .map(|at| (at, 2)),
            ]
            .into_iter()
            .flatten()
            .min_by_key(|(at, _)| *at);
            let Some((at, len)) = boundary else {
                break;
            };
            let block: Vec<u8> = self.buffer.drain(..at + len).collect();
            self.process_sse_block(&block[..at]);
        }
    }

    fn process_sse_block(&mut self, block: &[u8]) {
        let Ok(text) = std::str::from_utf8(block) else {
            self.lock();
            return;
        };
        let mut event = "";
        let mut data_lines = Vec::new();
        for line in text.split(['\n', '\r']) {
            let line = line.trim_end_matches('\r');
            if let Some(rest) = line.strip_prefix("event:") {
                event = rest.trim();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        if data_lines.is_empty() {
            return;
        }
        let data = data_lines.join("\n");
        let trimmed = data.trim();
        if trimmed.is_empty() {
            return;
        }
        if trimmed == "[DONE]" {
            if self.state == State::Pending {
                self.lock();
            }
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            self.lock();
            return;
        };
        if is_pure_heartbeat(event, &value) {
            return;
        }
        if self.is_sse_rejection(event, &value) {
            self.confirm();
        } else {
            self.lock();
        }
    }

    fn is_json_rejection(&self, value: &Value) -> bool {
        match self.provider {
            Provider::CodeArts => false,
            Provider::WorkBuddy => is_workbuddy_rejection(value),
            Provider::DoubaoWork => is_doubaowork_rejection("", value),
        }
    }

    fn is_sse_rejection(&self, event: &str, value: &Value) -> bool {
        match self.provider {
            Provider::CodeArts => is_codearts_rejection(value),
            Provider::WorkBuddy => is_workbuddy_rejection(value),
            Provider::DoubaoWork => is_doubaowork_rejection(event, value),
        }
    }
}

fn is_heartbeat_name(name: &str) -> bool {
    name.len() <= 16
        && matches!(
            name.to_ascii_lowercase().as_str(),
            "ping" | "pong" | "heartbeat" | "keepalive" | "keep-alive"
        )
}

fn is_pure_heartbeat(event: &str, value: &Value) -> bool {
    let kind = value["type"]
        .as_str()
        .or_else(|| value["event"].as_str())
        .unwrap_or("");
    if !is_heartbeat_name(event) && !is_heartbeat_name(kind) {
        return false;
    }
    let Some(obj) = value.as_object() else {
        return false;
    };
    !obj.iter().any(|(key, val)| match key.as_str() {
        "type" | "event" => false,
        "code" | "error_code" => scalar_i64(val).is_some_and(|n| n != 0)
            || val
                .as_str()
                .is_some_and(|s| !matches!(s.trim().to_ascii_lowercase().as_str(), "" | "0" | "0000" | "success")),
        "error" => !val.is_null() && val != false,
        _ => !val.is_null(),
    })
}

fn scalar_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

fn delta_has_content(node: &Value) -> bool {
    let Some(obj) = node.as_object() else {
        return false;
    };
    let has_content = match obj.get("content") {
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(parts)) => parts
            .iter()
            .any(|part| part["text"].as_str().is_some_and(|t| !t.is_empty())),
        _ => false,
    };
    has_content
        || obj
            .get("reasoning_content")
            .and_then(Value::as_str)
            .is_some_and(|t| !t.is_empty())
        || obj
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty())
}

fn is_codearts_rejection(value: &Value) -> bool {
    if value["choices"]
        .as_array()
        .is_some_and(|choices| !choices.is_empty())
        || value["usage"].is_object()
        || delta_has_content(&value["delta"])
        || value["text"]
            .as_str()
            .is_some_and(|t| !t.trim().is_empty() && t.trim() != "[DONE]")
    {
        return false;
    }
    let has_error = value
        .get("error")
        .is_some_and(|error| !error.is_null() && error != false);
    let has_code = ["error_code", "code"].iter().any(|key| {
        value.get(*key).is_some_and(|code| {
            code.as_i64().is_some_and(|n| n != 0)
                || code.as_str().is_some_and(|s| {
                    let s = s.trim().to_ascii_lowercase();
                    !s.is_empty() && !matches!(s.as_str(), "0" | "0000" | "success")
                })
        })
    });
    if !has_error && !has_code {
        return false;
    }
    let payload = value
        .get("error")
        .filter(|error| error.is_object())
        .unwrap_or(value);
    let code = ["error_code", "providerCode", "code"]
        .iter()
        .find_map(|key| {
            payload[*key]
                .as_str()
                .map(str::to_string)
                .or_else(|| payload[*key].as_i64().map(|n| n.to_string()))
        })
        .filter(|c| !matches!(c.trim(), "" | "0" | "0000"))
        .unwrap_or_default();
    let msg = payload["message"]
        .as_str()
        .or_else(|| payload["error_msg"].as_str())
        .or_else(|| payload["msg"].as_str())
        .or_else(|| value.as_str())
        .unwrap_or("");
    let code_lower = code.trim().to_ascii_lowercase();
    let msg_lower = msg.to_ascii_lowercase();

    let tail_status = code
        .trim()
        .rsplit('.')
        .next()
        .and_then(|tail| tail.parse::<u16>().ok())
        .is_some_and(|tail| super::is_safe_http_rejection(tail) || tail == 406);

    tail_status
        || code_lower.contains("429")
        || code_lower.contains("modelarts.81101")
        || code_lower.contains("modelarts.81111")
        || code_lower.contains("modelarts.81114")
        || code_lower == "busyexception"
        || code_lower == "quota_limited"
        || code_lower == "insufficient_quota"
        || msg_lower.contains("too many")
        || msg_lower.contains("rate limit")
        || msg_lower.contains("insufficient quota")
        || msg.contains("额度")
        || code_lower.starts_with("tm.00001041")
        || code_lower == "codearts_concurrency_limited"
        || code_lower.starts_with("apig.0301")
        || code_lower.starts_with("apig.0302")
        || code_lower.starts_with("apig.0303")
        || code_lower.starts_with("apig.0307")
        || code_lower.starts_with("apig.0602")
        || code_lower.starts_with("sts5.1804")
        || code_lower.starts_with("sts5.1806")
        || code_lower == "invalid_grant"
        || msg_lower.contains("token expired")
        || msg_lower.contains("invalid token")
        || msg_lower.contains("security token has expired")
        || msg_lower.contains("verify ak sk signature fail")
        || code_lower.contains("001001020")
        || code_lower.starts_with("inferhub.001001005")
        || code_lower.starts_with("inferhub.001001011")
        || code_lower.starts_with("inferhub.001001017")
        || code_lower.starts_with("tm.00000001")
        || code_lower == "model_not_found"
        || code_lower == "4000"
        || msg_lower.contains("is not multimodal")
        || msg_lower.contains("request param is invalid")
        || msg_lower.contains("content policy")
        || msg_lower.contains("content blocked")
        || msg_lower.contains("content filter")
        || msg.contains("网络繁忙")
}

fn workbuddy_business(value: &Value, depth: usize) -> Option<&Value> {
    if depth == 0 {
        return None;
    }
    if scalar_i64(&value["code"]).is_some_and(|code| code >= 1000) {
        return Some(value);
    }
    [
        value.get("data"),
        value.get("error"),
        value.pointer("/response/data"),
    ]
    .into_iter()
    .flatten()
    .find_map(|child| workbuddy_business(child, depth - 1))
}

fn workbuddy_node_message(value: &Value) -> Option<&str> {
    ["message", "msg", "displayMsg"]
        .iter()
        .find_map(|key| value[*key].as_str().filter(|s| !s.trim().is_empty()))
        .or_else(|| value.pointer("/error/message").and_then(Value::as_str))
}

fn is_workbuddy_rejection(value: &Value) -> bool {
    if value["choices"]
        .as_array()
        .is_some_and(|choices| !choices.is_empty())
        || value["usage"].is_object()
    {
        return false;
    }
    let business = workbuddy_business(value, 6);
    let code = business
        .and_then(|node| scalar_i64(&node["code"]))
        .or_else(|| scalar_i64(&value["code"]));
    let has_error = value
        .get("error")
        .is_some_and(|error| !error.is_null() && error != false);
    if !code.is_some_and(|c| c != 0) && !has_error {
        return false;
    }
    let msg = business
        .and_then(workbuddy_node_message)
        .or_else(|| workbuddy_node_message(value))
        .or_else(|| value.as_str())
        .unwrap_or("");
    let msg_lower = msg.to_ascii_lowercase();

    matches!(
        code,
        Some(
            12153
                | 11128
                | 11140
                | 14015
                | 14016
                | 14017
                | 11115
                | 11101
                | 11133
                | 11158
                | 6000..=6008
                | 14001
                | 14003
                | 14012
                | 14013
                | 14014
                | 14018
                | 14019
                | 10105
        )
    ) || [
        "blocked by security policy",
        "unapproved channel",
        "illegal api invocation",
        "rate limit",
        "rate-limiting",
        "rate-limited",
        "too many requests",
        "quota exceeded",
        "quota exhausted",
        "usage limit",
    ]
    .iter()
    .any(|part| msg_lower.contains(part))
        || [
            "请求过于频繁",
            "频率限制",
            "额度已用尽",
            "额度耗尽",
            "限流",
        ]
        .iter()
        .any(|part| msg.contains(part))
}

fn doubaowork_decision_object(value: &Value) -> Option<Value> {
    let decision = value
        .pointer("/extra/decision")
        .or_else(|| value.get("decision"))?;
    match decision {
        Value::Object(_) => Some(decision.clone()),
        Value::String(raw) if raw.len() <= 64 * 1024 => serde_json::from_str::<Value>(raw)
            .ok()
            .filter(Value::is_object),
        _ => None,
    }
}

fn is_doubaowork_rejection(event: &str, value: &Value) -> bool {
    if matches!(
        event,
        "SSE_ACK"
            | "STREAM_MSG_NOTIFY"
            | "FULL_MSG_NOTIFY"
            | "CHUNK_DELTA"
            | "STREAM_CHUNK"
            | "ASYNC_CHUNK_SNAPSHOT"
            | "FETCH_STREAM"
            | "SSE_REPLY_END"
    ) {
        return false;
    }
    let detail_code = value
        .get("error_detail")
        .filter(|v| v.is_object())
        .or_else(|| value.get("error").filter(|v| v.is_object()))
        .unwrap_or(value);
    let code = scalar_i64(&detail_code["code"])
        .or_else(|| scalar_i64(&detail_code["error_code"]))
        .or_else(|| scalar_i64(&value["code"]))
        .or_else(|| scalar_i64(&value["error_code"]));
    let decision = doubaowork_decision_object(value);
    let detail_msg = value
        .get("error_detail")
        .filter(|v| v.is_object())
        .or_else(|| value.get("error_details").filter(|v| v.is_object()))
        .or_else(|| value.get("error").filter(|v| v.is_object()))
        .unwrap_or(value);
    let msg = ["message", "msg", "error_msg", "error_message"]
        .iter()
        .find_map(|key| detail_msg[*key].as_str().filter(|s| !s.trim().is_empty()))
        .unwrap_or("");
    let msg_lower = msg.to_ascii_lowercase();

    decision
        .as_ref()
        .is_some_and(|item| item["type"].as_str() == Some("verify"))
        || matches!(
            code,
            Some(710012000 | 710012001 | 710010202 | 710020202 | 710012004 | 710022004)
        )
        || [
            "not logged in",
            "login required",
            "session expired",
            "invalid param",
            "insufficient credit",
            "insufficient quota",
            "quota exceeded",
            "content policy",
            "content blocked",
            "content filter",
        ]
        .iter()
        .any(|part| msg_lower.contains(part))
        || msg.contains("积分不足")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::server::core::upstream::usage::RequestTelemetry;

    fn encode_chunk(bytes: &[u8]) -> Result<Value, proto::WireError> {
        Ok(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            "done": false,
        }))
    }

    fn done_chunk() -> Result<Value, proto::WireError> {
        Ok(json!({ "done": true }))
    }

    fn open_frozen(
        provider: &str,
        result: &Value,
    ) -> (
        std::sync::Arc<crate::server::core::upstream::replay_guard::ReplayGuard>,
        crate::server::core::upstream::replay_guard::CallLease,
        HttpRejectionObserver,
    ) {
        let telemetry = RequestTelemetry::new();
        let guard = telemetry.replay_guard().clone();
        let lease = guard.open_call().unwrap();
        let prep = lease.preparation();
        prep.freeze().unwrap();
        let observer = HttpRejectionObserver::for_response(provider, "wasm-1", result, prep).unwrap();
        (guard, lease, observer)
    }

    #[test]
    fn codearts_sse_429_unlocks_replay_across_chunks_and_done() {
        let head = json!({ "status": 200, "headers": { "Content-Type": "text/event-stream; charset=utf-8" } });
        let (guard, lease, mut observer) = open_frozen("codearts", &head);
        let payload = b"data: {\"error_code\":\"InferHub.ModelArts.81114.429\",\"error_msg\":\"Too many requests, the rate limit is 300000000 tokens per minute.\"}\n\n";
        let split = 45;
        observer.observe(&encode_chunk(&payload[..split]));
        assert!(guard.blocked(), "半帧未完结前不能提前放行");
        observer.observe(&encode_chunk(&payload[split..]));
        observer.observe(&encode_chunk(b"data: [DONE]\n\n"));
        observer.observe(&done_chunk());
        observer.finish_on_close();
        lease.close();
        assert!(!guard.blocked(), "CodeArts 200+429 SSE 拒单后应允许换账号");
        assert!(guard.open_call().is_ok());
    }

    #[test]
    fn codearts_sse_answer_or_unknown_500_stays_locked() {
        let head = json!({ "status": 200, "headers": { "content-type": "text/event-stream" } });
        {
            let (guard, lease, mut observer) = open_frozen("codearts", &head);
            observer.observe(&encode_chunk(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n"));
            observer.observe(&encode_chunk(b"data: {\"error_code\":\"InferHub.ModelArts.81114.429\",\"error_msg\":\"Too many requests\"}\n\n"));
            observer.finish_on_close();
            lease.close();
            assert!(guard.blocked(), "先出现回答内容后即使再遇 429 也必须永久锁死");
        }
        {
            let (guard, lease, mut observer) = open_frozen("codearts", &head);
            observer.observe(&encode_chunk(b"data: {\"error_code\":\"InferHub.5000\",\"error_msg\":\"backend exploded\"}\n\n"));
            observer.finish_on_close();
            lease.close();
            assert!(guard.blocked(), "未知 5xx 内部错误不能解锁重放");
        }
        {
            let (guard, lease, mut observer) = open_frozen("codearts", &head);
            observer.observe(&encode_chunk(b"data: {\"error_code\":\"InferHub.ModelArts.81114.429\""));
            observer.finish_on_close();
            lease.close();
            assert!(guard.blocked(), "未读到完整事件就关闭连接必须永久锁死");
        }
    }

    #[test]
    fn workbuddy_json_and_sse_rejections_unlock_while_normal_answer_locks() {
        let json_head = json!({ "status": 200, "headers": { "content-type": "application/json" } });
        for body in [
            br#"{"code":6004,"message":"rate limit"}"#.as_slice(),
            br#"{"data":{"code":14001,"msg":"quota exhausted"}}"#.as_slice(),
        ] {
            let (guard, lease, mut observer) = open_frozen("workbuddy", &json_head);
            observer.observe(&encode_chunk(body));
            observer.observe(&done_chunk());
            observer.finish_on_close();
            lease.close();
            assert!(!guard.blocked(), "WorkBuddy 200 JSON 明确拒单应放行");
        }

        let sse_head = json!({ "status": 200, "headers": { "content-type": "text/event-stream" } });
        let (guard, lease, mut observer) = open_frozen("workbuddy", &sse_head);
        observer.observe(&encode_chunk(b"data: {\"code\":6004,\"message\":\"rate limit\"}\n\n"));
        observer.finish_on_close();
        lease.close();
        assert!(!guard.blocked(), "WorkBuddy 200 SSE 首帧 6004 拒单应放行");

        let (guard, lease, mut observer) = open_frozen("workbuddy", &sse_head);
        observer.observe(&encode_chunk(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n"));
        observer.finish_on_close();
        lease.close();
        assert!(guard.blocked(), "WorkBuddy 正常回答必须永久锁死");
    }

    #[test]
    fn doubaowork_json_and_sse_ack_semantics() {
        let json_head = json!({ "status": 200, "headers": { "content-type": "application/json" } });
        for body in [
            br#"{"extra":{"decision":{"type":"verify","subtype":"captcha"}}}"#.as_slice(),
            br#"{"code":710012004,"message":"too fast"}"#.as_slice(),
        ] {
            let (guard, lease, mut observer) = open_frozen("doubaowork", &json_head);
            observer.observe(&encode_chunk(body));
            observer.observe(&done_chunk());
            lease.close();
            assert!(!guard.blocked(), "豆包工作 200 JSON 明确拒单应放行");
        }

        let sse_head = json!({ "status": 200, "headers": { "content-type": "text/event-stream" } });
        {
            let (guard, lease, mut observer) = open_frozen("doubaowork", &sse_head);
            observer.observe(&encode_chunk(b"event: error\ndata: {\"code\":710012004,\"msg\":\"rate limited\"}\n\n"));
            observer.finish_on_close();
            lease.close();
            assert!(!guard.blocked(), "豆包工作 SSE_ACK 前明确拒单应放行");
        }
        {
            let (guard, lease, mut observer) = open_frozen("doubaowork", &sse_head);
            observer.observe(&encode_chunk(b"event: SSE_ACK\ndata: {\"ack_client_meta\":{\"conversation_id\":\"c1\"}}\n\n"));
            observer.observe(&encode_chunk(b"event: error\ndata: {\"code\":710012004}\n\n"));
            observer.finish_on_close();
            lease.close();
            assert!(guard.blocked(), "豆包工作收到 SSE_ACK 后已受理，后续再错也必须永久锁死");
        }
    }

    #[test]
    fn unsupported_providers_are_not_inspected() {
        let head = json!({ "status": 200, "headers": { "content-type": "application/json" } });
        assert!(!supports_http_200_rejection_inspection("tabbit", &head));
        assert!(!supports_http_200_rejection_inspection("tabbit-intl", &head));
    }
}
