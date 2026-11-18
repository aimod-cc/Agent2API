//! 本次手填原文的有界JSON读取；重复键和限额在构造无界Value之前拒绝。
//! 局部解析失败也由SecretJson/Zeroizing清理已构造明文；借入raw与serde内部临时缓冲不归本对象所有。

use std::{fmt, mem};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use zeroize::{Zeroize, Zeroizing};

use super::{EnrollmentInputError, MAX_INPUT_BYTES};

const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 2048;
const MAX_STRING_BYTES: usize = 8192;
const MAX_KEY_BYTES: usize = 128;

/// 私有明文所有者；任何早退/正常释放都会清理字符串及对象键。无Clone/Debug/serde。
pub(super) struct SecretJson(Value);

impl SecretJson {
    pub(super) fn object() -> Self {
        Self(Value::Object(Map::new()))
    }
    pub(super) fn array() -> Self {
        Self(Value::Array(Vec::new()))
    }
    pub(super) fn value(&self) -> &Value {
        &self.0
    }
    pub(super) fn into_value(mut self) -> Value {
        mem::replace(&mut self.0, Value::Null)
    }

    pub(super) fn insert(&mut self, key: &str, value: Value) -> Result<(), EnrollmentInputError> {
        let mut value = Self(value);
        let fields = self
            .0
            .as_object_mut()
            .ok_or(EnrollmentInputError::InvalidShape)?;
        if fields.contains_key(key) {
            return Err(EnrollmentInputError::DuplicateKey);
        }
        fields.insert(key.to_string(), value.take());
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
            for (mut key, mut value) in mem::take(fields) {
                key.zeroize();
                wipe(&mut value);
            }
        }
        _ => {}
    }
    *value = Value::Null;
}

pub(super) fn parse(raw: &[u8]) -> Result<SecretJson, EnrollmentInputError> {
    if raw.len() > MAX_INPUT_BYTES {
        return Err(EnrollmentInputError::LimitExceeded);
    }
    if raw.is_empty() {
        return Err(EnrollmentInputError::InvalidJson);
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
    let result = parsed.map_err(|_| budget.failure.unwrap_or(EnrollmentInputError::InvalidJson))?;
    decoder
        .end()
        .map_err(|_| EnrollmentInputError::InvalidJson)?;
    Ok(result)
}

struct Budget {
    remaining: usize,
    failure: Option<EnrollmentInputError>,
}

impl Budget {
    fn error<E: de::Error>(&mut self, error: EnrollmentInputError) -> E {
        self.failure.get_or_insert(error);
        E::custom("invalid bounded CodeArts input")
    }
    fn node<E: de::Error>(&mut self) -> Result<(), E> {
        if self.remaining == 0 {
            return Err(self.error(EnrollmentInputError::LimitExceeded));
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

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<SecretJson, D::Error> {
        self.budget.node()?;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Bounded<'_> {
    type Value = SecretJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON with unique object keys")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<SecretJson, E> {
        Ok(SecretJson(Value::Bool(value)))
    }
    fn visit_unit<E: de::Error>(self) -> Result<SecretJson, E> {
        Ok(SecretJson(Value::Null))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<SecretJson, E> {
        Ok(SecretJson(Value::from(value)))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<SecretJson, E> {
        Ok(SecretJson(Value::from(value)))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<SecretJson, E> {
        Number::from_f64(value)
            .map(|number| SecretJson(Value::Number(number)))
            .ok_or_else(|| self.budget.error(EnrollmentInputError::InvalidJson))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<SecretJson, E> {
        if value.len() > MAX_STRING_BYTES {
            return Err(self.budget.error(EnrollmentInputError::LimitExceeded));
        }
        Ok(SecretJson(Value::String(value.to_string())))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<SecretJson, E> {
        let mut text = Zeroizing::new(value);
        if text.len() > MAX_STRING_BYTES {
            return Err(self.budget.error(EnrollmentInputError::LimitExceeded));
        }
        Ok(SecretJson(Value::String(mem::take(&mut *text))))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<SecretJson, A::Error> {
        if self.containers >= MAX_DEPTH {
            return Err(self.budget.error(EnrollmentInputError::LimitExceeded));
        }
        let mut output = SecretJson::array();
        while let Some(value) = input.next_element_seed(Bounded {
            containers: self.containers + 1,
            budget: &mut *self.budget,
        })? {
            if let Value::Array(items) = &mut output.0 {
                items.push(value.into_value());
            }
        }
        Ok(output)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut input: A) -> Result<SecretJson, A::Error> {
        if self.containers >= MAX_DEPTH {
            return Err(self.budget.error(EnrollmentInputError::LimitExceeded));
        }
        let mut output = SecretJson::object();
        while let Some(key) = input.next_key::<String>()? {
            let mut key = Zeroizing::new(key);
            self.budget.node()?; // 键与值均计节点，不让只计值的预算漏掉大对象。
            if key.len() > MAX_KEY_BYTES {
                return Err(self.budget.error(EnrollmentInputError::LimitExceeded));
            }
            let Value::Object(fields) = &mut output.0 else {
                return Err(self.budget.error(EnrollmentInputError::InvalidShape));
            };
            if fields.contains_key(key.as_str()) {
                return Err(self.budget.error(EnrollmentInputError::DuplicateKey));
            }
            let value = input.next_value_seed(Bounded {
                containers: self.containers + 1,
                budget: &mut *self.budget,
            })?;
            fields.insert(mem::take(&mut *key), value.into_value());
        }
        Ok(output)
    }
}
