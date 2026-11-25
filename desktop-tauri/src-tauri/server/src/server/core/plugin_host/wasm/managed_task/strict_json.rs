//! 在 serde_json::Value 丢失重复字段之前检查原始字节。仅检查结构，不执行表达式。
//! 每层对象拒绝所有重复键（包括转义后相同的键），比只查 code/data/id 更保守。
//! 第一遍访客不保存正文值；最大输入字节与深度均先受约束，再按需要二次解析。

use std::{collections::BTreeSet, fmt, marker::PhantomData};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

use super::types::MAX_JSON_DEPTH;

#[derive(Clone, Copy)]
pub(super) enum JsonError {
    TooLarge,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StrictJsonError {
    TooLarge,
    DepthExceeded,
    DuplicateKey,
    Invalid,
}

struct Shape<'a> {
    // 标量不增加深度；根对象/数组算第一层，最多 32 层容器。
    containers: usize,
    cause: &'a mut Option<StrictJsonError>,
}

impl<'de> DeserializeSeed<'de> for Shape<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Shape<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON with unique object keys")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_string<E: de::Error>(self, _: String) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        if self.containers >= MAX_JSON_DEPTH {
            *self.cause = Some(StrictJsonError::DepthExceeded);
            return Err(de::Error::custom("JSON depth exceeded"));
        }
        while sequence
            .next_element_seed(Shape {
                containers: self.containers + 1,
                cause: &mut *self.cause,
            })?
            .is_some()
        {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<(), A::Error> {
        if self.containers >= MAX_JSON_DEPTH {
            *self.cause = Some(StrictJsonError::DepthExceeded);
            return Err(de::Error::custom("JSON depth exceeded"));
        }
        let mut keys = BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key) {
                *self.cause = Some(StrictJsonError::DuplicateKey);
                return Err(de::Error::custom("duplicate JSON field"));
            }
            object.next_value_seed(Shape {
                containers: self.containers + 1,
                cause: &mut *self.cause,
            })?;
        }
        Ok(())
    }
}

pub(super) fn validate_detailed(bytes: &[u8], limit: usize) -> Result<(), StrictJsonError> {
    if bytes.len() > limit {
        return Err(StrictJsonError::TooLarge);
    }
    if std::str::from_utf8(bytes).is_err() {
        return Err(StrictJsonError::Invalid);
    }
    let mut cause = None;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    if (Shape {
        containers: 0,
        cause: &mut cause,
    })
    .deserialize(&mut deserializer)
    .is_err()
    {
        return Err(cause.unwrap_or(StrictJsonError::Invalid));
    }
    deserializer.end().map_err(|_| StrictJsonError::Invalid)
}

pub(super) fn validate(bytes: &[u8], limit: usize) -> Result<(), JsonError> {
    validate_detailed(bytes, limit).map_err(|error| match error {
        StrictJsonError::TooLarge => JsonError::TooLarge,
        StrictJsonError::DepthExceeded
        | StrictJsonError::DuplicateKey
        | StrictJsonError::Invalid => JsonError::Invalid,
    })
}

/// 只接受 JSON 对象，防止 serde 结构体默认也从数组按字段顺序反序列化。
/// 本类型只是输入形状，不是任何信任/授权类型。
pub(super) struct Object<T>(pub(super) T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = Object<T>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, object: A) -> Result<Self::Value, A::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(object)).map(Object)
            }
        }
        deserializer.deserialize_map(ObjectVisitor::<T>(PhantomData))
    }
}

/// 已经过严格第一遍后仍只提取顶层 code，不保留其他大字段。
/// 不加 deny_unknown_fields：官方创建响应会回显任务/环境，本模块应忽略它们。
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    code: Value,
}

pub(super) fn code(bytes: &[u8]) -> Option<i64> {
    serde_json::from_slice::<Object<Envelope>>(bytes)
        .ok()?
        .0
        .code
        .as_i64()
}
