//! 输入与本次 JWT 声明共用有界预算，重复键在生成候选 Value 前拒绝。
//! 自持键/字符串在失败与 Drop 尽力清理；raw 借用和 serde 内部临时缓冲不归本对象。

use std::{fmt, mem};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use zeroize::{Zeroize, Zeroizing};

use super::{FreeInputError, MAX_FREE_INPUT_BYTES};

const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 2048;
const MAX_STRING_BYTES: usize = 8192;
const MAX_KEY_BYTES: usize = 128;

/// 私有明文所有者，无 Clone/Debug/serde；只借出当前本片材料，不签发来源。
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

    pub(super) fn insert(&mut self, key: &str, value: Value) -> Result<(), FreeInputError> {
        let mut value = Self(value);
        let fields = self.0.as_object_mut().ok_or(FreeInputError::InvalidShape)?;
        if fields.contains_key(key) {
            return Err(FreeInputError::DuplicateKey);
        }
        fields.insert(key.to_string(), value.take());
        Ok(())
    }

    fn take(&mut self) -> Value {
        mem::replace(&mut self.0, Value::Null)
    }
    fn into_value(mut self) -> Value {
        self.take()
    }
    pub(super) fn null() -> Self {
        Self(Value::Null)
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

pub(super) struct Budget {
    remaining: usize,
    failure: Option<FreeInputError>,
}

impl Budget {
    pub(super) fn new() -> Self {
        Self {
            remaining: MAX_NODES,
            failure: None,
        }
    }

    pub(super) fn parse(&mut self, raw: &[u8]) -> Result<SecretJson, FreeInputError> {
        if raw.len() > MAX_FREE_INPUT_BYTES {
            return Err(FreeInputError::LimitExceeded);
        }
        if raw.is_empty() {
            return Err(FreeInputError::InvalidJson);
        }
        let mut decoder = serde_json::Deserializer::from_slice(raw);
        let parsed = Bounded {
            containers: 0,
            budget: self,
        }
        .deserialize(&mut decoder);
        let result = parsed.map_err(|_| self.failure.unwrap_or(FreeInputError::InvalidJson))?;
        decoder.end().map_err(|_| FreeInputError::InvalidJson)?;
        Ok(result)
    }

    fn error<E: de::Error>(&mut self, error: FreeInputError) -> E {
        self.failure.get_or_insert(error);
        E::custom("invalid bounded Cline Free input")
    }

    fn node<E: de::Error>(&mut self) -> Result<(), E> {
        if self.remaining == 0 {
            return Err(self.error(FreeInputError::LimitExceeded));
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
        Ok(SecretJson::null())
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
            .ok_or_else(|| self.budget.error(FreeInputError::InvalidJson))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<SecretJson, E> {
        if value.len() > MAX_STRING_BYTES {
            return Err(self.budget.error(FreeInputError::LimitExceeded));
        }
        Ok(SecretJson(Value::String(value.to_string())))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<SecretJson, E> {
        let mut value = Zeroizing::new(value);
        if value.len() > MAX_STRING_BYTES {
            return Err(self.budget.error(FreeInputError::LimitExceeded));
        }
        Ok(SecretJson(Value::String(mem::take(&mut *value))))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<SecretJson, A::Error> {
        if self.containers >= MAX_DEPTH {
            return Err(self.budget.error(FreeInputError::LimitExceeded));
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
            return Err(self.budget.error(FreeInputError::LimitExceeded));
        }
        let mut output = SecretJson::object();
        while let Some(key) = input.next_key::<String>()? {
            let mut key = Zeroizing::new(key);
            self.budget.node()?; // 对象键和值都计节点，不只计算值。
            if key.len() > MAX_KEY_BYTES {
                return Err(self.budget.error(FreeInputError::LimitExceeded));
            }
            let Value::Object(fields) = &mut output.0 else {
                return Err(self.budget.error(FreeInputError::InvalidShape));
            };
            if fields.contains_key(key.as_str()) {
                return Err(self.budget.error(FreeInputError::DuplicateKey));
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
