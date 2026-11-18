//! 本固定状态的有界原地JSON；失败时已解码字段留在外持有RawSlot，不在Db锁内丢弃大Value。
//! 不改变普通账号解析；32层/16384节点/原字节上限，重复键、尾随内容及非有限数均拒绝。

use std::{fmt, io::{self, Cursor}};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use zeroize::Zeroizing;

use super::super::{bounded, Error};

const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 16_384;

/// 自有输入和解析器scratch一起保留到锁外，不在parse失败时丢弃解码器持有的明文。
pub(super) type Decoder = serde_json::Deserializer<serde_json::de::IoRead<Cursor<Zeroizing<Vec<u8>>>>>;

pub(super) fn parse_into(
    bytes: &[u8], value: &mut Value, pending_key: &mut Option<String>, displaced_key: &mut Option<String>,
    decoder: &mut Option<Decoder>, limit: usize,
) -> Result<(), Error> {
    if bytes.len() > limit || !value.is_null() || pending_key.is_some() || displaced_key.is_some() || decoder.is_some() {
        return Err(Error::LimitExceeded);
    }
    *decoder = Some(serde_json::Deserializer::from_reader(Cursor::new(Zeroizing::new(bytes.to_vec()))));
    let deserializer = decoder.as_mut().ok_or(Error::InvalidRecord)?;
    let mut remaining = MAX_NODES;
    Seed { value, pending_key, displaced_key, remaining: &mut remaining, containers: 0 }
        .deserialize(&mut *deserializer).map_err(|_| Error::InvalidRecord)?;
    deserializer.end().map_err(|_| Error::InvalidRecord)
}

/// 兼容字段按真实解码对象计入整行同一个节点/深度预算，不把两次独立解析当成预算扩张。
pub(super) fn check_loaded(raw: &Value, plugin: Option<&Value>, public: Option<&Value>) -> Result<(), Error> {
    let fields = raw.as_object().ok_or(Error::InvalidRecord)?;
    let mut remaining = MAX_NODES - 1;
    for (key, value) in fields {
        if key.len() > bounded::MAX_PLAIN_BYTES { return Err(Error::LimitExceeded); }
        let decoded = match (key.as_str(), value) {
            ("pluginSecret", Value::String(_)) => plugin.ok_or(Error::InvalidRecord)?,
            ("pluginPublic", Value::String(_)) => public.ok_or(Error::InvalidRecord)?,
            _ => value,
        };
        shape(decoded, 1, &mut remaining)?;
    }
    Ok(())
}

fn shape(value: &Value, depth: usize, remaining: &mut usize) -> Result<(), Error> {
    if *remaining == 0 { return Err(Error::LimitExceeded); }
    *remaining -= 1;
    match value {
        Value::String(text) if text.len() > bounded::MAX_PLAIN_BYTES => return Err(Error::LimitExceeded),
        Value::Object(fields) => {
            if depth >= MAX_DEPTH { return Err(Error::LimitExceeded); }
            for (key, value) in fields {
                if key.len() > bounded::MAX_PLAIN_BYTES { return Err(Error::LimitExceeded); }
                shape(value, depth + 1, remaining)?;
            }
        }
        Value::Array(items) => {
            if depth >= MAX_DEPTH { return Err(Error::LimitExceeded); }
            for value in items { shape(value, depth + 1, remaining)?; }
        }
        _ => {}
    }
    Ok(())
}

/// 只计数，不为长度核对序列化第二份明文。
pub(super) fn check(value: &Value, limit: usize) -> Result<(), Error> {
    bounded::shape_check(value)?;
    serde_json::to_writer(Counter { left: limit }, value).map_err(|_| Error::LimitExceeded)
}

/// 编码失败也保留外持有缓冲，不能在锁内自动丢弃已生成明文。
pub(super) fn encode_into(value: &Value, bytes: &mut Vec<u8>, limit: usize) -> Result<(), Error> {
    if !bytes.is_empty() { return Err(Error::InvalidRecord); }
    bounded::shape_check(value)?;
    serde_json::to_writer(Output { bytes, limit }, value).map_err(|_| Error::LimitExceeded)
}

