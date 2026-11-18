//! 固定只读ACP工具投影。仅复制明确字段，不把title/kind猜成函数名，不补工具ID。
//! rawInput的完整值/增量语义未核实，始终标unknown；stdout/rawOutput/commands等不保留。
//! 混合扁平形态不用于投影：已知字段冲突拒绝，否则保守未知；明确失败不能变正常结束。

use std::{fmt, io};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Number, Value};

use super::super::strict_json::Object;
use super::{object, BoundedText, Invalid, MAX_EVENT_ID_BYTES, MAX_NAME_BYTES};

const MAX_TITLE_BYTES: usize = 4096;
const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_INPUT_DEPTH: usize = 16;
const MAX_INPUT_NODES: usize = 4096;
const MAX_TOOL_BYTES: usize = 128 * 1024;
// 当前宿主固定句柄为mt-加64个十六进制字符；这里只计编码预算，不构造或接收身份。
const RESOURCE_FIELD_BYTES: usize = 67 + 8;

pub(super) enum Parsed {
    Tool(ToolObservation),
    Unknown,
    Failed,
}

/// 私有字段只由有界解析产生；它只描述远端活动，不是批准或客户端执行材料。
pub(in crate::server::core::plugin_host::wasm::managed_task) struct ToolObservation {
    event: &'static str,
    call_id: Option<String>,
    title: Option<String>,
    kind: Option<String>,
    input: Input,
    state: Option<String>,
    output_bytes: usize,
}

impl ToolObservation {
    pub(in crate::server::core::plugin_host::wasm::managed_task) fn event(&self) -> &'static str {
        self.event
    }

    pub(in crate::server::core::plugin_host::wasm::managed_task) fn projection(&self) -> Value {
        json!({
            "call_id": self.call_id, "title": self.title, "kind": self.kind,
            "input_present": self.input.present, "input": self.input.value,
            "input_mode": "unknown", "state": self.state,
        })
    }

    pub(in crate::server::core::plugin_host::wasm::managed_task) fn output_bytes(&self) -> usize {
        self.output_bytes
    }

    pub(in crate::server::core::plugin_host::wasm::managed_task) fn failed(&self) -> bool {
        failed(self.state.as_deref())
    }
}

#[derive(Deserialize)]
struct Nested {
    update: Object<Update>,
    #[serde(rename = "type", default)]
    flat_type: TextField<MAX_NAME_BYTES>,
    #[serde(default)]
    tool_name: TextField<MAX_TITLE_BYTES>,
    #[serde(default)]
    tool_input: Input,
    #[serde(default)]
    delta: Input,
    #[serde(default)]
    status: TextField<MAX_NAME_BYTES>,
    #[serde(rename = "toolCallId", default)]
    flat_id: TextField<MAX_EVENT_ID_BYTES>,
}

#[derive(Deserialize)]
struct Update {
    #[serde(rename = "sessionUpdate")]
    event: BoundedText<MAX_NAME_BYTES>,
    // 只复制显式toolCallId；id/名称/最近工具均不能补造它，缺失保持null。
    #[serde(rename = "toolCallId", default)]
    call_id: TextField<MAX_EVENT_ID_BYTES>,
    #[serde(default)]
    title: TextField<MAX_TITLE_BYTES>,
    #[serde(default)]
    kind: TextField<MAX_NAME_BYTES>,
    #[serde(rename = "rawInput", default)]
    input: Input,
    #[serde(default)]
    status: TextField<MAX_NAME_BYTES>,
}

pub(super) fn nested(bytes: &[u8], expected: &str) -> Result<Parsed, Invalid> {
    let document: Nested = object(bytes)?;
    let update = document.update.0;
    if update.event.0 != expected {
        return Err(Invalid::Fields);
    }
    let event = match expected {
        "tool_call" => "call",
        "tool_call_update" => "update",
        _ => return Err(Invalid::Fields),
    };
    if update
        .status
        .value
        .as_deref()
        .is_some_and(|state| state.trim() != state)
        || document
            .status
            .value
            .as_deref()
            .is_some_and(|state| state.trim() != state)
    {
        return Err(Invalid::Fields);
    }
    if update.call_id.value.as_deref().is_some_and(|id| {
        id.is_empty() || id.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    }) {
        return Err(Invalid::Fields);
    }
    // 两个明确类型/身份/状态不相符时拒绝。rawInput/tool_input的双形态值仅用于冲突排除，
    // delta不被当作rawInput别名或增量；title和tool_name也不假定是同一函数名。
    if document.flat_type.present && document.flat_type.value.as_deref() != Some(expected) {
        return Err(Invalid::Fields);
    }
    conflict(&document.flat_id, &update.call_id)?;
    conflict(&document.status, &update.status)?;
    if document.tool_input.present
        && update.input.present
        && document.tool_input.value != update.input.value
    {
        return Err(Invalid::Fields);
    }
    let mixed = document.flat_type.present
        || document.flat_id.present
        || document.status.present
        || document.tool_name.present
        || document.tool_input.present
        || document.delta.present;
    if mixed {
        return Ok(
            if failed(update.status.value.as_deref()) || failed(document.status.value.as_deref()) {
                Parsed::Failed
            } else {
                Parsed::Unknown
            },
        );
    }
    let mut tool = ToolObservation {
        event,
        call_id: update.call_id.value,
        title: update.title.value,
        kind: update.kind.value,
        input: update.input,
        state: update.status.value,
        output_bytes: 0,
    };
    // 计read工具回包全部固定字段+本次句柄长度，而非只计rawInput；这里不制造占位句柄。
    tool.output_bytes = encoded_size(
        &json!({
            "status": "tool", "event": event, "tool": tool.projection(),
            "done": false, "replay_forbidden": true,
        }),
        MAX_TOOL_BYTES,
    )? + RESOURCE_FIELD_BYTES;
    Ok(Parsed::Tool(tool))
}

