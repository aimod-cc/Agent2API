//! CatPaw 最终 Chat 输出的纯观察视图；不授予发送、工具交接或账号权限。
//! 只比较原完整工具证据的固定投影，不证明 HTTP 已写完或客户端已收到。

mod chat_stream;
mod converted;
pub(super) use converted::{verify_anthropic, verify_responses};

use agent2api_provider_protocol::WireError;
use serde_json::Value;

use super::{json_text, tool_evidence::ToolEvidence};

const MAX_TOTAL: usize = 64 * 1024 * 1024;
const MAX_EVENT: usize = 1024 * 1024;
const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_TOOLS: usize = 256;
const MAX_ARGUMENTS: usize = 256 * 1024;
const MAX_COUNT: u64 = 9_007_199_254_740_991;

/// 只有原 ToolEvidence 可构造；不持 SourceRef、账号、预约或任何发送凭证。
pub(super) struct ExpectedOutput {
    message: Projection,
    finish: &'static str,
}

/// 只表示一个完整 JSON 或完整 Chat SSE 已被观察；不是交付或工具接续回执。
/// 无 Clone/serde/公开字段，不能从调用者的 JSON 拼出该观察对象。
pub(in crate::server::core::plugin_host::wasm) struct OutputObservation {
    message: Projection,
    _identity: Identity,
    _usage: Option<Usage>,
    finish: &'static str,
}

impl OutputObservation {
    pub(super) fn chat_body(&self) -> Value {
        let mut message = serde_json::json!({"role": "assistant", "content": self.message.content});
        if !self.message.reasoning.is_empty() { message["reasoning_content"] = serde_json::json!(self.message.reasoning); }
        if !self.message.tools.is_empty() {
            message["tool_calls"] = Value::Array(self.message.tools.iter().map(|tool|
                serde_json::json!({"id": tool.id, "type": "function", "function": {"name": tool.name, "arguments": tool.arguments}})).collect());
        }
        let mut body = serde_json::json!({"id": self._identity.id, "object": "chat.completion", "created": self._identity.created,
            "model": self._identity.model, "choices": [{"index": 0, "message": message, "finish_reason": self.finish}]});
        if let Some(usage) = &self._usage {
            body["usage"] = serde_json::json!({"prompt_tokens": usage._prompt, "completion_tokens": usage._completion, "total_tokens": usage._total});
        }
        body
    }

    pub(super) fn matches_identity(&self, id: &str, model: &str, created: i64) -> bool {
        self._identity.id == id && self._identity.model == model
            && u64::try_from(created).ok() == Some(self._identity.created)
    }
}

pub(in crate::server::core::plugin_host::wasm) struct OutputObserver {
    input: Input,
    wire_bytes: usize,
    failed: bool,
}

enum Input {
    New,
    Body(OutputObservation),
    Stream(chat_stream::Stream),
}

impl ExpectedOutput {
    pub(super) fn from_evidence(evidence: &ToolEvidence) -> Result<Self, WireError> {
        let expected = Self::from_message(evidence.original_message())?;
        if expected.message.tools.len() != evidence.calls().len()
            || expected.message.tools.iter().zip(evidence.calls()).any(|(output, call)|
                output.id != call.id() || output.name != call.name() || output.arguments != call.arguments()) {
            return Err(invalid());
        }
        Ok(expected)
    }

