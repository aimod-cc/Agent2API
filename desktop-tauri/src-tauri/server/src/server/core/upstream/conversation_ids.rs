//! 稳定上游会话由宿主按客户端凭证作用域隔离；落盘只存哈希与追踪 ID。

use axum::http::HeaderMap;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::errors::GatewayError;
use crate::server::{core::task_state, logging};

const STATE_KEY: &str = "upstreamConversationIds";
const CONVERSATION_TTL: i64 = 7 * 24 * 60 * 60_000;
const TURN_TTL: i64 = 24 * 60 * 60_000;
const MAX_CONVERSATIONS: usize = 512;
const MAX_TURNS: usize = 2048;

pub(super) struct ConversationIds {
    conversation: String,
    turn: String,
    root: String,
}

impl ConversationIds {
    pub fn apply(&self, headers: &mut Vec<(String, String)>) {
        for (name, value) in [
            ("X-Conversation-ID", &self.conversation),
            ("X-Conversation-Request-ID", &self.turn),
            ("X-Root-Request-ID", &self.root),
        ] {
            headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
            headers.push((name.to_string(), value.clone()));
        }
    }
}

pub(super) fn resolve(headers: &HeaderMap, body: &Value) -> Result<ConversationIds, GatewayError> {
    let mut scope = Sha256::new();
    for name in ["authorization", "x-api-key", "x-client-id", "user-agent"] {
        scope.update(name.as_bytes());
        if let Some(value) = headers.get(name) {
            scope.update(value.as_bytes());
        }
        scope.update([0]);
    }
    for pointer in ["/user"] {
        scope.update(pointer.as_bytes());
        if let Some(value) = body.pointer(pointer) {
            scope.update(value.to_string().as_bytes());
        }
        scope.update([0]);
    }
    let scope = format!("{:x}", scope.finalize());
    let explicit = text(headers, "x-conversation-id");
    let routing_key = super::route_state::RouteState::conversation_key(headers, body);
    let mut conversation = Sha256::new();
    conversation.update(scope.as_bytes());
    if let Some(id) = &explicit {
        conversation.update(id.as_bytes());
    } else if routing_key.starts_with("id:") {
        conversation.update(routing_key.as_bytes());
    } else if let Some(first_user) =
        body.get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            })
    {
        conversation.update(first_user.to_string().as_bytes());
    } else {
        conversation.update(super::request::new_message_id().as_bytes());
    }
    let conversation_key = format!("{:x}", conversation.finalize());
    let mut turn = Sha256::new();
    turn.update(conversation_key.as_bytes());
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        let end = messages
            .iter()
            .rposition(|message| message.get("role").and_then(Value::as_str) == Some("user"));
        if let Some(end) = end {
            for message in &messages[..=end] {
                turn.update(message.to_string().as_bytes());
                turn.update([0]);
            }
        } else {
            turn.update(body.to_string().as_bytes());
        }
    } else {
        turn.update(body.to_string().as_bytes());
    }
    let turn_key = format!("{:x}", turn.finalize());
    let explicit_root = text(headers, "x-root-request-id");
    let explicit_turn =
        text(headers, "x-conversation-request-id").or_else(|| explicit_root.clone());
    let now = logging::now_ms();
    let state = task_state::update_value(STATE_KEY, |state| {
        if !state.is_object() {
            *state = json!({ "conversations": [], "turns": [] });
        }
        touch(
            state,
            "conversations",
            &conversation_key,
            explicit.as_deref(),
            now,
            CONVERSATION_TTL,
            MAX_CONVERSATIONS,
            super::request::new_conversation_id,
        );
        touch(
            state,
            "turns",
            &turn_key,
            explicit_turn.as_deref(),
            now,
            TURN_TTL,
            MAX_TURNS,
            super::request::new_message_id,
        );
    })
    .map_err(|_| GatewayError::with_status(503, "无法保存上游会话标识，未发送请求"))?;
    let id = |list: &str, key: &str| {
        state
            .get(list)
            .and_then(Value::as_array)
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row.get("key").and_then(Value::as_str) == Some(key))
            })
            .and_then(|row| row.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let turn = explicit_turn
        .or_else(|| id("turns", &turn_key))
        .ok_or_else(|| GatewayError::new("上游轮次标识保存失败"))?;
    Ok(ConversationIds {
        conversation: explicit
            .or_else(|| id("conversations", &conversation_key))
            .ok_or_else(|| GatewayError::new("上游会话标识保存失败"))?,
        root: explicit_root.unwrap_or_else(|| turn.clone()),
        turn,
    })
}

fn touch(
    state: &mut Value,
    field: &str,
    key: &str,
    explicit: Option<&str>,
    now: i64,
    ttl: i64,
    cap: usize,
    generate: fn() -> String,
) {
    if !state.get(field).is_some_and(Value::is_array) {
        state[field] = json!([]);
    }
    let Some(rows) = state.get_mut(field).and_then(Value::as_array_mut) else {
        return;
    };
    rows.retain(|row| {
        row.get("at")
            .and_then(Value::as_i64)
            .is_some_and(|at| at <= now && now.saturating_sub(at) < ttl)
    });
    if let Some(row) = rows
        .iter_mut()
        .find(|row| row.get("key").and_then(Value::as_str) == Some(key))
    {
        row["at"] = json!(now);
        if let Some(id) = explicit {
            row["id"] = json!(id);
        }
    } else {
        while rows.len() >= cap {
            rows.remove(0);
        }
        rows.push(json!({ "key": key, "id": explicit.map(str::to_string).unwrap_or_else(generate), "at": now }));
    }
}

fn text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)?
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value.bytes().all(|byte| byte.is_ascii_graphic())
        })
        .map(str::to_string)
}
