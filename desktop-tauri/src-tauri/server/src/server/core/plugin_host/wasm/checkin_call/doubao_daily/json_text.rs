//! 上游及原凭据中的JSON文本有界解析；重复键/深度/节点超限在构造无界Value前拒绝。

use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::DailyError;

const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 65_536;
const MAX_KEY: usize = 256;
const MAX_TEXT: usize = 1024 * 1024;

pub(super) fn parse(bytes: &[u8], maximum: usize) -> Result<Value, DailyError> {
    if bytes.is_empty() || bytes.len() > maximum { return Err(DailyError::ResponseInvalid); }
    let mut remaining = MAX_NODES;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Bounded { depth: 0, remaining: &mut remaining }.deserialize(&mut deserializer)
        .map_err(|_| DailyError::ResponseInvalid)?;
    deserializer.end().map_err(|_| DailyError::ResponseInvalid)?;
    Ok(value)
}

struct Bounded<'a> { depth: usize, remaining: &'a mut usize }

impl<'de> DeserializeSeed<'de> for Bounded<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if *self.remaining == 0 { return Err(de::Error::custom("daily JSON nodes")); }
        *self.remaining -= 1;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Bounded<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("bounded unique JSON") }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> { Ok(Value::Bool(value)) }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value).map(Value::Number).ok_or_else(|| de::Error::custom("daily JSON number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        if value.len() > MAX_TEXT { return Err(de::Error::custom("daily JSON text")); }
        Ok(Value::String(value.to_string()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        if value.len() > MAX_TEXT { return Err(de::Error::custom("daily JSON text")); }
        Ok(Value::String(value))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> { Ok(Value::Null) }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_DEPTH { return Err(de::Error::custom("daily JSON depth")); }
        let mut items = Vec::new();
        while let Some(value) = sequence.next_element_seed(Bounded { depth: self.depth + 1, remaining: &mut *self.remaining })? {
            items.push(value);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_DEPTH { return Err(de::Error::custom("daily JSON depth")); }
        let mut fields = Map::new();
        while let Some(name) = object.next_key::<String>()? {
            if name.len() > MAX_KEY || fields.contains_key(&name) { return Err(de::Error::custom("daily JSON key")); }
            let value = object.next_value_seed(Bounded { depth: self.depth + 1, remaining: &mut *self.remaining })?;
            fields.insert(name, value);
        }
        Ok(Value::Object(fields))
    }
}
