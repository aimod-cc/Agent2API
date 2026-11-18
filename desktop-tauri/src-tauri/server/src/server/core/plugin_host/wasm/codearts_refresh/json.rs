//! 固定 CodeArts 响应的独立有界 JSON；重复键/深度/节点在保留材料之前拒绝。
//! 自有明文和失败中间对象尽力清理；借入raw及serde解码器临时缓冲仍由调用者/库负责。

use std::{fmt, io, mem};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use zeroize::{Zeroize, Zeroizing};

use super::{ProtocolError, Result, MAX_RESPONSE_BYTES};

const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 4096;
const MAX_STRING_BYTES: usize = 8192;
const MAX_KEY_BYTES: usize = 128;
const MAX_ORIGINAL_BYTES: usize = 256 * 1024;
const MAX_ORIGINAL_DEPTH: usize = 32;
const MAX_ORIGINAL_NODES: usize = 16_384;

/// 无 Clone/Debug/serde；持有的字符串/对象键在任何早退和正常结束时尽力清理。
pub(super) struct SecretJson(Value);

impl SecretJson {
    pub(super) fn object() -> Self {
        Self(Value::Object(Map::new()))
    }
    fn array() -> Self {
        Self(Value::Array(Vec::new()))
    }
    pub(super) fn value(&self) -> &Value {
        &self.0
    }

    pub(super) fn insert(&mut self, name: &str, value: Value) -> Result<()> {
        let mut value = Self(value);
        let fields = self
            .0
            .as_object_mut()
            .ok_or(ProtocolError::InvalidResponse)?;
        if fields.contains_key(name) {
            return Err(ProtocolError::DuplicateKey);
        }
        fields.insert(name.to_string(), value.take());
        Ok(())
    }

    fn take(&mut self) -> Value {
        mem::replace(&mut self.0, Value::Null)
    }
}

impl Drop for SecretJson {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

fn wipe(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(wipe),
        Value::Object(fields) => {
            for (mut name, mut value) in mem::take(fields) {
                name.zeroize();
                wipe(&mut value);
            }
        }
        _ => {}
    }
    *value = Value::Null;
}

pub(super) fn parse(raw: &[u8]) -> Result<SecretJson> {
    if raw.len() > MAX_RESPONSE_BYTES {
        return Err(ProtocolError::LimitExceeded);
    }
    if raw.is_empty() {
        return Err(ProtocolError::InvalidJson);
    }
    let mut budget = Budget {
        remaining: MAX_NODES,
        failure: None,
    };
    let mut decoder = serde_json::Deserializer::from_slice(raw);
    let parsed = Bounded {
        containers: 0,
        budget: &mut budget,
    }
    .deserialize(&mut decoder);
    let value = parsed.map_err(|_| budget.failure.unwrap_or(ProtocolError::InvalidJson))?;
    decoder.end().map_err(|_| ProtocolError::InvalidJson)?;
    Ok(value)
}

struct Budget {
    remaining: usize,
    failure: Option<ProtocolError>,
}

impl Budget {
    fn error<E: de::Error>(&mut self, error: ProtocolError) -> E {
        self.failure.get_or_insert(error);
        E::custom("invalid bounded CodeArts token response")
    }
    fn node<E: de::Error>(&mut self) -> std::result::Result<(), E> {
        if self.remaining == 0 {
            return Err(self.error(ProtocolError::LimitExceeded));
        }
        self.remaining -= 1;
        Ok(())
    }
}

struct Bounded<'a> {
    containers: usize,
    budget: &'a mut Budget,
}

