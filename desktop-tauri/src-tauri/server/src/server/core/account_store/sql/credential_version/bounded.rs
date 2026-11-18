//! 本合同独立的有界JSON读写；不改变普通账号解析，也不借用任务层许可。
//! 重复键、超深度/节点和超限正文在构造无界Value/序列化输出之前拒绝。

use std::{fmt, io};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::Error;

pub(super) const MAX_ROW_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_PLAIN_BYTES: usize = 1024 * 1024;
pub(super) const MAX_SECRET_BYTES: usize = 256 * 1024;
pub(super) const MAX_PUBLIC_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 16_384;

pub(super) fn parse(bytes: &[u8], limit: usize) -> Result<Value, Error> {
    if bytes.len() > limit {
        return Err(Error::LimitExceeded);
    }
    let mut remaining = MAX_NODES;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Bounded { containers: 0, remaining: &mut remaining }
        .deserialize(&mut deserializer).map_err(|_| Error::InvalidRecord)?;
    deserializer.end().map_err(|_| Error::InvalidRecord)?;
    Ok(value)
}

pub(super) fn encode(value: &Value, limit: usize) -> Result<Vec<u8>, Error> {
    shape_check(value)?;
    let mut out = Output { bytes: Vec::new(), limit };
    serde_json::to_writer(&mut out, value).map_err(|_| Error::LimitExceeded)?;
    Ok(out.bytes)
}

/// 普通写者已经持有字段表；先按借用检查，不能为限长校验先克隆整条大记录。
pub(super) fn encode_fields(fields: &Map<String, Value>, limit: usize) -> Result<Vec<u8>, Error> {
    let mut remaining = MAX_NODES - 1;
    for (key, value) in fields {
        if key.len() > MAX_PLAIN_BYTES { return Err(Error::LimitExceeded); }
        shape(value, 1, &mut remaining)?;
    }
    let mut out = Output { bytes: Vec::new(), limit };
    serde_json::to_writer(&mut out, fields).map_err(|_| Error::LimitExceeded)?;
    Ok(out.bytes)
}

pub(super) fn shape_check(value: &Value) -> Result<(), Error> {
    let mut remaining = MAX_NODES;
    shape(value, 0, &mut remaining)
}

pub(super) fn check(value: &Value, limit: usize) -> Result<(), Error> {
    let encoded = zeroize::Zeroizing::new(encode(value, limit)?);
    drop(encoded);
    Ok(())
}

fn shape(value: &Value, depth: usize, remaining: &mut usize) -> Result<(), Error> {
    if *remaining == 0 {
        return Err(Error::LimitExceeded);
    }
    *remaining -= 1;
    match value {
        Value::String(text) if text.len() > MAX_PLAIN_BYTES => return Err(Error::LimitExceeded),
        Value::Object(fields) => {
            if depth >= MAX_DEPTH {
                return Err(Error::LimitExceeded);
            }
            for (key, value) in fields {
                if key.len() > MAX_PLAIN_BYTES {
                    return Err(Error::LimitExceeded);
                }
                shape(value, depth + 1, remaining)?;
            }
        }
        Value::Array(items) => {
            if depth >= MAX_DEPTH {
                return Err(Error::LimitExceeded);
            }
            for item in items {
                shape(item, depth + 1, remaining)?;
            }
        }
        _ => {}
    }
    Ok(())
}

struct Output {
    bytes: Vec<u8>,
    limit: usize,
}

impl io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("credential JSON limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Bounded<'a> {
    containers: usize,
    remaining: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for Bounded<'_> {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if *self.remaining == 0 {
            return Err(de::Error::custom("credential JSON node limit"));
        }
        *self.remaining -= 1;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Bounded<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded credential JSON with unique keys")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> { Ok(Value::Bool(value)) }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value).map(Value::Number).ok_or_else(|| de::Error::custom("invalid credential number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> { Ok(Value::String(value.to_string())) }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> { Ok(Value::String(value)) }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> { Ok(Value::Null) }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        if self.containers >= MAX_DEPTH {
            return Err(de::Error::custom("credential JSON depth limit"));
        }
        let mut items = Vec::new();
        while let Some(value) = sequence.next_element_seed(Bounded {
            containers: self.containers + 1, remaining: &mut *self.remaining,
        })? {
            items.push(value);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        if self.containers >= MAX_DEPTH {
            return Err(de::Error::custom("credential JSON depth limit"));
        }
        let mut fields = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if fields.contains_key(&key) {
                return Err(de::Error::custom("duplicate credential JSON key"));
            }
            let value = object.next_value_seed(Bounded {
                containers: self.containers + 1, remaining: &mut *self.remaining,
            })?;
            fields.insert(key, value);
        }
        Ok(Value::Object(fields))
    }
}
