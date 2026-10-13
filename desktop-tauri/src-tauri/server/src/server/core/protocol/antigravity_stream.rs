//! 上游 **Antigravity / Gemini v1internal SSE** → 标准 chat SSE（状态机）。
//!
//! 从 `antigravity_outbound.rs` 拆出（单文件行数约定）：那个文件装请求信封
//! 转换，本文件装响应方向的状态机。接口逐字镜像
//! [`super::anthropic_outbound::ChatFromAnthropicStream`]：
//! `new(model)` / `push(&[u8]) -> Vec<Bytes>` / `finish() -> Vec<Bytes>`；
//! 壳（`upstream::translate::AntigravityToChatStream`）负责空闲守卫、
//! 调试采集与帧保序，分派在 `upstream::provider_loop`。
//!
//! ── 信封解包（规格 §4.2，最容易写错的一处）──────────────────
//! 上游每帧是 v1internal 包装：真正的 Gemini 响应在 `response` 键下，
//! **缺省回退顶层**（两套参考都是这个取值链：Manager
//! `response.get("response").unwrap_or(response)`、9router `chunk.response || chunk`）。
//!
//! ── parts 折叠（对照 9router `gemini-to-openai.js` 与 Manager
//!    `mappers/openai/streaming.rs`，两台互为佐证）────────────────
//!   - `.text` + `thought != true` → `delta.content`
//!   - `.text` + `thought == true` → `delta.reasoning_content`（本仓约定）
//!   - `functionCall{name,args,id}` → **一次给全**的 `tool_calls` 增量
//!     （本家的工具调用是整体事件，不是 Anthropic 那种「宣告帧 + 参数帧」）
//!   - `inlineData` / `inline_data`（响应侧图片）→ 本仓 chat 形态装不下
//!     （没有 images 通道），**记日志并跳过**（报告的已知限制）
//!   - 独占一帧的签名 part（无 text、无 functionCall）→ 暂存给紧随其后的
//!     functionCall（9router `pendingThoughtSignature` 同款）
//!
//! ── `finishReason` / usage / 收尾时序 ────────────────────────
//!   - 归一表见 [`map_finish_reason`]；发过工具调用且归一结果是 `stop` 时强制
//!     `tool_calls`（9router/Manager 同款修补：否则客户端把「还要继续调工具」
//!     当成对话结束）；
//!   - `usageMetadata` → chat usage（9router `USAGE_EXTRACTORS.gemini` 口径：
//!     `completion = candidatesTokenCount + thoughtsTokenCount`，
//!     `cachedContentTokenCount` → `prompt_tokens_details.cached_tokens`）；
//!     最后一帧覆盖式吸收（两套参考同款）；
//!   - 完成信号 = `finishReason` **或** `data: [DONE]`；EOF 前一个都没见到
//!     → 按截断报错（错误帧 + `[DONE]`，**不补** finish）；错误帧前不补 usage
//!     帧（带 usage 且 choices 为空的帧是收尾帧形态，会让部分客户端提前认为
//!     流已结束 —— 与 `commandcode_outbound` 逐字同形）。
//!
//! ── thoughtSignature 的带出方案（限制见报告）─────────────────
//! 两套参考都在服务端**按 call id / 会话缓存签名**、历史不带时回填缓存值或
//! 默认签名；本仓刻意不做跨请求状态缓存，OpenAI chat 又没有签名字段，因此：
//! 签名随 tool_call 增量带出，位置取 Google 自家 OpenAI 兼容端点的官方字段
//! `extra_content.google.thought_signature`；客户端下一轮原样带回时由请求侧
//! （`antigravity_outbound` 的 `call_signature`）回填到 `functionCall`。
//! 客户端不回带 → 省略（不发明默认签名）。非流式客户端走聚合器时该字段会被
//! `merge_tool_call` 丢弃（聚合形态只保留 id/type/name/arguments）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Map, Value};

use super::{chat_frame, is_truthy, json_text, random_id, string_field, string_value, SseLineBuffer};

/// 上游 `finishReason` → chat `finish_reason`（9router
/// `toOpenAIFinish(reason, "gemini")` 的归一表 + Manager 的
/// `MALFORMED_FUNCTION_CALL`→stop）。未列出的原因折 `stop`（两台参考同款）。
fn map_finish_reason(raw: &str) -> String {
    match raw.trim().to_ascii_uppercase().as_str() {
        "MAX_TOKENS" => "length".to_string(),
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" => "content_filter".to_string(),
        _ => "stop".to_string(),
    }
}

