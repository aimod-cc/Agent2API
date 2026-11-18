//! MonkeyCode 固定详情与单条任务流消息的纯观察值，不构成收据、归属或网络许可。
//! 依据官方 ad169c9b946ea04a1fc19a7abff9815d38116a73；只解释文本首轮的有限字段。

mod detail;
mod message;
mod tool;
pub(super) use tool::ToolObservation;

use std::fmt;

use serde::de::{DeserializeOwned, Visitor};
use serde::{Deserialize, Deserializer};

use super::{strict_json, types::NonZeroUuid};

pub(super) const MAX_DETAIL_BYTES: usize = 1024 * 1024;
pub(super) const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_DECODED_BYTES: usize = 512 * 1024;
pub(super) const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_NAME_BYTES: usize = 128;
const MAX_CONDITION_MESSAGE_BYTES: usize = 4096;
const MAX_EVENT_ID_BYTES: usize = 512;
const MAX_CURSOR_BYTES: usize = 4096;
const MAX_CONDITIONS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Invalid {
    LimitExceeded,
    Json,
    Fields,
    Base64,
    HttpStatus,
    Identity,
}

impl Invalid {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::LimitExceeded => "任务观察消息超过本层大小限制",
            Self::Json => "任务观察消息不是完整JSON、存在重复字段或超过深度限制",
            Self::Fields => "任务观察消息的已解释字段缺失、类型或长度无效",
            Self::Base64 => "任务流数据不是规范的base64编码",
            Self::HttpStatus => "任务详情缺少有效HTTP状态",
            Self::Identity => "任务详情中的编号不是合法非零UUID",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttachStatus {
    Processing,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DetailFailure {
    TaskError,
    EnvironmentFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DetailUnknown {
    HttpRejected,
    BusinessRejected,
    UnrecognizedStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DetailState {
    Waiting,
    /// 两种状态都仍须 attach 读取真实 task-ended，不能据此合成答复成功。
    NeedsAttach(AttachStatus),
    Failed(DetailFailure),
    Unknown(DetailUnknown),
}

/// 只提供编号比对，不暴露编号原文；相等也不是任务归属证明。
pub(super) struct DetailObservation {
    state: DetailState,
    identity: Option<NonZeroUuid>,
}

impl DetailObservation {
    pub(super) fn state(&self) -> DetailState {
        self.state
    }

    pub(super) fn matches_id(&self, expected: &NonZeroUuid) -> Option<bool> {
        self.identity
            .as_ref()
            .map(|identity| identity.as_str() == expected.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MessageMeta {
    pub(super) timestamp: i64,
    pub(super) seq: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextKind {
    Message,
    Thought,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Interaction {
    Permission,
    Question,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamFailure {
    TaskError,
    Error,
    ToolFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MessageUnknown {
    Event,
    RunningKind,
    AcpShape,
    AcpEvent,
    NonTextContent,
    QuestionShape,
}

/// 不携带原始包、任务编号、URL、工具执行权限或推测的用量。
pub(super) enum MessageEvent {
    Text {
        kind: TextKind,
        text: String,
    },
    /// 仅远端Agent工具活动的只读投影，不授权客户端执行、答复或新一轮。
    Tool(ToolObservation),
    Started,
    Ping,
    Cursor {
        has_more: bool,
    },
    /// 仅结束一轮，不确认任务停止、执行环境回收或其他消息的成功。
    RoundEnded,
    Failed(StreamFailure),
    /// 必须交给未来的独立交互门禁，不能把观察值当作批准或自动答复。
    NeedsInteraction(Interaction),
    PermissionRequest(String),
    QuestionRequest(String),
    /// 未实现的事件或形态不能当成功终态，也不授权透传上行。
    Unknown(MessageUnknown),
}

/// 单条消息的观察结果，不去重、不选最新轮、不把 seq/timestamp 当权限。
pub(super) struct MessageObservation {
    pub(super) meta: MessageMeta,
    pub(super) event: MessageEvent,
}

/// bytes 必须来自一次完整详情响应；部分响应不得拼出一个成功观察值。
pub(super) fn parse_detail(http_status: u16, bytes: &[u8]) -> Result<DetailObservation, Invalid> {
    detail::parse(http_status, bytes)
}

/// 只接单个完整 WS 文本消息；帧重组、累计预算和连接归属不在本层。
pub(super) fn parse_message(bytes: &[u8]) -> Result<MessageObservation, Invalid> {
    message::parse(bytes)
}

fn validate(bytes: &[u8], limit: usize) -> Result<(), Invalid> {
    strict_json::validate(bytes, limit).map_err(|error| match error {
        strict_json::JsonError::TooLarge => Invalid::LimitExceeded,
        strict_json::JsonError::Invalid => Invalid::Json,
    })
}

/// 调用者已完成原始字节校验；Object 禁止数组按字段顺序冒充对象。
fn object<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Invalid> {
    serde_json::from_slice::<strict_json::Object<T>>(bytes)
        .map(|value| value.0)
        .map_err(|_| Invalid::Fields)
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

struct BoundedText<const LIMIT: usize>(String);

impl<'de, const LIMIT: usize> Deserialize<'de> for BoundedText<LIMIT> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TextVisitor<const LIMIT: usize>;
        impl<'de, const LIMIT: usize> Visitor<'de> for TextVisitor<LIMIT> {
            type Value = BoundedText<LIMIT>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("bounded text")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.len() > LIMIT {
                    return Err(E::custom("text limit exceeded"));
                }
                Ok(BoundedText(value.to_string()))
            }
        }
        deserializer.deserialize_str(TextVisitor::<LIMIT>)
    }
}