struct Counter { left: usize }
impl io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.left { return Err(io::Error::other("fixed state JSON limit")); }
        self.left -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

struct Output<'a> { bytes: &'a mut Vec<u8>, limit: usize }
impl io::Write for Output<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("fixed state JSON limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

struct Seed<'a> {
    value: &'a mut Value,
    pending_key: &'a mut Option<String>,
    displaced_key: &'a mut Option<String>,
    remaining: &'a mut usize,
    containers: usize,
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if *self.remaining == 0 || !self.value.is_null() {
            return Err(de::Error::custom("fixed state JSON node limit"));
        }
        *self.remaining -= 1;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded fixed state JSON with unique keys")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<(), E> {
        *self.value = Value::Bool(value); Ok(())
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<(), E> {
        *self.value = Value::Number(value.into()); Ok(())
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<(), E> {
        *self.value = Value::Number(value.into()); Ok(())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<(), E> {
        *self.value = Value::Number(Number::from_f64(value).ok_or_else(|| de::Error::custom("invalid number"))?);
        Ok(())
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        self.visit_string(value.to_string())
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<(), E> {
        *self.value = Value::String(value);
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> { Ok(()) }
    fn visit_none<E: de::Error>(self) -> Result<(), E> { Ok(()) }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        if self.containers >= MAX_DEPTH { return Err(de::Error::custom("fixed state JSON depth limit")); }
        *self.value = Value::Array(Vec::new());
        let items = self.value.as_array_mut().ok_or_else(|| de::Error::custom("invalid array"))?;
        loop {
            // 新slot先放到外持有根对象；解码失败时局部栈不拥有这一节点。
            items.push(Value::Null);
            let last = items.last_mut().ok_or_else(|| de::Error::custom("missing array slot"))?;
            if sequence.next_element_seed(Seed {
                value: last, pending_key: &mut *self.pending_key, displaced_key: &mut *self.displaced_key,
                remaining: &mut *self.remaining, containers: self.containers + 1,
            })?.is_none() {
                let _ = items.pop(); // 未解码的尾slot只是Null，不包含明文或异常载荷。
                return Ok(());
            }
        }
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<(), A::Error> {
        if self.containers >= MAX_DEPTH { return Err(de::Error::custom("fixed state JSON depth limit")); }
        *self.value = Value::Object(Map::new());
        let fields = self.value.as_object_mut().ok_or_else(|| de::Error::custom("invalid object"))?;
        while object.next_key_seed(Key { output: &mut *self.pending_key })?.is_some() {
            let key = self.pending_key.as_ref().ok_or_else(|| de::Error::custom("missing key"))?;
            if fields.contains_key(key) {
                *self.displaced_key = self.pending_key.take();
                return Err(de::Error::custom("duplicate fixed state JSON key"));
            }
            let key = self.pending_key.take().ok_or_else(|| de::Error::custom("missing key"))?;
            let slot = fields.entry(key).or_insert(Value::Null);
            object.next_value_seed(Seed {
                value: slot, pending_key: &mut *self.pending_key, displaced_key: &mut *self.displaced_key,
                remaining: &mut *self.remaining, containers: self.containers + 1,
            })?;
        }
        Ok(())
    }
}

struct Key<'a> { output: &'a mut Option<String> }
impl<'de> DeserializeSeed<'de> for Key<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if self.output.is_some() { return Err(de::Error::custom("unconsumed key")); }
        deserializer.deserialize_string(self)
    }
}
impl<'de> Visitor<'de> for Key<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result { formatter.write_str("JSON object key") }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> { self.visit_string(value.to_string()) }
    fn visit_string<E: de::Error>(self, value: String) -> Result<(), E> { *self.output = Some(value); Ok(()) }
}