    pub(super) fn from_message(original: &Value) -> Result<Self, WireError> {
        json_text::shape(original, MAX_MESSAGE).map_err(|_| invalid())?;
        super::message::fields(original).map_err(|_| invalid())?;
        if original["type"] != "assistant" || original["finished"] != true { return Err(invalid()); }
        if let Some(id) = original.get("messageId") { identifier(id, 256)?; }
        let blocks = original["content"].as_array().filter(|blocks| blocks.len() <= 1024).ok_or_else(invalid)?;
        let mut message = Projection::new();
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => {
                    json_text::fields(block, &["type", "text", "reasoningContent"], &["type"]).map_err(|_| invalid())?;
                    if block.get("text").is_none() && block.get("reasoningContent").is_none() { return Err(invalid()); }
                    if let Some(text) = block.get("text") { message.append_content(text.as_str().ok_or_else(invalid)?)?; }
                    if let Some(text) = block.get("reasoningContent") { message.append_reasoning(text.as_str().ok_or_else(invalid)?)?; }
                }
                Some("tool_use") => {
                    json_text::fields(block, &["type", "toolCallId", "toolName", "toolParams"],
                        &["type", "toolCallId", "toolName", "toolParams"]).map_err(|_| invalid())?;
                    let arguments = block["toolParams"].as_str().ok_or_else(invalid)?;
                    check_arguments(arguments)?;
                    message.add_tool(block["toolCallId"].as_str().ok_or_else(invalid)?,
                        block["toolName"].as_str().ok_or_else(invalid)?, arguments)?;
                }
                _ => return Err(invalid()),
            }
        }
        let finish = if message.tools.is_empty() { "stop" } else { "tool_calls" };
        Ok(Self { message, finish })
    }

    pub(super) fn matches_assistant(&self, assistant: &Value) -> Result<bool, WireError> {
        Ok(self.message.same(&Projection::from_chat(assistant)?))
    }

    pub(super) fn verify(&self, observation: &OutputObservation) -> Result<(), WireError> {
        if self.finish == observation.finish && self.message.same(&observation.message) {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}

impl OutputObserver {
    pub(in crate::server::core::plugin_host::wasm) fn new() -> Self {
        Self {
            input: Input::New,
            wire_bytes: 0,
            failed: false,
        }
    }

    pub(in crate::server::core::plugin_host::wasm) fn terminal_seen(&self) -> bool {
        !self.failed && matches!(&self.input, Input::Stream(stream) if stream.terminal_seen())
    }

    /// Value 不保留原始重复键或字节长度：入口接线必须先用 json_text 严格解析原文。
    /// 本入口仅消费一次完整 Chat completion；不能随后补 SSE 或再换一个完整正文。
    pub(in crate::server::core::plugin_host::wasm) fn observe_body(
        &mut self,
        body: &Value,
    ) -> Result<(), WireError> {
        if self.failed || !matches!(&self.input, Input::New) {
            self.failed = true;
            return Err(invalid());
        }
        let result = observe_body(body);
        match result {
            Ok(observation) => {
                self.input = Input::Body(observation);
                Ok(())
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    pub(in crate::server::core::plugin_host::wasm) fn push(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), WireError> {
        if self.failed || matches!(&self.input, Input::Body(_)) {
            self.failed = true;
            return Err(invalid());
        }
        let Some(total) = self
            .wire_bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= MAX_TOTAL)
        else {
            self.failed = true;
            return Err(invalid());
        };
        self.wire_bytes = total;
        if matches!(&self.input, Input::New) {
            self.input = Input::Stream(chat_stream::Stream::new());
        }
        let result = match &mut self.input {
            Input::Stream(stream) => stream.push(bytes),
            _ => Err(invalid()),
        };
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// 按值结束观察；错误、残帧、缺终态或任何前序失败均不能取得观察对象。
    pub(in crate::server::core::plugin_host::wasm) fn finish(
        self,
    ) -> Result<OutputObservation, WireError> {
        if self.failed {
            return Err(invalid());
        }
        match self.input {
            Input::Body(observation) => Ok(observation),
            Input::Stream(stream) => stream.finish(),
            Input::New => Err(invalid()),
        }
    }
}

fn observe_body(body: &Value) -> Result<OutputObservation, WireError> {
    json_text::shape(body, MAX_TOTAL).map_err(|_| invalid())?;
    let identity = Identity::parse(body, "chat.completion")?;
    let choices = body["choices"]
        .as_array()
        .filter(|choices| choices.len() == 1)
        .ok_or_else(invalid)?;
    let choice = &choices[0];
    json_text::fields(
        choice,
        &["index", "message", "finish_reason"],
        &["index", "message", "finish_reason"],
    )
    .map_err(|_| invalid())?;
    if choice["index"].as_u64() != Some(0) { return Err(invalid()); }
    let finish = match choice["finish_reason"].as_str() {
        Some("tool_calls") => "tool_calls",
        Some("stop") => "stop",
        _ => return Err(invalid()),
    };
    let message = Projection::from_chat(&choice["message"])?;
    if (finish == "tool_calls") == message.tools.is_empty() { return Err(invalid()); }
    let usage = match body.get("usage") {
        None | Some(Value::Null) => None,
        Some(value) => Some(Usage::parse(value)?),
    };
    Ok(OutputObservation {
        message,
        _identity: identity,
        _usage: usage,
        finish,
    })
}

struct Identity {
    id: String,
    model: String,
    created: u64,
}

impl Identity {
    fn parse(value: &Value, object: &str) -> Result<Self, WireError> {
        json_text::fields(
            value,
            &["id", "object", "created", "model", "choices", "usage"],
            &["id", "object", "created", "model", "choices"],
        )
        .map_err(|_| invalid())?;
        if value["object"].as_str() != Some(object) {
            return Err(invalid());
        }
        let id = identifier(&value["id"], 256)?;
        let model = identifier(&value["model"], 1024)?;
        let created = value["created"]
            .as_u64()
            .filter(|created| *created > 0 && *created <= 253_402_300_799)
            .ok_or_else(invalid)?;
        Ok(Self {
            id: id.to_string(),
            model: model.to_string(),
            created,
        })
    }

    fn same(&self, other: &Self) -> bool {
        self.id == other.id && self.model == other.model && self.created == other.created
    }
}

struct Usage {
    _prompt: u64,
    _completion: u64,
    _total: u64,
}

impl Usage {
    fn parse(value: &Value) -> Result<Self, WireError> {
        json_text::fields(
            value,
            &["prompt_tokens", "completion_tokens", "total_tokens"],
            &["prompt_tokens", "completion_tokens", "total_tokens"],
        )
        .map_err(|_| invalid())?;
        let count = |name: &str| {
            value[name]
                .as_u64()
                .filter(|n| *n <= MAX_COUNT)
                .ok_or_else(invalid)
        };
        let prompt = count("prompt_tokens")?;
        let completion = count("completion_tokens")?;
        let total = count("total_tokens")?;
        if prompt.checked_add(completion) != Some(total) {
            return Err(invalid());
        }
        // 零仅能是已观察到的显式整数；缺字段/null 从不补成三个零。
        Ok(Self {
            _prompt: prompt,
            _completion: completion,
            _total: total,
        })
    }
}

struct ToolOutput {
    id: String,
    name: String,
    arguments: String,
}

struct Projection {
    content: String,
    reasoning: String,
    tools: Vec<ToolOutput>,
    bytes: usize,
}

impl Projection {
    fn new() -> Self {
        Self {
            content: String::new(),
            reasoning: String::new(),
            tools: Vec::new(),
            bytes: b"{\"role\":\"assistant\",\"content\":\"\",\"tool_calls\":[]}".len(),
        }
    }

    fn from_chat(value: &Value) -> Result<Self, WireError> {
        json_text::shape(value, MAX_MESSAGE).map_err(|_| invalid())?;
        json_text::fields(
            value,
            &["role", "content", "reasoning_content", "tool_calls"],
            &["role"],
        )
        .map_err(|_| invalid())?;
        if value["role"] != "assistant" {
            return Err(invalid());
        }
        let mut message = Self::new();
        match value.get("content") {
            None | Some(Value::Null) => {},
            Some(Value::String(text)) => message.append_content(text)?,
            _ => return Err(invalid()),
        }
        if let Some(reasoning) = value.get("reasoning_content") {
            message.append_reasoning(reasoning.as_str().ok_or_else(invalid)?)?;
        }
        let tools = match value.get("tool_calls") {
            None | Some(Value::Null) => &[][..],
            Some(Value::Array(tools)) if tools.len() <= MAX_TOOLS => tools.as_slice(),
            _ => return Err(invalid()),
        };
        for tool in tools {
            json_text::fields(
                tool,
                &["id", "type", "function"],
                &["id", "type", "function"],
            )
            .map_err(|_| invalid())?;
            json_text::fields(
                &tool["function"],
                &["name", "arguments"],
                &["name", "arguments"],
            )
            .map_err(|_| invalid())?;
            if tool["type"] != "function" {
                return Err(invalid());
            }
            let id = tool["id"].as_str().ok_or_else(invalid)?;
            let name = tool["function"]["name"].as_str().ok_or_else(invalid)?;
            let arguments = tool["function"]["arguments"].as_str().ok_or_else(invalid)?;
            check_arguments(arguments)?;
            message.add_tool(id, name, arguments)?;
        }
        Ok(message)
    }

    fn append_content(&mut self, text: &str) -> Result<(), WireError> {
        self.charge(escaped_bytes(text)?)?;
        self.content.push_str(text);
        Ok(())
    }

    fn append_reasoning(&mut self, text: &str) -> Result<(), WireError> {
        let field = if self.reasoning.is_empty() && !text.is_empty() {
            b",\"reasoning_content\":\"\"".len()
        } else {
            0
        };
        self.charge(
            escaped_bytes(text)?
                .checked_add(field)
                .ok_or_else(invalid)?,
        )?;
        self.reasoning.push_str(text);
        Ok(())
    }

    fn add_tool(&mut self, id: &str, name: &str, arguments: &str) -> Result<(), WireError> {
        if self.tools.len() >= MAX_TOOLS
            || !json_text::identifier(id, 256)
            || !valid_name(name)
            || arguments.len() > MAX_ARGUMENTS
            || self.tools.iter().any(|tool| tool.id == id)
        {
            return Err(invalid());
        }
        let mut bytes =
            b"{\"id\":\"\",\"type\":\"function\",\"function\":{\"name\":\"\",\"arguments\":\"\"}}"
                .len();
        bytes += usize::from(!self.tools.is_empty());
        for text in [id, name, arguments] {
            bytes = bytes
                .checked_add(escaped_bytes(text)?)
                .ok_or_else(invalid)?;
        }
        self.charge(bytes)?;
        self.tools.push(ToolOutput {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        });
        Ok(())
    }

    fn append_arguments(&mut self, index: usize, text: &str) -> Result<(), WireError> {
        let tool = self.tools.get(index).ok_or_else(invalid)?;
        if text.is_empty()
            || tool
                .arguments
                .len()
                .checked_add(text.len())
                .is_none_or(|bytes| bytes > MAX_ARGUMENTS)
        {
            return Err(invalid());
        }
        self.charge(escaped_bytes(text)?)?;
        self.tools[index].arguments.push_str(text);
        Ok(())
    }

    fn complete_tools(&self) -> Result<(), WireError> {
        if self.tools.is_empty() {
            return Err(invalid());
        }
        for tool in &self.tools {
            check_arguments(&tool.arguments)?;
        }
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<(), WireError> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|bytes| *bytes <= MAX_MESSAGE)
            .ok_or_else(invalid)?;
        Ok(())
    }

    fn same(&self, other: &Self) -> bool {
        self.content == other.content
            && self.reasoning == other.reasoning
            && self.tools.len() == other.tools.len()
            && self
                .tools
                .iter()
                .zip(&other.tools)
                .all(|(expected, observed)| {
                    expected.id == observed.id
                        && expected.name == observed.name
                        && expected.arguments == observed.arguments
                })
    }
}

fn check_arguments(arguments: &str) -> Result<(), WireError> {
    let parsed = json_text::parse(arguments, MAX_ARGUMENTS).map_err(|_| invalid())?;
    if parsed.is_object() {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn identifier(value: &Value, maximum: usize) -> Result<&str, WireError> {
    value
        .as_str()
        .filter(|text| json_text::identifier(text, maximum))
        .ok_or_else(invalid)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

/// 与 JSON 字符串编码逐字节计费，不重编码工具参数，也不每帧复制整个累计消息。
fn escaped_bytes(text: &str) -> Result<usize, WireError> {
    text.bytes().try_fold(0usize, |total, byte| {
        let bytes = match byte {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 8 | 12 => 2,
            0..=31 => 6,
            _ => 1,
        };
        total
            .checked_add(bytes)
            .filter(|bytes| *bytes <= MAX_MESSAGE)
            .ok_or_else(invalid)
    })
}

fn invalid() -> WireError {
    WireError::with_code(
        "CatPaw 最终Chat输出不完整、重复、冲突、超预算或与原全部工具消息不一致；未取得输出观察或交接许可",
        "catpaw_turn_output_observation_invalid",
    )
}
