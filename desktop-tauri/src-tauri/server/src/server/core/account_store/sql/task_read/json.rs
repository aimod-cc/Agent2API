//! 原始账号JSON的有界解析；任意层重复键（含转义后相同键）、超深度/节点数都拒绝。
//! 不依赖任务宿主模块，不日志；未知字段完整保留，最终secret另有256KiB投影上限。

use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::{TaskReadError, MAX_DATA_BYTES};

const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 32_768;

pub(super) fn parse(bytes: &[u8]) -> Result<Value, TaskReadError> {
    if bytes.len() > MAX_DATA_BYTES { return Err(TaskReadError::TooLarge); }
    let mut remaining = MAX_NODES;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Bounded { containers: 0, remaining: &mut remaining }
        .deserialize(&mut deserializer).map_err(|_| TaskReadError::Invalid)?;
    deserializer.end().map_err(|_| TaskReadError::Invalid)?;
    Ok(value)
}

struct Bounded<'a> {
    containers: usize,
    remaining: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for Bounded<'_> {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if *self.remaining == 0 {
            return Err(de::Error::custom("account JSON node limit"));
        }
        *self.remaining -= 1;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Bounded<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded account JSON with unique keys")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> { Ok(Value::Bool(value)) }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value).map(Value::Number).ok_or_else(|| de::Error::custom("invalid account number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> { Ok(Value::String(value.to_string())) }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> { Ok(Value::String(value)) }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> { Ok(Value::Null) }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        if self.containers >= MAX_DEPTH { return Err(de::Error::custom("account JSON depth limit")); }
        let mut items = Vec::new();
        while let Some(value) = sequence.next_element_seed(Bounded {
            containers: self.containers + 1,
            remaining: &mut *self.remaining,
        })? {
            items.push(value);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        if self.containers >= MAX_DEPTH { return Err(de::Error::custom("account JSON depth limit")); }
        let mut fields = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if fields.contains_key(&key) {
                return Err(de::Error::custom("duplicate account JSON key"));
            }
            let value = object.next_value_seed(Bounded {
                containers: self.containers + 1,
                remaining: &mut *self.remaining,
            })?;
            fields.insert(key, value);
        }
        Ok(Value::Object(fields))
    }
}