/// 未接入的扁平工具仍不能把明确failed/error吞掉后接受round_ended。
/// 不从它构造观察材料、ID、参数或权限；其余扁平形态保持原Unknown行为。
pub(super) fn unsupported_failure(bytes: &[u8]) -> Result<bool, Invalid> {
    #[derive(Deserialize)]
    struct Kind {
        #[serde(rename = "type", default)]
        kind: TextField<MAX_NAME_BYTES>,
    }
    let kind: Kind = object(bytes)?;
    if !matches!(
        kind.kind.value.as_deref(),
        Some("tool_call" | "tool_call_update")
    ) {
        return Ok(false);
    }
    #[derive(Deserialize)]
    struct Flat {
        #[serde(default)]
        status: TextField<MAX_NAME_BYTES>,
    }
    let flat: Flat = object(bytes)?;
    if flat
        .status
        .value
        .as_deref()
        .is_some_and(|state| state.trim() != state)
    {
        return Err(Invalid::Fields);
    }
    Ok(failed(flat.status.value.as_deref()))
}

fn failed(state: Option<&str>) -> bool {
    state.is_some_and(|state| {
        state.eq_ignore_ascii_case("failed") || state.eq_ignore_ascii_case("error")
    })
}

fn conflict<const LIMIT: usize>(
    left: &TextField<LIMIT>,
    right: &TextField<LIMIT>,
) -> Result<(), Invalid> {
    if left.present && right.present && left.value != right.value {
        return Err(Invalid::Fields);
    }
    Ok(())
}

#[derive(Default)]
struct TextField<const LIMIT: usize> {
    present: bool,
    value: Option<String>,
}

impl<'de, const LIMIT: usize> Deserialize<'de> for TextField<LIMIT> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Text<const LIMIT: usize>;
        impl<'de, const LIMIT: usize> Visitor<'de> for Text<LIMIT> {
            type Value = TextField<LIMIT>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded optional tool string")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(TextField {
                    present: true,
                    value: None,
                })
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.len() > LIMIT || value.chars().any(char::is_control) {
                    return Err(E::custom("invalid tool string"));
                }
                Ok(TextField {
                    present: true,
                    value: Some(value.to_string()),
                })
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                if value.len() > LIMIT || value.chars().any(char::is_control) {
                    return Err(E::custom("invalid tool string"));
                }
                Ok(TextField {
                    present: true,
                    value: Some(value),
                })
            }
        }
        deserializer.deserialize_any(Text::<LIMIT>)
    }
}

struct Input {
    present: bool,
    value: Value,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            present: false,
            value: Value::Null,
        }
    }
}

impl<'de> Deserialize<'de> for Input {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut budget = Budget {
            nodes: MAX_INPUT_NODES,
            bytes: MAX_INPUT_BYTES,
        };
        let value = Json {
            depth: 0,
            budget: &mut budget,
        }
        .deserialize(deserializer)?;
        encoded_size(&value, MAX_INPUT_BYTES)
            .map_err(|_| <D::Error as de::Error>::custom("tool input limit"))?;
        Ok(Self {
            present: true,
            value,
        })
    }
}

struct Budget {
    nodes: usize,
    bytes: usize,
}
impl Budget {
    fn charge<E: de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        if bytes > self.bytes {
            return Err(E::custom("tool input byte limit"));
        }
        self.bytes -= bytes;
        Ok(())
    }
}

struct Json<'a> {
    depth: usize,
    budget: &'a mut Budget,
}
impl<'de> DeserializeSeed<'de> for Json<'_> {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if self.budget.nodes == 0 {
            return Err(de::Error::custom("tool input node limit"));
        }
        self.budget.nodes -= 1;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Json<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded tool input JSON")
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        self.budget.charge(4)?;
        Ok(Value::Null)
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        self.budget.charge(if value { 4 } else { 5 })?;
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        self.budget.charge(value.to_string().len())?;
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        self.budget.charge(value.to_string().len())?;
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        let value =
            Number::from_f64(value).ok_or_else(|| E::custom("invalid tool input number"))?;
        self.budget.charge(value.to_string().len())?;
        Ok(Value::Number(value))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        self.budget.charge(value.len().saturating_add(2))?;
        Ok(Value::String(value.to_string()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        self.budget.charge(value.len().saturating_add(2))?;
        Ok(Value::String(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_INPUT_DEPTH {
            return Err(de::Error::custom("tool input depth limit"));
        }
        self.budget.charge(2)?;
        let mut items = Vec::new();
        while let Some(item) = sequence.next_element_seed(Json {
            depth: self.depth + 1,
            budget: &mut *self.budget,
        })? {
            self.budget.charge(1)?;
            items.push(item);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_INPUT_DEPTH {
            return Err(de::Error::custom("tool input depth limit"));
        }
        self.budget.charge(2)?;
        let mut fields = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            self.budget.charge(key.len().saturating_add(4))?;
            if fields.contains_key(&key) {
                return Err(de::Error::custom("duplicate tool input key"));
            }
            let value = object.next_value_seed(Json {
                depth: self.depth + 1,
                budget: &mut *self.budget,
            })?;
            fields.insert(key, value);
        }
        Ok(Value::Object(fields))
    }
}

fn encoded_size(value: &Value, limit: usize) -> Result<usize, Invalid> {
    struct Count {
        size: usize,
        limit: usize,
    }
    impl io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.size) {
                return Err(io::Error::other("tool projection limit"));
            }
            self.size += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count { size: 0, limit };
    serde_json::to_writer(&mut count, value).map_err(|_| Invalid::LimitExceeded)?;
    Ok(count.size)
}
