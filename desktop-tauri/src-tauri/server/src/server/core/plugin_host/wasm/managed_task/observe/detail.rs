use std::fmt;

use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::{
    object, present, validate, AttachStatus, BoundedText, DetailFailure, DetailObservation,
    DetailState, DetailUnknown, Invalid, MAX_CONDITIONS, MAX_CONDITION_MESSAGE_BYTES,
    MAX_DETAIL_BYTES, MAX_NAME_BYTES,
};
use super::super::{strict_json::Object, types::NonZeroUuid};

#[derive(Deserialize)]
struct Envelope {
    code: i64,
    data: Object<Task>,
}

#[derive(Deserialize)]
struct Task {
    status: BoundedText<MAX_NAME_BYTES>,
    #[serde(default, deserialize_with = "present")]
    id: Option<BoundedText<36>>,
    // 官方尚未分配执行环境时可能为 null；不保留 host/URL 等任何连接材料。
    #[serde(default)]
    virtualmachine: Option<Object<Machine>>,
}

#[derive(Deserialize)]
struct Machine {
    // Go nil slice 对应 null；已出现的数组元素必须是正确形状。
    #[serde(default)]
    conditions: Option<Conditions>,
}

#[derive(Deserialize)]
struct Condition {
    #[serde(rename = "type")]
    kind: BoundedText<MAX_NAME_BYTES>,
    #[serde(rename = "message", default, deserialize_with = "present")]
    _message: Option<BoundedText<MAX_CONDITION_MESSAGE_BYTES>>,
}

struct Conditions {
    last: Option<Condition>,
}

impl<'de> Deserialize<'de> for Conditions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ConditionsVisitor;
        impl<'de> Visitor<'de> for ConditionsVisitor {
            type Value = Conditions;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("bounded condition objects")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
                let mut count = 0;
                let mut last = None;
                while let Some(condition) = sequence.next_element::<Object<Condition>>()? {
                    if count >= MAX_CONDITIONS {
                        return Err(serde::de::Error::custom("condition limit exceeded"));
                    }
                    if condition.0.kind.0.is_empty() {
                        return Err(serde::de::Error::custom("empty condition type"));
                    }
                    count += 1;
                    last = Some(condition.0);
                }
                Ok(Conditions { last })
            }
        }
        deserializer.deserialize_seq(ConditionsVisitor)
    }
}

pub(super) fn parse(http_status: u16, bytes: &[u8]) -> Result<DetailObservation, Invalid> {
    validate(bytes, MAX_DETAIL_BYTES)?;
    if !(100..600).contains(&http_status) { return Err(Invalid::HttpStatus); }
    let envelope: Envelope = object(bytes)?;
    let task = envelope.data.0;
    if task.status.0.is_empty() { return Err(Invalid::Fields); }
    let identity = task.id.map(|id| NonZeroUuid::parse(&id.0).ok_or(Invalid::Identity)).transpose()?;
    let failed_environment = task.virtualmachine.and_then(|machine| machine.0.conditions)
        .and_then(|conditions| conditions.last)
        .is_some_and(|condition| condition.kind.0.eq_ignore_ascii_case("failed"));
    let state = if !(200..300).contains(&http_status) {
        DetailState::Unknown(DetailUnknown::HttpRejected)
    } else if envelope.code != 0 {
        DetailState::Unknown(DetailUnknown::BusinessRejected)
    } else if task.status.0 == "error" {
        DetailState::Failed(DetailFailure::TaskError)
    } else if failed_environment {
        DetailState::Failed(DetailFailure::EnvironmentFailed)
    } else {
        match task.status.0.as_str() {
            "pending" => DetailState::Waiting,
            "processing" => DetailState::NeedsAttach(AttachStatus::Processing),
            "finished" => DetailState::NeedsAttach(AttachStatus::Finished),
            _ => DetailState::Unknown(DetailUnknown::UnrecognizedStatus),
        }
    };
    let identity = if (200..300).contains(&http_status) && envelope.code == 0 { identity } else { None };
    Ok(DetailObservation { state, identity })
}