/// `usageMetadata` → chat usage（9router `USAGE_EXTRACTORS.gemini` 的口径：
/// `candidatesTokenCount` 缺失时用 `total - prompt - thoughts` 反推）。
fn usage_metadata(usage: &Value) -> Value {
    let number = |key: &str| usage.get(key).and_then(Value::as_i64).unwrap_or(0);
    let prompt = number("promptTokenCount");
    let thoughts = number("thoughtsTokenCount");
    let mut candidates = number("candidatesTokenCount");
    let total = number("totalTokenCount");
    if candidates == 0 && total > 0 {
        candidates = (total - prompt - thoughts).max(0);
    }
    let cached = number("cachedContentTokenCount");
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": candidates + thoughts,
        "total_tokens": if total > 0 { total } else { prompt + candidates + thoughts },
        "prompt_tokens_details": { "cached_tokens": cached },
    })
}

/// 上游错误状态码 → 下行 `error.type`（与 `commandcode_outbound` 的映射同形）
fn error_type(status: u16) -> &'static str {
    match status {
        400 | 422 => "invalid_request_error",
        401 | 403 => "authentication_error",
        404 => "not_found",
        429 => "rate_limit_error",
        500 | 502 => "upstream_error",
        503 => "temporarily_unavailable",
        _ => "upstream_error",
    }
}

/// 上游 Gemini SSE 字节流 → 标准 chat SSE 字节流（状态机）。
pub struct ChatFromAntigravityStream {
    buffer: SseLineBuffer,
    /// 输出帧的 model（发给上游的真名；ForwardStream 的回写层按需改写）
    model: String,
    id: String,
    created: i64,
    /// 已下发过首帧（role assistant）
    started: bool,
    finished: bool,
    /// 见过完成信号（`finishReason` 或 `data: [DONE]`）
    saw_finish: bool,
    /// 已归一成 chat 口径的结束原因
    finish_reason: Option<String>,
    /// 工具调用序号（`tool_calls[].index`，一个流内自增）
    tool_index: i64,
    /// 下发过工具调用（finish_reason 的 `tool_calls` 修补用）
    tools_emitted: bool,
    /// 独占一帧的签名 part 暂存给紧随其后的 functionCall（9router 同款）
    pending_signature: Option<String>,
    /// usage（最后一帧覆盖式吸收，与两套参考同口径）
    usage: Option<Value>,
}

impl ChatFromAntigravityStream {
    pub fn new(model: &str) -> Self {
        Self {
            buffer: SseLineBuffer::new(),
            model: model.to_string(),
            id: String::new(),
            created: crate::server::logging::now_ms() / 1000,
            started: false,
            finished: false,
            saw_finish: false,
            finish_reason: None,
            tool_index: 0,
            tools_emitted: false,
            pending_signature: None,
            usage: None,
        }
    }

