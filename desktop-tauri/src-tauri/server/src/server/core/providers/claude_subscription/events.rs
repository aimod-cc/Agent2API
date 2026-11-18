//! Magpie readOutput 的 stream_event/assistant/result 归一。
//! message_stop 不是请求成功：普通回复必须等 result；工具回复必须等 MCP 已挂起。
//! 没有终止原因的截断一律失败。完整 assistant 与增量双报按 message id 去重。
use super::{
    errors,
    tools::{Call, ToolSet, PREFIX},
};
use crate::server::errors::GatewayError;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

const MAX_REPLY: usize = 32 * 1024 * 1024;
#[derive(Default, Clone)]
struct Usage {
    input: i64,
    output: i64,
    read: i64,
    write: i64,
    thinking: i64,
}
impl Usage {
    fn update(&mut self, value: &Value) {
        for (key, target) in [
            ("input_tokens", &mut self.input),
            ("output_tokens", &mut self.output),
            ("cache_read_input_tokens", &mut self.read),
            ("cache_creation_input_tokens", &mut self.write),
        ] {
            if let Some(count) = value.get(key).and_then(Value::as_i64).filter(|n| *n >= 0) {
                *target = count;
            }
        }
        if let Some(count) =
            value.pointer("/output_tokens_details/thinking_tokens").and_then(Value::as_i64).filter(|n| *n >= 0)
        {
            self.thinking = count;
        }
    }
    fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.read = self.read.saturating_add(other.read);
        self.write = self.write.saturating_add(other.write);
        self.thinking = self.thinking.saturating_add(other.thinking);
    }
    fn chat(&self) -> Value {
        let input = self.input.saturating_add(self.read).saturating_add(self.write);
        json!({"prompt_tokens":input,"completion_tokens":self.output,"total_tokens":input.saturating_add(self.output),
            "prompt_tokens_details":{"cached_tokens":self.read},"cache_creation_input_tokens":self.write,
            "completion_tokens_details":{"reasoning_tokens":self.thinking}})
    }
}

#[derive(Clone)]
pub(super) struct Reply {
    pub text: String,
    pub thinking: String,
    pub calls: Vec<Call>,
    pub usage: Value,
    pub finish: String,
}
impl Reply {
    fn empty() -> Self {
        Self {
            text: String::new(),
            thinking: String::new(),
            calls: Vec::new(),
            usage: Value::Null,
            finish: String::new(),
        }
    }
    pub fn message(&self) -> Value {
        let mut message = json!({"role":"assistant","content":self.text});
        if !self.thinking.is_empty() {
            message["reasoning_content"] = json!(self.thinking);
        }
        if !self.calls.is_empty() {
            message["tool_calls"] = json!(self.calls.iter().map(|call| json!({"id":call.id,"type":"function", "function":{"name":call.name,"arguments":call.args.to_string()}})).collect::<Vec<_>>());
        }
        message
    }
}

pub(super) enum Boundary {
    Tools,
    Complete,
}
#[derive(Default)]
pub(super) struct Update {
    pub deltas: Vec<Value>,
    pub boundary: Option<Boundary>,
    pub limits: Option<Value>,
}
struct Block {
    kind: String,
    id: String,
    name: String,
    args: String,
    input: Value,
    tool_index: Option<usize>,
    closed: bool,
}

pub(super) struct Decoder {
    pub reply: Reply,
    tools: ToolSet,
    current: String,
    stop: String,
    blocks: BTreeMap<usize, Block>,
    seen: HashSet<String>,
    usages: BTreeMap<String, Usage>,
    shown: bool,
    cut: bool,
    tools_ended: bool,
    total_bytes: usize,
    whole_count: usize,
    deferred_whole: Vec<Value>,
    tool_pending: bool,
    wire_bytes: usize,
    last_limits: Option<Value>,
}

