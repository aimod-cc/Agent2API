use base64::Engine;
use serde::Deserialize;

use super::super::strict_json::Object;
use super::{
    object, present, tool, validate, BoundedText, Interaction, Invalid, MessageEvent, MessageMeta,
    MessageObservation, MessageUnknown, StreamFailure, TextKind, MAX_CURSOR_BYTES,
    MAX_DECODED_BYTES, MAX_EVENT_ID_BYTES, MAX_MESSAGE_BYTES, MAX_NAME_BYTES, MAX_TEXT_BYTES,
};

const MAX_ENCODED_BYTES: usize = MAX_DECODED_BYTES.div_ceil(3) * 4;

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: BoundedText<MAX_NAME_BYTES>,
    #[serde(rename = "kind", default)]
    kind_hint: Option<BoundedText<MAX_NAME_BYTES>>,
    // Go []byte 可为 null；缺字段不等于显式的空载荷。
    #[serde(default, deserialize_with = "nullable_data")]
    data: Option<BoundedText<MAX_ENCODED_BYTES>>,
    timestamp: i64,
    #[serde(default, deserialize_with = "present")]
    seq: Option<u64>,
}

#[derive(Deserialize)]
struct Acp {
    #[serde(default, deserialize_with = "present")]
    update: Option<Object<Update>>,
}

#[derive(Deserialize)]
struct Update {
    #[serde(rename = "sessionUpdate")]
    kind: BoundedText<MAX_NAME_BYTES>,
}

#[derive(Deserialize)]
struct TextAcp {
    update: Object<TextUpdate>,
}

#[derive(Deserialize)]
struct TextUpdate {
    content: Object<Content>,
}

#[derive(Deserialize)]
struct Content {
    #[serde(rename = "type")]
    kind: BoundedText<MAX_NAME_BYTES>,
    #[serde(default, deserialize_with = "present")]
    text: Option<BoundedText<MAX_TEXT_BYTES>>,
}

#[derive(Deserialize)]
struct Permission {
    id: BoundedText<MAX_EVENT_ID_BYTES>,
}

#[derive(Deserialize)]
struct Question {
    #[serde(default, deserialize_with = "present")]
    request_id: Option<BoundedText<MAX_EVENT_ID_BYTES>>,
    #[serde(default, deserialize_with = "present")]
    id: Option<BoundedText<MAX_EVENT_ID_BYTES>>,
    #[serde(rename = "toolCall", default, deserialize_with = "present")]
    tool_call: Option<Object<QuestionCall>>,
}

#[derive(Deserialize)]
struct QuestionCall {
    #[serde(rename = "toolCallId")]
    id: BoundedText<MAX_EVENT_ID_BYTES>,
}

#[derive(Deserialize)]
struct Cursor {
    cursor: BoundedText<MAX_CURSOR_BYTES>,
    has_more: bool,
}

#[derive(Deserialize)]
struct StatusPayload {}

pub(super) fn parse(bytes: &[u8]) -> Result<MessageObservation, Invalid> {
    validate(bytes, MAX_MESSAGE_BYTES)?;
    let envelope: Envelope = object(bytes)?;
    if envelope.kind.0.is_empty() {
        return Err(Invalid::Fields);
    }
    let data = envelope.data.map(|data| decode(&data.0)).transpose()?;
    let event = match envelope.kind.0.as_str() {
        "ping" | "task-started" | "task-ended" => {
            status_payload(data.as_deref())?;
            match envelope.kind.0.as_str() {
                "ping" => MessageEvent::Ping,
                "task-started" => MessageEvent::Started,
                _ => MessageEvent::RoundEnded,
            }
        }
        // 失败类型只投影固定分类，不解释或返回错误载荷，更不合成成功。
        "task-error" => MessageEvent::Failed(StreamFailure::TaskError),
        "error" => MessageEvent::Failed(StreamFailure::Error),
        "cursor" => {
            let cursor: Cursor = object(json_payload(data.as_deref())?)?;
            if cursor.cursor.0.is_empty() {
                return Err(Invalid::Fields);
            }
            MessageEvent::Cursor {
                has_more: cursor.has_more,
            }
        }
        "permission-req" => {
            let permission: Permission = object(json_payload(data.as_deref())?)?;
            if !event_id(&permission.id.0) {
                return Err(Invalid::Fields);
            }
            MessageEvent::PermissionRequest(permission.id.0)
        }
        "task-running" => {
            let kind = envelope.kind_hint.ok_or(Invalid::Fields)?;
            if kind.0.is_empty() {
                return Err(Invalid::Fields);
            }
            match kind.0.as_str() {
                "acp_event" => acp(json_payload(data.as_deref())?)?,
                "acp_ask_user_question" => question(json_payload(data.as_deref())?)?,
                _ => MessageEvent::Unknown(MessageUnknown::RunningKind),
            }
        }
        _ => MessageEvent::Unknown(MessageUnknown::Event),
    };
    Ok(MessageObservation {
        meta: MessageMeta {
            timestamp: envelope.timestamp,
            seq: envelope.seq,
        },
        event,
    })
}