    /// 吃一段上游字节，吐出要下发的 chat SSE 帧
    pub fn push(&mut self, chunk: &[u8]) -> Vec<bytes::Bytes> {
        let mut out = Vec::new();
        for payload in self.buffer.push(chunk) {
            match payload {
                // `data: [DONE]`：完成信号（收尾即刻发生，不等到 EOF）
                None => out.extend(self.done()),
                Some(data) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&data) {
                        out.extend(self.consume(&value));
                    }
                }
            }
            if self.finished {
                break;
            }
        }
        out
    }

    /// 上游流结束：冲刷残留半行，再按「有没有完成信号」收尾
    pub fn finish(&mut self) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        let mut out = Vec::new();
        for payload in self.buffer.finish() {
            match payload {
                None => out.extend(self.done()),
                Some(data) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&data) {
                        out.extend(self.consume(&value));
                    }
                }
            }
            if self.finished {
                return out;
            }
        }
        if !self.saw_finish {
            // 一个完成信号都没有：上游契约里就是「没走完」，不能补 finish_reason
            return self.fail_message(
                "上游流在收到结束原因之前结束（响应被截断）",
                "upstream_error",
                502,
                None,
            );
        }
        let reason = self
            .finish_reason
            .clone()
            .unwrap_or_else(|| "stop".to_string());
        out.extend(self.complete(&reason));
        out
    }

    /// `data: [DONE]` 到达：立即收尾（没有 finishReason 时按 stop 兜底）
    fn done(&mut self) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        self.saw_finish = true;
        let reason = self
            .finish_reason
            .clone()
            .unwrap_or_else(|| "stop".to_string());
        self.complete(&reason)
    }

    /// 一帧上游 JSON → 零到多个 chat 帧
    fn consume(&mut self, event: &Value) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        // 信封解包：真身在 `response` 键下，缺省回退顶层（规格 §4.2）
        let inner = event
            .get("response")
            .filter(|value| value.is_object())
            .unwrap_or(event);
        if let Some(error) = inner.get("error").filter(|value| is_truthy(value)) {
            return self.fail(error);
        }
        if self.id.is_empty() {
            if let Some(id) = inner
                .get("responseId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                self.id = id.to_string();
            }
        }
        if let Some(usage) = inner.get("usageMetadata").filter(|value| value.is_object()) {
            self.usage = Some(usage_metadata(usage));
        }
        let Some(candidate) = inner.pointer("/candidates/0") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        {
            for part in parts {
                out.extend(self.consume_part(part));
            }
        }
        if let Some(raw) = candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
        {
            self.saw_finish = true;
            self.finish_reason = Some(map_finish_reason(raw));
        }
        out
    }

    /// 一个 part → 零到多个 chat 帧
    fn consume_part(&mut self, part: &Value) -> Vec<bytes::Bytes> {
        let Some(object) = part.as_object() else {
            return Vec::new();
        };
        let signature = object
            .get("thoughtSignature")
            .or_else(|| object.get("thought_signature"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|signature| !signature.is_empty())
            .map(str::to_string);
        let text = object.get("text").and_then(Value::as_str).unwrap_or("");
        let is_thought = object
            .get("thought")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // 签名先记住（9router `pendingThoughtSignature` 同款：任何带签名的 part
        // 都可能是紧随其后的 functionCall 的签名来源），被下一个 functionCall 消费
        if signature.is_some() {
            self.pending_signature = signature;
        }
        if let Some(call) = object
            .get("functionCall")
            .and_then(Value::as_object)
            .filter(|call| !call.is_empty())
        {
            let signature = self.pending_signature.take();
            return self.tool_call_frames(&Value::Object(call.clone()), signature);
        }
        if text.is_empty() {
            if object
                .get("inlineData")
                .or_else(|| object.get("inline_data"))
                .is_some()
            {
                // chat 形态装不下响应侧图片（报告的已知限制）
                crate::server::logging::verbose(
                    "[Antigravity]",
                    "上游返回了 inlineData 图片，本仓 chat 形态无法承载（缺 images 通道），已跳过",
                );
            }
            return Vec::new();
        }
        let mut out = self.start();
        if is_thought {
            out.push(self.delta_frame(json!({ "reasoning_content": text })));
        } else {
            out.push(self.delta_frame(json!({ "content": text })));
        }
        out
    }

    /// `functionCall` → 一次给全的 `tool_calls` 增量（本家的工具调用是整体事件）
    fn tool_call_frames(&mut self, call: &Value, signature: Option<String>) -> Vec<bytes::Bytes> {
        let name = string_field(call, "name");
        let raw_id = string_field(call, "id").trim().to_string();
        let id = if raw_id.is_empty() {
            random_id("call")
        } else {
            raw_id
        };
        let arguments = match call.get("args") {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => "{}".to_string(),
            Some(other) => json_text(other),
        };
        let index = self.tool_index;
        self.tool_index += 1;
        self.tools_emitted = true;
        let mut tool = Map::new();
        tool.insert("index".to_string(), Value::from(index));
        tool.insert("id".to_string(), Value::String(id));
        tool.insert("type".to_string(), Value::String("function".to_string()));
        tool.insert(
            "function".to_string(),
            json!({ "name": name, "arguments": arguments }),
        );
        if let Some(signature) = signature {
            // 签名随 tool_call 增量带出（Google 自家 OpenAI 兼容端点的字段位置，
            // 见模块头）；请求侧识别同一位置并回填
            tool.insert(
                "extra_content".to_string(),
                json!({ "google": { "thought_signature": signature } }),
            );
        }
        let mut out = self.start();
        out.push(self.delta_frame(json!({ "tool_calls": [Value::Object(tool)] })));
        out
    }

    /// 首帧（role assistant），只发一次
    fn start(&mut self) -> Vec<bytes::Bytes> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        if self.id.is_empty() {
            self.id = random_id("chatcmpl");
        }
        vec![self.delta_frame(json!({ "role": "assistant", "content": "" }))]
    }

    /// 收尾：finish 帧 + usage 帧 + `[DONE]`（只做一次）。
    ///
    /// 有 finishReason（哪怕零内容，如 SAFETY 拦截）时正常收尾；一个完成信号
    /// 都没有的「空流」按空响应报错（与 `commandcode_outbound` 同一取舍：
    /// 不能把空回答记成一次成功计费）。
    fn complete(&mut self, reason: &str) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        if !self.started && self.finish_reason.is_none() {
            return self.fail_message(
                "上游返回空响应（没有任何内容帧）",
                "rate_limit_error",
                429,
                None,
            );
        }
        self.finished = true;
        // 发过工具调用时把 stop 修补成 tool_calls（9router/Manager 同款）
        let finish = match reason {
            "stop" if self.tools_emitted => "tool_calls",
            other => other,
        };
        let mut out = self.start();
        out.push(self.finish_frame(finish));
        out.push(chat_frame(&json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [],
            "usage": self.usage_payload(),
        })));
        out.push(bytes::Bytes::from_static(b"data: [DONE]\n\n"));
        out
    }

    /// 上游错误对象 → 错误帧 + `[DONE]`（只做一次）
    fn fail(&mut self, error: &Value) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        let message = {
            let text = string_field(error, "message");
            if text.trim().is_empty() {
                string_value(error)
            } else {
                text
            }
        };
        let message = if message.trim().is_empty() {
            "上游流式返回错误".to_string()
        } else {
            message
        };
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .or_else(|| {
                let text = string_field(error, "code");
                text.trim().parse::<i64>().ok()
            });
        // Google 把语义写在 `error.status` 字符串里（规格 §3.2 的分类口径），
        // 数字 code 缺失/越界时按它兜底
        let status = code
            .and_then(|code| u16::try_from(code).ok())
            .filter(|status| (100..=599).contains(status))
            .unwrap_or_else(|| {
                match string_field(error, "status").to_ascii_uppercase().as_str() {
                    "RESOURCE_EXHAUSTED" => 429,
                    "PERMISSION_DENIED" | "UNAUTHENTICATED" => 401,
                    "NOT_FOUND" => 404,
                    "UNAVAILABLE" => 503,
                    "DEADLINE_EXCEEDED" | "INTERNAL" => 502,
                    _ => 502,
                }
            });
        crate::server::logging::verbose(
            "[Antigravity]",
            &format!(
                "上游流内错误：{message}（映射为 {status} {}）",
                error_type(status)
            ),
        );
        self.fail_message(&message, error_type(status), i64::from(status), code)
    }

    /// 把「协议级失败」落成 chat 错误帧 + `[DONE]`（只做一次）。
    ///
    /// 与成功路径唯一的差别是**不补 usage 帧**（理由见模块头末条）。
    fn fail_message(
        &mut self,
        message: &str,
        kind: &str,
        status: i64,
        code: Option<i64>,
    ) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut error = json!({
            "message": message,
            "type": kind,
            // `status` 是**额外**字段：帧的形态是聚合器与 ForwardStream 认的
            // `{error:{message,type}}` 超集，多这一个键让流式客户端与抓包能
            // 直接看出上游报的是 429 还是 503
            "status": status,
        });
        if let (Some(code), Some(object)) = (code, error.as_object_mut()) {
            object.insert("code".to_string(), Value::from(code));
        }
        let mut frame = json!({ "error": error });
        if status == 429 {
            // 限流必须带 retry_after，否则 SDK 不知道该等多久（两台参考同款）
            if let Some(object) = frame.as_object_mut() {
                object.insert("retry_after".to_string(), Value::from(30));
            }
        }
        vec![
            chat_frame(&frame),
            bytes::Bytes::from_static(b"data: [DONE]\n\n"),
        ]
    }

    /// chat 口径的 usage（没有 usageMetadata 时给零值，与另外两台同口径）
    fn usage_payload(&self) -> Value {
        self.usage.clone().unwrap_or_else(|| {
            json!({
                "prompt_tokens": 0,
                "completion_tokens": 0,
                "total_tokens": 0,
            })
        })
    }

    /// 一个 delta 帧
    fn delta_frame(&self, delta: Value) -> bytes::Bytes {
        chat_frame(&json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": Value::Null }],
        }))
    }

    /// 收尾帧（空 delta + finish_reason）
    fn finish_frame(&self, finish_reason: &str) -> bytes::Bytes {
        chat_frame(&json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }],
        }))
    }
}