impl Decoder {
    pub fn new(tools: ToolSet) -> Self {
        Self {
            reply: Reply::empty(),
            tools,
            current: String::new(),
            stop: String::new(),
            blocks: BTreeMap::new(),
            seen: HashSet::new(),
            usages: BTreeMap::new(),
            shown: false,
            cut: false,
            tools_ended: false,
            total_bytes: 0,
            whole_count: 0,
            deferred_whole: Vec::new(),
            tool_pending: false,
            wire_bytes: 0,
            last_limits: None,
        }
    }
    pub fn expected_calls(&self) -> &[Call] {
        &self.reply.calls
    }
    pub fn is_duplicate_whole(&self, envelope: &Value) -> bool {
        envelope.get("type").and_then(Value::as_str) == Some("assistant")
            && envelope.pointer("/message/id").and_then(Value::as_str).is_some_and(|id| self.seen.contains(id))
    }
    pub fn begin(&mut self, tools: ToolSet) {
        self.reply = Reply::empty();
        self.tools = tools;
        self.current.clear();
        self.stop.clear();
        self.blocks.clear();
        self.usages.clear();
        self.shown = false;
        self.cut = false;
        self.tools_ended = false;
        self.total_bytes = 0;
        self.whole_count = 0;
        self.deferred_whole.clear();
        self.tool_pending = false;
        self.last_limits = None;
        self.wire_bytes = 0;
        if self.seen.len() > 2048 {
            self.seen.clear();
        }
    }
    fn charge(&mut self, bytes: usize) -> Result<(), GatewayError> {
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        if self.total_bytes > MAX_REPLY {
            return Err(errors::protocol("Claude 回复超过本次内存上限，未当成完整回复"));
        }
        Ok(())
    }
    fn usage(&mut self, value: &Value) {
        if !self.current.is_empty() && value.is_object() {
            self.usages.entry(self.current.clone()).or_default().update(value);
            let mut sum = Usage::default();
            for usage in self.usages.values() {
                sum.add(usage);
            }
            self.reply.usage = sum.chat();
        }
    }
    fn start(&mut self, id: &str, usage: &Value) -> Result<(), GatewayError> {
        if id.is_empty() {
            return Err(errors::protocol("Claude 消息缺少 id"));
        }
        if self.cut && self.shown || !self.current.is_empty() && self.stop.is_empty() && self.shown {
            return Err(errors::protocol("Claude 在未完整结束回复后重试，不能将两次答案拼成成功"));
        }
        if self.seen.len() >= 4096 || self.usages.len() >= 4096 {
            return Err(errors::protocol("Claude 单次会话消息数量超过资源上限"));
        }
        self.current = id.into();
        self.blocks.clear();
        self.stop.clear();
        self.whole_count = 0;
        self.cut = false;
        self.usage(usage);
        Ok(())
    }