impl<'de> DeserializeSeed<'de> for Bounded<'_> {
    type Value = SecretJson;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<SecretJson, D::Error> {
        self.budget.node()?;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Bounded<'_> {
    type Value = SecretJson;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON with unique object keys")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<SecretJson, E> {
        Ok(SecretJson(Value::Bool(value)))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<SecretJson, E> {
        Ok(SecretJson(Value::Null))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<SecretJson, E> {
        Ok(SecretJson(Value::from(value)))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<SecretJson, E> {
        Ok(SecretJson(Value::from(value)))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<SecretJson, E> {
        Number::from_f64(value)
            .map(|number| SecretJson(Value::Number(number)))
            .ok_or_else(|| self.budget.error(ProtocolError::InvalidJson))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<SecretJson, E> {
        if value.len() > MAX_STRING_BYTES {
            return Err(self.budget.error(ProtocolError::LimitExceeded));
        }
        Ok(SecretJson(Value::String(value.to_string())))
    }
    fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<SecretJson, E> {
        let mut text = Zeroizing::new(value);
        if text.len() > MAX_STRING_BYTES {
            return Err(self.budget.error(ProtocolError::LimitExceeded));
        }
        Ok(SecretJson(Value::String(mem::take(&mut *text))))
    }

    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut input: A,
    ) -> std::result::Result<SecretJson, A::Error> {
        if self.containers >= MAX_DEPTH {
            return Err(self.budget.error(ProtocolError::LimitExceeded));
        }
        let mut out = SecretJson::array();
        while let Some(mut item) = input.next_element_seed(Bounded {
            containers: self.containers + 1,
            budget: &mut *self.budget,
        })? {
            if let Value::Array(items) = &mut out.0 {
                items.push(item.take());
            }
        }
        Ok(out)
    }

    fn visit_map<A: MapAccess<'de>>(
        self,
        mut input: A,
    ) -> std::result::Result<SecretJson, A::Error> {
        if self.containers >= MAX_DEPTH {
            return Err(self.budget.error(ProtocolError::LimitExceeded));
        }
        let mut out = SecretJson::object();
        while let Some(name) = input.next_key::<String>()? {
            let mut name = Zeroizing::new(name);
            self.budget.node()?; // 键与值均占预算。
            if name.len() > MAX_KEY_BYTES {
                return Err(self.budget.error(ProtocolError::LimitExceeded));
            }
            let Value::Object(fields) = &mut out.0 else {
                return Err(self.budget.error(ProtocolError::InvalidResponse));
            };
            if fields.contains_key(name.as_str()) {
                return Err(self.budget.error(ProtocolError::DuplicateKey));
            }
            let mut value = input.next_value_seed(Bounded {
                containers: self.containers + 1,
                budget: &mut *self.budget,
            })?;
            fields.insert(mem::take(&mut *name), value.take());
        }
        Ok(out)
    }
}

/// 原ReadStamp已经持有Value，只按借用检查，避免为限额校验先克隆整份secret。
/// 未知旧字段照常参与预算，不把其内容作为请求或新响应的一部分。
pub(super) fn check_original(value: &Value) -> Result<()> {
    let mut nodes = MAX_ORIGINAL_NODES;
    shape(value, 0, &mut nodes)?;
    let mut bytes = Count {
        remaining: MAX_ORIGINAL_BYTES,
    };
    serde_json::to_writer(&mut bytes, value).map_err(|_| ProtocolError::LimitExceeded)
}

fn shape(value: &Value, depth: usize, nodes: &mut usize) -> Result<()> {
    if *nodes == 0 {
        return Err(ProtocolError::LimitExceeded);
    }
    *nodes -= 1;
    match value {
        Value::String(text) if text.len() > MAX_ORIGINAL_BYTES => Err(ProtocolError::LimitExceeded),
        Value::Object(fields) => {
            if depth >= MAX_ORIGINAL_DEPTH {
                return Err(ProtocolError::LimitExceeded);
            }
            for (name, item) in fields {
                if *nodes == 0 || name.len() > MAX_ORIGINAL_BYTES {
                    return Err(ProtocolError::LimitExceeded);
                }
                *nodes -= 1;
                shape(item, depth + 1, nodes)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            if depth >= MAX_ORIGINAL_DEPTH {
                return Err(ProtocolError::LimitExceeded);
            }
            for item in items {
                shape(item, depth + 1, nodes)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

struct Count {
    remaining: usize,
}

impl io::Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::other("CodeArts protocol limit"));
        }
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
