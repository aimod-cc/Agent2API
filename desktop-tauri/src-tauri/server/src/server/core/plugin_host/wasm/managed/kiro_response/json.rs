//! Kiro 响应私有有界 JSON；键和值字符串独立零化持有，解析失败也释放已建节点。
//! 不暴露 Value、序列化、Clone 或 Debug；未知数组仍完整校验，但不保存其子节点。
//! 不保证借入原字节、解析库临时缓冲、分配器历史副本或全内存被擦除。

use std::{borrow::Borrow, cmp::Ordering, collections::BTreeMap, fmt};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Number;
use zeroize::Zeroizing;

const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 16_384;
const MAX_STRING: usize = 64 * 1024;
const MAX_KEY: usize = 1024;
const INVALID: &str = "Kiro响应的JSON结构无效、字段重复或超过限制";

pub(super) struct SecretJson(Node);

enum Node {
    Null,
    Other,
    Number(Number),
    Text(Zeroizing<String>),
    Object(BTreeMap<Key, SecretJson>),
}

impl SecretJson {
    pub(super) fn get(&self, key: &str) -> Option<&Self> {
        match &self.0 {
            Node::Object(fields) => fields.get(key),
            _ => None,
        }
    }

    pub(super) fn is_null(&self) -> bool {
        matches!(self.0, Node::Null)
    }

    pub(super) fn is_object(&self) -> bool {
        matches!(self.0, Node::Object(_))
    }

    pub(super) fn as_str(&self) -> Option<&str> {
        match &self.0 {
            Node::Text(text) => Some(text.as_str()),
            _ => None,
        }
    }

    pub(super) fn as_i64(&self) -> Option<i64> {
        match &self.0 {
            Node::Number(number) => number.as_i64(),
            _ => None,
        }
    }
}

pub(super) fn parse(raw: &[u8], limit: usize) -> Result<SecretJson, ()> {
    if raw.is_empty() || raw.len() > limit || std::str::from_utf8(raw).is_err() {
        return Err(());
    }
    let mut budget = Budget {
        remaining: MAX_NODES,
    };
    let mut deserializer = serde_json::Deserializer::from_slice(raw);
    let response = Bounded {
        depth: 0,
        budget: &mut budget,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| ())?;
    deserializer.end().map_err(|_| ())?;
    if !response.is_object() {
        return Err(());
    }
    Ok(response)
}

struct Budget {
    remaining: usize,
}

impl Budget {
    fn take<E: de::Error>(&mut self) -> Result<(), E> {
        if self.remaining == 0 {
            return Err(E::custom(INVALID));
        }
        self.remaining -= 1;
        Ok(())
    }
}

// 每个键也计入节点预算；重复比较基于已解码内容，不接受转义别名绕过。
struct Key(Zeroizing<String>);

impl Borrow<str> for Key {
    fn borrow(&self) -> &str {
        self.0.as_str()
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

impl Eq for Key {}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.as_str().cmp(other.0.as_str())
    }
}

struct KeySeed<'a>(&'a mut Budget);

impl<'de> DeserializeSeed<'de> for KeySeed<'_> {
    type Value = Key;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Key, D::Error> {
        self.0.take()?;
        deserializer.deserialize_string(KeyVisitor)
    }
}

struct KeyVisitor;

impl<'de> Visitor<'de> for KeyVisitor {
    type Value = Key;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(INVALID)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Key, E> {
        if value.len() > MAX_KEY {
            return Err(E::custom(INVALID));
        }
        Ok(Key(Zeroizing::new(value.to_string())))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Key, E> {
        let value = Zeroizing::new(value);
        if value.len() > MAX_KEY {
            return Err(E::custom(INVALID));
        }
        Ok(Key(value))
    }
}

struct Bounded<'a> {
    depth: usize,
    budget: &'a mut Budget,
}

impl<'de> DeserializeSeed<'de> for Bounded<'_> {
    type Value = SecretJson;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<SecretJson, D::Error> {
        self.budget.take()?;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Bounded<'_> {
    type Value = SecretJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(INVALID)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<SecretJson, E> {
        Ok(SecretJson(Node::Other))
    }

    fn visit_unit<E: de::Error>(self) -> Result<SecretJson, E> {
        Ok(SecretJson(Node::Null))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<SecretJson, E> {
        Ok(SecretJson(Node::Number(value.into())))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<SecretJson, E> {
        Ok(SecretJson(Node::Number(value.into())))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<SecretJson, E> {
        Number::from_f64(value)
            .map(|number| SecretJson(Node::Number(number)))
            .ok_or_else(|| E::custom(INVALID))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<SecretJson, E> {
        if value.len() > MAX_STRING {
            return Err(E::custom(INVALID));
        }
        Ok(SecretJson(Node::Text(Zeroizing::new(value.to_string()))))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<SecretJson, E> {
        let value = Zeroizing::new(value);
        if value.len() > MAX_STRING {
            return Err(E::custom(INVALID));
        }
        Ok(SecretJson(Node::Text(value)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<SecretJson, A::Error> {
        if self.depth >= MAX_DEPTH {
            return Err(de::Error::custom(INVALID));
        }
        while sequence
            .next_element_seed(Bounded {
                depth: self.depth + 1,
                budget: &mut *self.budget,
            })?
            .is_some()
        {}
        Ok(SecretJson(Node::Other))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<SecretJson, A::Error> {
        if self.depth >= MAX_DEPTH {
            return Err(de::Error::custom(INVALID));
        }
        let mut fields: BTreeMap<Key, SecretJson> = BTreeMap::new();
        while let Some(key) = object.next_key_seed(KeySeed(&mut *self.budget))? {
            if fields.contains_key(key.0.as_str()) {
                return Err(de::Error::custom(INVALID));
            }
            let value = object.next_value_seed(Bounded {
                depth: self.depth + 1,
                budget: &mut *self.budget,
            })?;
            fields.insert(key, value);
        }
        Ok(SecretJson(Node::Object(fields)))
    }
}