    pub fn flush_whole(&mut self) -> Result<Update, GatewayError> {
        let mut update = Update::default();
        if self.deferred_whole.is_empty() {
            return Ok(update);
        }
        for message in std::mem::take(&mut self.deferred_whole) {
            self.whole(&message, &mut update)?;
        }
        if !self.tools_ended && self.tool_pending && !self.reply.calls.is_empty() {
            self.stop = "tool_use".into();
            self.finish_tools(&mut update)?;
        }
        self.tool_pending = false;
        Ok(update)
    }
    pub fn event(&mut self, envelope: &Value) -> Result<Update, GatewayError> {
        self.wire_bytes = self.wire_bytes.saturating_add(envelope.to_string().len());
        if self.wire_bytes > 64 * 1024 * 1024 {
            return Err(errors::protocol("Claude 本轮原始输出超过资源上限"));
        }
        let mut update = Update::default();
        match envelope.get("type").and_then(Value::as_str).unwrap_or("") {
            "rate_limit_event" => {
                self.last_limits = envelope.get("rate_limit_info").cloned();
                update.limits = self.last_limits.clone();
            }
            "error" => return Err(errors::upstream(envelope)),
            "assistant" => {
                // stream-json 同时发送完整消息；它可能先于增量 message_start。
                // 和 Magpie 一样等待短暂安静窗口，优先使用真正的流而不重复输出。
                self.charge(envelope.to_string().len())?;
                self.deferred_whole.push(envelope.clone());
            }
            "stream_event" => {
                if envelope.get("parent_tool_use_id").is_some_and(|v| !v.is_null()) {
                    return Err(errors::protocol("Claude 启动了不允许的本机子任务，已终止"));
                }
                let event = envelope.get("event").ok_or_else(|| errors::protocol("Claude stream_event 缺少事件"))?;
                match event.get("type").and_then(Value::as_str).unwrap_or("") {
                    "message_start" => {
                        let id = event.pointer("/message/id").and_then(Value::as_str).unwrap_or("");
                        if id == self.current && self.whole_count > 0 {
                            return Err(errors::protocol("Claude 完整消息已交付后才开始重发增量，未将两份回复拼接"));
                        }
                        self.start(id, event.pointer("/message/usage").unwrap_or(&Value::Null))?;
                        self.seen.insert(id.into());
                    }
                    "content_block_start" => {
                        let index = index(event)?;
                        self.block_start(index, event.get("content_block").unwrap_or(&Value::Null), &mut update)?;
                    }
                    "content_block_delta" => {
                        let index = index(event)?;
                        let delta = event.get("delta").unwrap_or(&Value::Null);
                        self.block_delta(index, delta, &mut update)?;
                    }
                    "content_block_stop" => self.block_stop(index(event)?, &mut update)?,
                    "message_delta" => {
                        self.usage(event.get("usage").unwrap_or(&Value::Null));
                        if let Some(stop) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                            self.stop = stop.into();
                        }
                    }
                    "message_stop" => {
                        if self.stop.is_empty() {
                            self.cut = true;
                            if self.shown {
                                return Err(errors::protocol("Claude 输出被截断，没有完整终止原因"));
                            }
                        } else if self.stop == "tool_use" && !self.reply.calls.is_empty() {
                            self.finish_tools(&mut update)?;
                        }
                    }
                    "error" => return Err(errors::upstream(event)),
                    _ => {}
                }
            }
            "result" => {
                for message in std::mem::take(&mut self.deferred_whole) {
                    self.whole(&message, &mut update)?;
                }
                if envelope.get("is_error").and_then(Value::as_bool) == Some(true)
                    || envelope.get("subtype").and_then(Value::as_str).is_some_and(|s| s != "success")
                {
                    let mut details = envelope.clone();
                    if details.get("rate_limit_info").is_none() {
                        if let Some(limits) = &self.last_limits {
                            details["rate_limit_info"] = limits.clone();
                        }
                    }
                    return Err(errors::upstream(&details));
                }
                if self.tools_ended {
                    return Err(errors::protocol("Claude 尚有待回传工具时提前结束了会话"));
                }
                if self.cut {
                    return Err(errors::protocol("Claude 流已截断，不能报告成功"));
                }
                if self.tools.required && self.reply.calls.is_empty() {
                    return Err(errors::protocol("客户端要求调用工具，但 Claude Code 未发出工具调用"));
                }
                if !self.reply.calls.is_empty() {
                    return Err(errors::protocol("Claude 工具调用没有进入可回传状态"));
                }
                if self.current.is_empty() {
                    return Err(errors::protocol("Claude 没有返回有效 assistant 消息"));
                }
                if self.blocks.values().any(|block| !block.closed) {
                    return Err(errors::protocol("Claude 回复的内容块尚未结束"));
                }
                self.reply.finish = match self.stop.as_str() {
                    "max_tokens" => "length",
                    "refusal" => "content_filter",
                    "end_turn" | "stop_sequence" => "stop",
                    _ => return Err(errors::protocol("Claude 回复缺少可确认的终止原因")),
                }
                .into();
                self.seen.insert(self.current.clone());
                update.boundary = Some(Boundary::Complete);
            }
            _ => {}
        }
        Ok(update)
    }

    fn block_start(&mut self, index: usize, value: &Value, update: &mut Update) -> Result<(), GatewayError> {
        if self.current.is_empty() || self.blocks.contains_key(&index) {
            return Err(errors::protocol("Claude 内容块顺序无效"));
        }
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        let mut block = Block {
            kind: kind.into(),
            id: String::new(),
            name: String::new(),
            args: String::new(),
            input: Value::Null,
            tool_index: None,
            closed: false,
        };
        match kind {
            "text" => {
                let text = value.get("text").and_then(Value::as_str).unwrap_or("");
                self.charge(text.len())?;
                if !text.is_empty() {
                    self.reply.text.push_str(text);
                    update.deltas.push(json!({"content":text}));
                    self.shown = true;
                }
            }
            "thinking" | "redacted_thinking" => {}
            "tool_use" => {
                let name = value.get("name").and_then(Value::as_str).unwrap_or("");
                if !name.starts_with(PREFIX) || self.tools.original(name).is_none() {
                    return Err(errors::protocol("Claude 请求了未授权的本机工具，已终止"));
                }
                block.id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| errors::protocol("Claude 工具调用缺少 id"))?
                    .into();
                block.name = self.tools.original(name).unwrap_or("").into();
                block.input = value.get("input").cloned().unwrap_or_else(|| json!({}));
                if self.tools.can_call(name) {
                    if self.reply.calls.len() >= 256 || self.reply.calls.iter().any(|call| call.id == block.id) {
                        return Err(errors::protocol("Claude 工具调用重复或过多"));
                    }
                    let tool_index = self.reply.calls.len();
                    block.tool_index = Some(tool_index);
                    self.reply.calls.push(Call { id: block.id.clone(), name: block.name.clone(), args: Value::Null });
                    update.deltas.push(json!({"tool_calls":[{"index":tool_index,"id":block.id,"type":"function","function":{"name":block.name,"arguments":""}}]}));
                    self.shown = true;
                }
            }
            _ => return Err(errors::protocol("Claude 返回了未支持的内容块，未将其丢弃后报告成功")),
        }
        self.blocks.insert(index, block);
        Ok(())
    }

    fn block_delta(&mut self, index: usize, value: &Value, update: &mut Update) -> Result<(), GatewayError> {
        let kind = self
            .blocks
            .get(&index)
            .filter(|b| !b.closed)
            .map(|b| b.kind.clone())
            .ok_or_else(|| errors::protocol("Claude 增量找不到对应内容块"))?;
        match value.get("type").and_then(Value::as_str).unwrap_or("") {
            "text_delta" if kind == "text" => {
                let text = value.get("text").and_then(Value::as_str).unwrap_or("");
                self.charge(text.len())?;
                self.reply.text.push_str(text);
                update.deltas.push(json!({"content":text}));
                self.shown |= !text.is_empty();
            }
            "thinking_delta" if kind == "thinking" => {
                let text = value.get("thinking").and_then(Value::as_str).unwrap_or("");
                self.charge(text.len())?;
                self.reply.thinking.push_str(text);
                update.deltas.push(json!({"reasoning_content":text}));
                self.shown |= !text.is_empty();
            }
            "signature_delta" if kind == "thinking" => {} // 官方 CLI 自己持有原始签名，不伪造或跨会话回放。
            "input_json_delta" if kind == "tool_use" => {
                let text = value.get("partial_json").and_then(Value::as_str).unwrap_or("");
                self.charge(text.len())?;
                if let Some(block) = self.blocks.get_mut(&index) {
                    block.args.push_str(text);
                    if let Some(tool_index) = block.tool_index {
                        update.deltas.push(json!({"tool_calls":[{"index":tool_index,"function":{"arguments":text}}]}));
                    }
                }
            }
            _ => return Err(errors::protocol("Claude 内容增量与内容块不匹配")),
        }
        Ok(())
    }

    fn block_stop(&mut self, index: usize, update: &mut Update) -> Result<(), GatewayError> {
        let block = self.blocks.get_mut(&index).ok_or_else(|| errors::protocol("Claude 结束了不存在的内容块"))?;
        if block.closed {
            return Err(errors::protocol("Claude 内容块重复结束"));
        }
        block.closed = true;
        if let Some(tool_index) = block.tool_index {
            let args = if block.args.is_empty() {
                update.deltas.push(
                    json!({"tool_calls":[{"index":tool_index,"function":{"arguments":block.input.to_string()}}]}),
                );
                block.input.clone()
            } else {
                serde_json::from_str(&block.args).map_err(|_| errors::protocol("Claude 工具 JSON 参数被截断或无效"))?
            };
            if !args.is_object() {
                return Err(errors::protocol("Claude 工具参数必须是完整对象"));
            }
            if let Some(call) = self.reply.calls.get_mut(tool_index) {
                call.args = args;
            }
        }
        Ok(())
    }

    fn finish_tools(&mut self, update: &mut Update) -> Result<(), GatewayError> {
        if self.tools_ended {
            return Ok(());
        }
        if self.blocks.values().any(|block| !block.closed) || self.reply.calls.iter().any(|call| !call.args.is_object())
        {
            return Err(errors::protocol("Claude 工具调用尚未完整结束"));
        }
        self.seen.insert(self.current.clone());
        self.tools_ended = true;
        self.reply.finish = "tool_calls".into();
        update.boundary = Some(Boundary::Tools);
        Ok(())
    }

    fn whole(&mut self, envelope: &Value, update: &mut Update) -> Result<(), GatewayError> {
        let message = envelope.get("message").ok_or_else(|| errors::protocol("Claude assistant 缺少消息"))?;
        if message.get("model").and_then(Value::as_str) == Some("<synthetic>")
            || envelope.get("error").is_some_and(|v| !v.is_null())
        {
            return Err(errors::upstream(
                &json!({"result":crate::server::core::protocol::content_text(message.get("content").unwrap_or(&Value::Null)),
                "error":envelope.get("error"),"api_error_status":envelope.get("api_error_status")}),
            ));
        }
        let id = message.get("id").and_then(Value::as_str).unwrap_or("");
        if self.seen.contains(id) {
            if id == self.current {
                self.usage(message.get("usage").unwrap_or(&Value::Null));
            }
            return Ok(());
        }
        if self.current != id {
            self.start(id, message.get("usage").unwrap_or(&Value::Null))?;
        }
        let parts = message
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| errors::protocol("Claude assistant 内容无效"))?;
        // Print 模式非流回退可能一次给多个完整块；每个 envelope 是新块，不按位置覆写。
        for part in parts {
            let index = self.whole_count;
            self.whole_count += 1;
            self.block_start(index, part, update)?;
            if part.get("type").and_then(Value::as_str) == Some("thinking") {
                self.block_delta(index, &json!({"type":"thinking_delta","thinking":part.get("thinking").and_then(Value::as_str).unwrap_or("")}), update)?;
            }
            self.block_stop(index, update)?;
        }
        self.usage(message.get("usage").unwrap_or(&Value::Null));
        if let Some(stop) = message.get("stop_reason").and_then(Value::as_str) {
            self.stop = stop.into();
            if stop == "tool_use" && !self.reply.calls.is_empty() {
                self.tool_pending = true;
            }
        } else if !self.reply.calls.is_empty() {
            // CLI 的无流回退按完整内容块发送，stop_reason 可能为 null。
            // 只在后续 MCP 回调到达且安静窗口结束后认定工具段完整。
            self.stop = "tool_use".into();
            self.tool_pending = true;
        } else if !parts.is_empty() {
            self.stop = "end_turn".into();
        }
        Ok(())
    }
}

fn index(event: &Value) -> Result<usize, GatewayError> {
    event
        .get("index")
        .and_then(Value::as_u64)
        .filter(|n| *n < 4096)
        .map(|n| n as usize)
        .ok_or_else(|| errors::protocol("Claude 内容块序号无效"))
}
