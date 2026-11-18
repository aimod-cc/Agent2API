//! Responses SSE → Chat SSE 的状态机。请求转换仍在父模块，公共导出路径不变。
//! new 保留兼容上游的 EOF 收尾；new_strict 要求明确终态，异常流不能变成成功回答。
//! 工具 item_id / call_id / output_index 是同一槽位的别名，不是三次独立工具调用。

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{json, Map, Value};

use super::super::{
    chat_frame, freeform, is_truthy, json_text, random_id, string_field, string_value,
    SseLineBuffer, FIELD_ENCRYPTED_CONTENT,
};

/// 上游 Responses SSE 字节流 → 标准 Chat SSE 字节流。
/// 输出继续使用 choices[].delta、finish_reason、独立 usage 帧与 [DONE]。
pub struct ChatFromResponsesStream {
    buffer: SseLineBuffer,
    model: String,
    id: String,
    created: i64,
    started: bool,
    finished: bool,
    strict: bool,
    tools: Vec<ToolSlot>,
    tool_aliases: BTreeMap<ToolKey, usize>,
    usage: Option<Value>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ToolKey {
    Item(String),
    Call(String),
    Output(u64),
    Anonymous,
}

#[derive(Default)]
struct ToolSlot {
    call_id: String,
    item_id: String,
    name: String,
    declared: bool,
    has_arguments: bool,
    arguments_done: bool,
    // 未收到工具名称时先保存参数；名称一到就宣告一次并冲刷，不发空名称工具。
    pending_arguments: String,
}

impl ChatFromResponsesStream {
    /// 兼容模式：坏 JSON 仍跳过、没有显式终态的 EOF 仍按历史行为收尾。
    pub fn new(model: &str) -> Self {
        Self {
            buffer: SseLineBuffer::new(),
            model: model.to_string(),
            id: String::new(),
            created: crate::server::logging::now_ms() / 1000,
            started: false,
            finished: false,
            strict: false,
            tools: Vec::new(),
            tool_aliases: BTreeMap::new(),
            usage: None,
        }
    }

    /// 严格模式：内置订阅必须提供正常完成或已知截断的终态，不能靠 EOF 猜成功。
    pub fn new_strict(model: &str) -> Self {
        Self {
            strict: true,
            ..Self::new(model)
        }
    }

    /// 正常终态或错误终态都返回 true；包装层据此及时关闭仍未断开的上游。
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        let payloads = self.buffer.push(chunk);
        self.consume_payloads(payloads)
    }