fn nullable_data<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<BoundedText<MAX_ENCODED_BYTES>>, D::Error> {
    Option::<BoundedText<MAX_ENCODED_BYTES>>::deserialize(deserializer)
}

fn json_payload(data: Option<&[u8]>) -> Result<&[u8], Invalid> {
    let bytes = data
        .filter(|bytes| !bytes.is_empty())
        .ok_or(Invalid::Fields)?;
    validate(bytes, MAX_DECODED_BYTES)?;
    Ok(bytes)
}

fn status_payload(data: Option<&[u8]>) -> Result<(), Invalid> {
    let Some(bytes) = data.filter(|bytes| !bytes.is_empty()) else {
        return Ok(());
    };
    // 正向状态只认空载荷或结构化对象；不能把任意字节当作正常轮次结束。
    validate(bytes, MAX_DECODED_BYTES)?;
    object::<StatusPayload>(bytes)?;
    Ok(())
}

fn event_id(id: &str) -> bool {
    !id.is_empty() && !id.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}

fn acp(bytes: &[u8]) -> Result<MessageEvent, Invalid> {
    let acp: Acp = object(bytes)?;
    let Some(update) = acp.update else {
        return Ok(if tool::unsupported_failure(bytes)? {
            MessageEvent::Failed(StreamFailure::ToolFailed)
        } else {
            MessageEvent::Unknown(MessageUnknown::AcpShape)
        });
    };
    let update = update.0;
    let kind = match update.kind.0.as_str() {
        "agent_message_chunk" => TextKind::Message,
        "agent_thought_chunk" => TextKind::Thought,
        "tool_call" | "tool_call_update" => {
            return tool::nested(bytes, &update.kind.0).map(|parsed| match parsed {
                tool::Parsed::Tool(tool) => MessageEvent::Tool(tool),
                tool::Parsed::Unknown => MessageEvent::Unknown(MessageUnknown::AcpShape),
                tool::Parsed::Failed => MessageEvent::Failed(StreamFailure::ToolFailed),
            });
        }
        "" => return Err(Invalid::Fields),
        _ => return Ok(MessageEvent::Unknown(MessageUnknown::AcpEvent)),
    };
    let text_acp: TextAcp = object(bytes)?;
    let content = text_acp.update.0.content.0;
    if content.kind.0.is_empty() {
        return Err(Invalid::Fields);
    }
    if content.kind.0 != "text" {
        return Ok(MessageEvent::Unknown(MessageUnknown::NonTextContent));
    }
    let text = content.text.ok_or(Invalid::Fields)?.0;
    Ok(MessageEvent::Text { kind, text })
}

fn question(bytes: &[u8]) -> Result<MessageEvent, Invalid> {
    let question: Question = object(bytes)?;
    if let Some(request) = question.request_id.or(question.id) {
        if !event_id(&request.0) { return Err(Invalid::Fields); }
        return Ok(MessageEvent::QuestionRequest(request.0));
    }
    let Some(tool_call) = question.tool_call else {
        return Ok(MessageEvent::Unknown(MessageUnknown::QuestionShape));
    };
    if !event_id(&tool_call.0.id.0) {
        return Err(Invalid::Fields);
    }
    // 只报告需要交互，不返回编号、不生成答案或授权。
    Ok(MessageEvent::NeedsInteraction(Interaction::Question))
}

fn decode(encoded: &str) -> Result<Vec<u8>, Invalid> {
    if encoded.len() > MAX_ENCODED_BYTES {
        return Err(Invalid::LimitExceeded);
    }
    // 先拒绝可确定越界的载荷，再分配解码缓冲；padding 错误由标准引擎拒绝。
    if encoded.len() % 4 != 0 {
        return Err(Invalid::Base64);
    }
    let padding = encoded
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'=')
        .count();
    if padding > 2 {
        return Err(Invalid::Base64);
    }
    let length = (encoded.len() / 4)
        .checked_mul(3)
        .and_then(|length| length.checked_sub(padding))
        .ok_or(Invalid::Base64)?;
    if length > MAX_DECODED_BYTES {
        return Err(Invalid::LimitExceeded);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| Invalid::Base64)?;
    if bytes.len() > MAX_DECODED_BYTES {
        return Err(Invalid::LimitExceeded);
    }
    // 固定云端 data 是任意 []byte；是否必须为 JSON 由已识别的事件决定。
    Ok(bytes)
}