    /// 冲刷最后一行；严格模式在没有明确终态时输出错误，而不是 stop/tool_calls。
    pub fn finish(&mut self) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        let payloads = self.buffer.finish();
        let mut out = self.consume_payloads(payloads);
        if !self.finished {
            out.extend(self.end_stream());
        }
        out
    }

    fn consume_payloads(&mut self, payloads: Vec<Option<String>>) -> Vec<Bytes> {
        let mut out = Vec::new();
        for payload in payloads {
            if self.finished {
                break;
            }
            match payload {
                // [DONE] 只表示传输结束，不能代替 Responses 的完成事件。
                None => out.extend(self.end_stream()),
                Some(data) => match serde_json::from_str::<Value>(&data) {
                    Ok(value) if !self.strict || value.is_object() => {
                        out.extend(self.consume(&value))
                    }
                    Ok(_) | Err(_) if self.strict => {
                        out.extend(self.fail_message("上游 Responses 流包含无效 JSON 事件"))
                    }
                    _ => {}
                },
            }
        }
        out
    }

    fn end_stream(&mut self) -> Vec<Bytes> {
        if self.strict {
            self.fail_message("上游 Responses 流在完成事件到达前结束，回复可能不完整")
        } else {
            self.complete(None)
        }
    }

    fn consume(&mut self, event: &Value) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        if let Some(usage) = event
            .get("usage")
            .filter(|usage| usage.is_object())
            .or_else(|| {
                event
                    .pointer("/response/usage")
                    .filter(|usage| usage.is_object())
            })
        {
            self.usage = Some(usage.clone());
        }
        let kind = string_field(event, "type").to_lowercase();
        if self.strict
            && event
                .get("type")
                .and_then(Value::as_str)
                .is_none_or(|kind| kind.is_empty())
        {
            return self.fail_message("上游 Responses 事件缺少类型，无法确认响应状态");
        }
        match kind.as_str() {
            "response.created" => {
                if let Some(id) = event
                    .pointer("/response/id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    self.id = id.to_string();
                }
                self.start()
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                self.text_delta(event, "content")
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                self.text_delta(event, "reasoning_content")
            }
            "response.output_item.added" => {
                let item = event.get("item").unwrap_or(&Value::Null);
                if matches!(
                    string_field(item, "type").to_lowercase().as_str(),
                    "function_call" | "custom_tool_call"
                ) {
                    self.tool_event(event, item, None)
                } else {
                    Vec::new()
                }
            }
            "response.function_call_arguments.delta" => {
                let delta = string_field(event, "delta");
                if delta.is_empty() {
                    return Vec::new();
                }
                self.tool_event(event, event, Some((delta, false)))
            }
            "response.custom_tool_call_input.delta" => {
                // 自由文本增量沿用原逻辑；完整 JSON 包壳只用于从未收到增量的 done。
                let delta = json_text(
                    event
                        .get("delta")
                        .or_else(|| event.get("input"))
                        .unwrap_or(&Value::Null),
                );
                if delta.is_empty() {
                    return Vec::new();
                }
                self.tool_event(event, event, Some((delta, false)))
            }
            "response.function_call_arguments.done" => {
                self.tool_event(event, event, Some((function_arguments(event), true)))
            }
            "response.custom_tool_call_input.done" => {
                let arguments = freeform::wrap_freeform_input(&json_text(
                    event.get("input").unwrap_or(&Value::Null),
                ));
                self.tool_event(event, event, Some((arguments, true)))
            }
            "response.output_item.done" => self.item_done(event),
            "error" | "response.failed" => self.fail(event),
            "response.completed" | "response.done" => self.terminal(event),
            "response.incomplete" => self.incomplete(event),
            _ => Vec::new(),
        }
    }

    fn text_delta(&mut self, event: &Value, field: &str) -> Vec<Bytes> {
        let Some(text) = event
            .get("delta")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        else {
            return Vec::new();
        };
        let mut out = self.start();
        let mut delta = Map::new();
        delta.insert(field.to_string(), Value::String(text.to_string()));
        out.push(self.delta_frame(Value::Object(delta)));
        out
    }

    fn item_done(&mut self, event: &Value) -> Vec<Bytes> {
        let item = event.get("item").unwrap_or(&Value::Null);
        match string_field(item, "type").to_lowercase().as_str() {
            "reasoning" => {
                let Some(encrypted) = item
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                else {
                    return Vec::new();
                };
                let mut out = self.start();
                let mut delta = Map::new();
                // 只传回上游真实密文，不从摘要、item id 或其他提供商签名构造替代物。
                delta.insert(
                    FIELD_ENCRYPTED_CONTENT.to_string(),
                    Value::String(encrypted.to_string()),
                );
                out.push(self.delta_frame(Value::Object(delta)));
                out
            }
            "function_call" => self.tool_event(event, item, Some((function_arguments(item), true))),
            "custom_tool_call" => {
                let arguments = freeform::wrap_freeform_input(&json_text(
                    item.get("input").unwrap_or(&Value::Null),
                ));
                self.tool_event(event, item, Some((arguments, true)))
            }
            _ => Vec::new(),
        }
    }

    fn terminal(&mut self, event: &Value) -> Vec<Bytes> {
        if self.strict {
            // response.done 是兼容终态别名，但其中的 failed/incomplete 不能被当 completed。
            // 与参考 OmniRoute resolveResponsesDoneEventType 同一处置。
            if event
                .get("response")
                .is_some_and(|response| !response.is_object())
            {
                return self.fail_message("上游 Responses 终态载荷无效");
            }
            if event.pointer("/response/error").is_some_and(is_truthy)
                || event.get("error").is_some_and(is_truthy)
            {
                return self.fail(event);
            }
            match event.pointer("/response/status") {
                Some(Value::String(status))
                    if matches!(status.as_str(), "failed" | "cancelled" | "canceled") =>
                {
                    return self.fail(event)
                }
                Some(Value::String(status)) if status == "incomplete" => {
                    return self.incomplete(event)
                }
                None => {}
                Some(Value::String(status)) if status == "completed" => {}
                Some(_) => return self.fail_message("上游 Responses 终态没有确认回复已完成"),
            }
        }
        self.complete(None)
    }

    fn incomplete(&mut self, event: &Value) -> Vec<Bytes> {
        let reason = event
            .pointer("/response/incomplete_details/reason")
            .or_else(|| event.pointer("/incomplete_details/reason"))
            .map(string_value)
            .unwrap_or_default();
        match reason.as_str() {
            "max_output_tokens" => self.complete(Some("length")),
            "content_filter" => self.complete(Some("content_filter")),
            _ if self.strict => self.fail_message("上游 Responses 未完成，未提供可识别的截断原因"),
            // 老的宽松出口保留非标准 reason 与缺省 length 的兼容语义。
            "" => self.complete(Some("length")),
            _ => self.complete(Some(&reason)),
        }
    }

    fn start(&mut self) -> Vec<Bytes> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        if self.id.is_empty() {
            self.id = random_id("chatcmpl");
        }
        vec![self.delta_frame(json!({ "role": "assistant", "content": "" }))]
    }

    fn tool_event(
        &mut self,
        event: &Value,
        item: &Value,
        arguments: Option<(String, bool)>,
    ) -> Vec<Bytes> {
        let index = match self.bind_tool(event, item) {
            Ok(index) => index,
            Err(message) => return self.fail_message(message),
        };
        let mut out = self.start();
        out.extend(self.declare_tool(index, false));
        if let Some((arguments, snapshot)) = arguments {
            out.extend(self.push_arguments(index, &arguments, snapshot));
        }
        out
    }

    /// 优先用 item.id/event.item_id 查槽；call_id/output_index 只给它补别名。
    /// 不按“最近的工具”猜并行参数归属，也不把三种键的字面值混在一个命名空间。
    fn bind_tool(&mut self, event: &Value, item: &Value) -> Result<usize, &'static str> {
        let mut keys = tool_keys(event, item);
        if keys.is_empty() {
            if self.tools.len() == 1 {
                keys.push(ToolKey::Anonymous);
            } else if self.strict {
                return Err("上游工具事件缺少可配对的调用标识");
            } else {
                keys.push(ToolKey::Anonymous);
            }
        }
        let mut found = None;
        for key in &keys {
            if let Some(index) = self.tool_aliases.get(key).copied() {
                if found.is_some_and(|found| found != index) {
                    return Err("上游工具事件的标识相互冲突，已停止输出以免工具参数错配");
                }
                found = Some(index);
            }
        }
        let index = found.unwrap_or_else(|| {
            if keys == [ToolKey::Anonymous] && self.tools.len() == 1 {
                return 0;
            }
            let index = self.tools.len();
            self.tools.push(ToolSlot::default());
            index
        });
        for key in keys {
            self.tool_aliases.insert(key, index);
        }
        let Some(slot) = self.tools.get_mut(index) else {
            return Err("上游工具槽位状态异常");
        };
        if !slot.declared {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .or_else(|| event.get("call_id").and_then(Value::as_str))
                .unwrap_or("");
            if !call_id.is_empty() {
                slot.call_id = call_id.to_string();
            }
            let item_id = event
                .get("item")
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .or_else(|| event.get("item_id").and_then(Value::as_str))
                .unwrap_or("");
            if !item_id.is_empty() {
                slot.item_id = item_id.to_string();
            }
            let name = string_field(item, "name");
            if !name.is_empty() {
                slot.name = name;
            }
        }
        Ok(index)
    }

    /// 名称首次就绪才输出；后续 added/done 不重复输出 name（Chat 消费方会拼接）。
    fn declare_tool(&mut self, index: usize, legacy_fallback: bool) -> Vec<Bytes> {
        let Some(slot) = self.tools.get_mut(index) else {
            return Vec::new();
        };
        if slot.declared || slot.name.is_empty() && !legacy_fallback {
            return Vec::new();
        }
        slot.declared = true;
        let call_id = if !slot.call_id.is_empty() {
            slot.call_id.clone()
        } else if !slot.item_id.is_empty() {
            slot.item_id.clone()
        } else {
            random_id("call")
        };
        let name = slot.name.clone();
        let pending = std::mem::take(&mut slot.pending_arguments);
        let mut out = vec![self.delta_frame(json!({
            "tool_calls": [{ "index": index, "id": call_id, "type": "function",
                "function": { "name": name, "arguments": "" } }],
        }))];
        if !pending.is_empty() {
            out.push(self.arguments_frame(index, &pending));
        }
        out
    }

    fn push_arguments(&mut self, index: usize, delta: &str, snapshot: bool) -> Vec<Bytes> {
        let Some(slot) = self.tools.get_mut(index) else {
            return Vec::new();
        };
        if slot.arguments_done || delta.is_empty() {
            return Vec::new();
        }
        if snapshot && slot.has_arguments {
            slot.arguments_done = true;
            return Vec::new();
        }
        slot.has_arguments = true;
        slot.arguments_done = snapshot;
        if !slot.declared {
            if slot.pending_arguments.len().saturating_add(delta.len()) > 8 * 1024 * 1024 {
                return self.fail_message("上游工具参数在缺少名称时超过缓冲上限");
            }
            slot.pending_arguments.push_str(delta);
            return Vec::new();
        }
        vec![self.arguments_frame(index, delta)]
    }

    fn complete(&mut self, incomplete_reason: Option<&str>) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        if self.strict
            && incomplete_reason.is_none()
            && self.tools.iter().any(|slot| !slot.declared)
        {
            return self
                .fail_message("上游 Responses 工具调用缺少名称，未将不完整工具调用作为成功结果");
        }
        let mut out = self.start();
        if !self.strict {
            // 缺声明的旧上游仍可在 EOF 用原有空名称兜底；严格入口绝不这样做。
            for index in 0..self.tools.len() {
                out.extend(self.declare_tool(index, true));
            }
        }
        self.finished = true;
        self.buffer = SseLineBuffer::new();
        let finish_reason = incomplete_reason.unwrap_or(if self.tools.is_empty() {
            "stop"
        } else {
            "tool_calls"
        });
        out.push(self.finish_frame(finish_reason));
        if let Some(mut usage) = self.usage.take() {
            super::super::usage::normalize_chat(&mut usage);
            out.push(chat_frame(&json!({
                "id": self.id, "object": "chat.completion.chunk", "created": self.created,
                "model": self.model, "choices": [], "usage": usage,
            })));
        }
        out.push(Bytes::from_static(b"data: [DONE]\n\n"));
        out
    }

    fn fail(&mut self, event: &Value) -> Vec<Bytes> {
        let error = event
            .get("error")
            .filter(|error| is_truthy(error))
            .or_else(|| {
                event
                    .pointer("/response/error")
                    .filter(|error| is_truthy(error))
            });
        let message = match error {
            Some(error) => {
                let text = string_field(error, "message");
                if text.is_empty() {
                    string_value(error)
                } else {
                    text
                }
            }
            None if self.strict => event
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("上游 Responses 返回失败或取消终态")
                .to_string(),
            None => string_value(event),
        };
        self.fail_message(if message.trim().is_empty() {
            "上游流式返回错误"
        } else {
            &message
        })
    }

    fn fail_message(&mut self, message: &str) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        self.buffer = SseLineBuffer::new();
        self.tools.clear();
        self.tool_aliases.clear();
        self.usage = None;
        vec![
            chat_frame(&json!({ "error": { "message": message, "type": "upstream_error" } })),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ]
    }

    fn delta_frame(&self, delta: Value) -> Bytes {
        chat_frame(&json!({
            "id": self.id, "object": "chat.completion.chunk", "created": self.created,
            "model": self.model, "choices": [{ "index": 0, "delta": delta, "finish_reason": Value::Null }],
        }))
    }

    fn arguments_frame(&self, index: usize, arguments: &str) -> Bytes {
        self.delta_frame(
            json!({ "tool_calls": [{ "index": index, "function": { "arguments": arguments } }] }),
        )
    }

    fn finish_frame(&self, finish_reason: &str) -> Bytes {
        chat_frame(&json!({
            "id": self.id, "object": "chat.completion.chunk", "created": self.created,
            "model": self.model, "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }],
        }))
    }
}

fn function_arguments(item: &Value) -> String {
    let raw = json_text(item.get("arguments").unwrap_or(&Value::Null));
    if raw.is_empty() {
        "{}".to_string()
    } else {
        raw
    }
}

/// added/done 的 item.id 与 delta 的 event.item_id 共用 Item 命名空间。
/// 每一帧出现的全部身份都绑定同一槽，因此仅 call_id/output_index 的变体也能续接。
fn tool_keys(event: &Value, item: &Value) -> Vec<ToolKey> {
    let mut keys = Vec::new();
    for value in [
        event.get("item").and_then(|item| item.get("id")),
        event.get("item_id"),
    ] {
        if let Some(id) = value.and_then(Value::as_str).filter(|id| !id.is_empty()) {
            keys.push(ToolKey::Item(id.to_string()));
        }
    }
    for value in [item.get("call_id"), event.get("call_id")] {
        if let Some(id) = value.and_then(Value::as_str).filter(|id| !id.is_empty()) {
            let key = ToolKey::Call(id.to_string());
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    for value in [event.get("output_index"), item.get("output_index")] {
        if let Some(index) =
            value.and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        {
            let key = ToolKey::Output(index);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}
