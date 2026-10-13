//! 上游 **Command Code NDJSON** 字节流 → 标准 chat SSE 字节流（状态机）。
//!
//! ── 为什么需要新的一台状态机（而不是复用 Anthropic 那台）────────
//! Command Code（`api.commandcode.ai`）的 `/alpha/generate` 返回的是
//! **AI SDK v5 风格的 NDJSON**（`Content-Type: application/x-ndjson`，一行一个
//! JSON 对象，**没有 `data:` 前缀**），不是 SSE；且它 **HTTP 恒 200** ——
//! 认证失败、限流、余额不足全部以流内 `{"type":"error"}` 事件表达。
//! 「行形态」（无前缀）与「事件词表」（`text-delta` / `reasoning-delta` /
//! `tool-call` / `finish` / `finish-step` / `error`）都与 Anthropic 完全不同，
//! 因此与 [`super::anthropic_outbound::ChatFromAnthropicStream`] 并列另开一台，
//! 而不是在那台里塞分支。
//!
//! ── 事件折法（逐条对照参考实现 proxy.mjs 的 `createSseTranslator`）──
//!   - `start` / `start-step` / `text-start` / `text-end` / `reasoning-start` /
//!     `reasoning-end` / `tool-input-start` / `tool-input-delta` / `tool-input-end` /
//!     `tool-error` / `provider-metadata` → 信号，无用户可见内容，忽略
//!   - `text-delta` → `delta.content`（取 `text`，兼容 `delta` 字段）
//!   - `reasoning-delta` → `delta.reasoning_content`
//!   - `tool-call` → `tool_calls` 帧（`id` / `name` / `arguments` 一次给全，
//!     与 Anthropic 那条的「宣告帧 + 参数帧」不同：CC 的工具调用是**整体**事件）
//!   - `finish-step` → 记「见过完成信号」+ 记 finishReason/usage（**不发帧**）
//!   - `finish` → **终态**：finish 帧 + usage 帧 + `data: [DONE]`
//!   - `error` → 错误帧（+ `[DONE]`），按参考的 `CC_STATUS_MAP` 折出
//!     下行的 error.type（聚合器据此转 502；流式路径原样透给客户端）
//!
//! ── `finish` 与 [DONE] 的收尾时序（本文件的契约，逐字对齐参考）──
//!   1. 内容帧永远先于收尾帧：收尾帧一次给全 —— `finish_reason` 帧、
//!      usage 帧、`data: [DONE]\n\n`，三者之后流结束；
//!   2. **没有完成信号**（既无 `finish`、也无 `finish-step`）就 EOF：
//!      按「响应被截断」报错（可重试 502 语义），**不补** finish_reason/[DONE]
//!      —— 那等于把截断谎报成完整回答（上游契约里没有 finish 就是没走完）；
//!   3. 只见 `finish-step`、没有终态 `finish`：按「上游给过完成信号」正常收尾
//!      （参考实现 `incompleteUpstreamDetail` 的口径：真正要拦的是
//!      「一个完成信号都没有」）；
//!   4. `finish` 的 `finishReason` 归一后有 `upstream_error` 一档（裸 `error` /
//!      `network|connection|upstream-error`）：它是**可重试的连接失败**而不是
//!      合法 finish_reason，因此走错误帧而不是收尾帧（原样透出会让严格客户端
//!      整条流反序列化失败）。
//!
//! ── 与 Anthropic 那台的两处刻意差异 ───────────────────────────
//!   1. 错误帧的 `error.type` 用参考的映射表（`rate_limit_error` /
//!      `authentication_error` / …）而不是一律 `upstream_error`：NDJSON 的
//!      `error` 事件带了 `statusCode` / `code`，这一档信息在帧里能保留就保留；
//!   2. 上游**没有内容事件**（`!started`）就收尾：按「上游返回空响应」报错，
//!      与参考实现的「零输出当错误」同一取舍（防下游把空回答记成成功计费）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use super::{chat_frame, is_truthy, json_text, random_id, string_field, string_value};

/// OpenAI 侧认得的 `finish_reason` 全集（照抄参考实现的 `OPENAI_FINISH_REASONS`）。
const OPENAI_FINISH_REASONS: [&str; 5] = [
    "stop",
    "length",
    "tool_calls",
    "content_filter",
    "function_call",
];

/// 上游错误事件 → 下行 `(status, error.type)`（参考实现 `CC_STATUS_MAP` 的 Rust 形态）。
///
/// 未列出的状态码一律 502 `upstream_error` —— 与参考的兜底逐条相同。
/// 402（付费失败）映射 429、403（权限/风控）映射 401 是**有意**的下游语义
/// （让下游按限流退避 / 换账号，而不是当成服务端故障）。
fn map_cc_status(status: Option<i64>) -> (i64, &'static str) {
    match status {
        Some(400) | Some(422) => (400, "invalid_request_error"),
        Some(401) | Some(403) => (401, "authentication_error"),
        // 402 付费失败 → 按限流（充值/换套餐是用户侧的事，下游该退避）
        Some(402) => (429, "rate_limit_error"),
        Some(404) => (404, "not_found"),
        Some(429) => (429, "rate_limit_error"),
        Some(500) | Some(502) => (502, "upstream_error"),
        Some(503) => (503, "temporarily_unavailable"),
        _ => (502, "upstream_error"),
    }
}

/// 上游 finishReason → 本机内部词表（照抄参考实现的 `mapFinishReason`）。
///
/// `length` 家族**不止 `length` 一个值**：`max_output_tokens` 与
/// `model_context_window_exceeded` 都是「输出被截断」，折成 stop 等于把半截回答
/// 谎报成完整回答。裸 `error` 与 `network|connection|upstream-error` 一族并进
/// `upstream_error`（可重试的连接失败，不是合法 finish_reason）。
fn map_finish_reason(reason: &str) -> String {
    let value = reason.trim().to_lowercase();
    if value.is_empty() {
        return "stop".to_string();
    }
    match value.as_str() {
        "tool-calls" | "tool_calls" | "tool_use" => "tool_calls".to_string(),
        "length" | "max_tokens" | "max_output_tokens" | "model_context_window_exceeded" => {
            "length".to_string()
        }
        "error" => "upstream_error".to_string(),
        other => {
            if is_network_failure_finish(other) {
                "upstream_error".to_string()
            } else {
                other.to_string()
            }
        }
    }
}

/// `/^(network|connection|upstream)[-_\s]?error$/` 的 Rust 形态（不用 regex 依赖）
fn is_network_failure_finish(value: &str) -> bool {
    for prefix in ["network", "connection", "upstream"] {
        let Some(rest) = value.strip_prefix(prefix) else {
            continue;
        };
        let rest = rest.trim_start_matches(['-', '_', ' ']);
        if rest == "error" {
            return true;
        }
    }
    false
}

/// 内部词表 → 真正下发的 `finish_reason`（照抄参考的 `toOpenAIFinishReason`）。
///
/// `pause_turn`（Anthropic 原生语义「后面还有内容」）在 Chat 侧折 `length`；
/// 不认识的结束原因折 `stop`（上游确实发过 finish，只是原因不在词表 ——
/// 不能报截断）；`upstream_error` 走错误帧，不从这里出。
fn to_openai_finish_reason(reason: &str) -> String {
    if reason == "pause_turn" {
        return "length".to_string();
    }
    if OPENAI_FINISH_REASONS.contains(&reason) {
        return reason.to_string();
    }
    "stop".to_string()
}

/// NDJSON 行缓冲：上游按 TCP 分片到达，一行 JSON 完全可能被切成两半。
///
/// 与 [`super::SseLineBuffer`] 的差别只有一处：NDJSON **没有 `data:` 前缀**，
/// 整行就是 JSON；因此这里不做前缀剥离，只按 `\n` 切分并在 EOF 时冲刷残留
/// （上游发完最后一帧却没带收尾换行是真实存在的形态，冲刷能让它被正确识别）。
#[derive(Default)]
struct NdjsonLineBuffer {
    tail: Vec<u8>,
}

impl NdjsonLineBuffer {
    fn new() -> Self {
        Self { tail: Vec::new() }
    }

    /// 吃一段字节，吐出其中**完整**的行（已按参考口径过滤空行 / `[DONE]` / 注释行）
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        self.tail.extend_from_slice(chunk);
        let mut start = 0usize;
        while let Some(offset) = self.tail[start..].iter().position(|byte| *byte == b'\n') {
            let end = start + offset;
            let line = String::from_utf8_lossy(&self.tail[start..end]).to_string();
            if let Some(line) = keep_line(&line) {
                out.push(line);
            }
            start = end + 1;
        }
        self.tail.drain(..start);
        out
    }

    /// 流结束：把残留的半行也当一行处理（判据与 [`Self::push`] 相同）
    fn finish(&mut self) -> Vec<String> {
        if self.tail.is_empty() {
            return Vec::new();
        }
        let line = String::from_utf8_lossy(&self.tail).to_string();
        self.tail.clear();
        keep_line(&line).into_iter().collect()
    }
}

/// 一行是否要交给解析器（空行 / `[DONE]` / `:` 注释行都不是事件）
fn keep_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" || trimmed.starts_with(':') {
        return None;
    }
    Some(trimmed.to_string())
}

/// 上游 NDJSON 字节流 → 标准 chat SSE 字节流（状态机）。
pub struct ChatFromCommandCodeStream {
    buffer: NdjsonLineBuffer,
    /// 输出帧的 model（发给上游的真名；ForwardStream 的回写层按需改写）
    model: String,
    id: String,
    created: i64,
    /// 已下发过首帧（role assistant）——同时也表达「上游给过内容事件」
    started: bool,
    finished: bool,
    /// 见过任何完成信号（终态 `finish` 或 `finish-step`）
    saw_finish: bool,
    /// `finish-step` 带来的结束原因（规范化后；`finish` 缺席时用它收尾）
    step_finish_reason: Option<String>,
    /// 工具调用的序号（`tool_calls[].index`，一个流内自增）
    tool_index: i64,
    /// usage（`finish-step` 的 `usage` → `finish` 的 `totalUsage` 覆盖）
    usage: Option<Value>,
}

impl ChatFromCommandCodeStream {
    pub fn new(model: &str) -> Self {
        Self {
            buffer: NdjsonLineBuffer::new(),
            model: model.to_string(),
            id: String::new(),
            created: crate::server::logging::now_ms() / 1000,
            started: false,
            finished: false,
            saw_finish: false,
            step_finish_reason: None,
            tool_index: 0,
            usage: None,
        }
    }

    /// 吃一段上游字节，吐出要下发的 chat SSE 帧
    pub fn push(&mut self, chunk: &[u8]) -> Vec<bytes::Bytes> {
        let mut out = Vec::new();
        for line in self.buffer.push(chunk) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                out.extend(self.consume(&value));
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
        for line in self.buffer.finish() {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                out.extend(self.consume(&value));
            }
            if self.finished {
                return out;
            }
        }
        if !self.saw_finish {
            // 一个完成信号都没有：上游契约里就是「没走完」，不能补 finish_reason
            out.extend(self.fail_message(
                "上游流在收到完成事件之前结束（响应被截断）",
                "upstream_error",
                502,
                None,
            ));
            return out;
        }
        let reason = self
            .step_finish_reason
            .clone()
            .unwrap_or_else(|| "stop".to_string());
        out.extend(self.complete(&reason));
        out
    }

    /// 一行 NDJSON 事件 → 零到多个 chat 帧
    fn consume(&mut self, event: &Value) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        let Some(object) = event.as_object() else {
            return Vec::new();
        };
        let kind = string_field(event, "type").to_lowercase();
        match kind.as_str() {
            // 信号类事件：无用户可见内容（与参考实现的静默列表逐字对齐）
            "start" | "start-step" | "text-start" | "text-end" | "reasoning-start"
            | "reasoning-end" | "tool-input-start" | "tool-input-delta" | "tool-input-end"
            | "tool-error" | "provider-metadata" => Vec::new(),
            "text-delta" => {
                // 正文增量：`text` 为主、兼容部分实现的 `delta`
                let text = {
                    let primary = string_field(event, "text");
                    if primary.is_empty() {
                        string_field(event, "delta")
                    } else {
                        primary
                    }
                };
                if text.is_empty() {
                    return Vec::new();
                }
                let mut out = self.start();
                out.push(self.delta_frame(json!({ "content": text })));
                out
            }
            "reasoning-delta" => {
                // 思考增量：只取 `text`（与参考一致，不读 `delta`）
                let text = string_field(event, "text");
                if text.is_empty() {
                    return Vec::new();
                }
                let mut out = self.start();
                out.push(self.delta_frame(json!({ "reasoning_content": text })));
                out
            }
            "tool-call" => {
                // 工具调用是**整体**事件（id / name / 完整参数一次给全）
                let raw_id = string_field(event, "toolCallId");
                let id = if raw_id.is_empty() {
                    random_id("call")
                } else {
                    raw_id
                };
                let name = string_field(event, "toolName");
                let input = object.get("input").cloned().unwrap_or(Value::Null);
                let arguments = match &input {
                    Value::String(text) => text.clone(),
                    Value::Null => "{}".to_string(),
                    other => json_text(other),
                };
                let index = self.tool_index;
                self.tool_index += 1;
                let mut out = self.start();
                out.push(self.delta_frame(json!({
                    "tool_calls": [{
                        "index": index,
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": arguments },
                    }],
                })));
                out
            }
            "finish-step" => {
                // 单步结束：只算「见过完成信号」，帧由 finish 或缺席时的 EOF 出
                self.saw_finish = true;
                if let Some(reason) = object
                    .get("finishReason")
                    .filter(|value| is_truthy(value))
                    .map(string_value)
                    .filter(|value| !value.trim().is_empty())
                {
                    self.step_finish_reason = Some(map_finish_reason(&reason));
                }
                if let Some(usage) = object.get("usage").filter(|usage| usage.is_object()) {
                    self.usage = Some(normalized_usage(usage));
                }
                Vec::new()
            }
            "finish" => {
                self.saw_finish = true;
                // 参考的取值链：finish-step 的 finishReason 优先，其次 finish 自己的
                let reason = match self.step_finish_reason.clone() {
                    Some(reason) => reason,
                    None => {
                        let raw = string_field(event, "finishReason");
                        if raw.trim().is_empty() {
                            "stop".to_string()
                        } else {
                            map_finish_reason(&raw)
                        }
                    }
                };
                if let Some(usage) = object
                    .get("totalUsage")
                    .or_else(|| object.get("usage"))
                    .filter(|usage| usage.is_object())
                {
                    self.usage = Some(normalized_usage(usage));
                }
                // `upstream_error` 一档（裸 `error` / `network|connection|upstream-error`）
                // 由 `complete` 统一转成可重试的错误帧 —— 它不是合法 finish_reason
                self.complete(&reason)
            }
            "error" => self.fail(event),
            other => {
                // 未知事件：忽略但不静默 —— 事件词表若发生漂移，日志是唯一线索
                crate::server::logging::verbose(
                    "[CommandCode]",
                    &format!("上游发了未知的 NDJSON 事件类型：{other}"),
                );
                Vec::new()
            }
        }
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
    /// `reason` 是**归一后的内部词表**取值（`map_finish_reason` 的输出）：
    /// `upstream_error` 在这里转成可重试的错误帧（它不是合法 finish_reason）；
    /// 其余经 `to_openai_finish_reason` 折成 OpenAI 认的值。
    ///
    /// 上游一个内容事件都没给就走到这里（只有 finish / finish-step）时按空响应
    /// 报错：那是一条「成功但什么都没有」的流，记成成功会让下游按空回答计费。
    fn complete(&mut self, reason: &str) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        // 顺序是刻意的（与参考实现同款）：**先判连接失败族，再判零输出** ——
        // provider 报连接失败时「零输出」只是表象，按 502 报才指向真实原因
        if reason == "upstream_error" {
            return self.fail_message(
                "上游报告连接失败（provider reported an upstream connection failure）",
                "upstream_error",
                502,
                None,
            );
        }
        if !self.started {
            return self.fail_message(
                "上游返回空响应（没有任何内容事件）",
                "rate_limit_error",
                429,
                None,
            );
        }
        self.finished = true;
        let mut out = self.start();
        out.push(self.finish_frame(&to_openai_finish_reason(reason)));
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

    /// 上游错误事件 → 错误帧 + `[DONE]`（只做一次）
    ///
    /// 状态码取值链（参考实现 `mapCcEventError`）：message 里的 `<NNN>` 前缀
    /// 优先，其次 `error.statusCode`，都没有回落 502 —— 拿不到真实状态码会让
    /// 429/503 全塌成 502，下游不再退避、监控误判成后端故障。
    fn fail(&mut self, event: &Value) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        let error = event.get("error").filter(|value| is_truthy(value));
        // 文案取值链与参考实现同款：`error.message` → `event.message` → 兜底文案
        let message = match error {
            Some(error) => {
                let text = string_field(error, "message");
                if text.trim().is_empty() {
                    string_field(event, "message")
                } else {
                    text
                }
            }
            None => string_field(event, "message"),
        };
        let message = if message.trim().is_empty() {
            "上游流式返回错误".to_string()
        } else {
            message
        };
        // 状态码取值链（参考 `mapCcEventError`）：message 里的 `<NNN>` 前缀
        // 优先，其次 `error.statusCode`，都没有回落 502
        let reported = error.and_then(|error| {
            embedded_status(&message).or_else(|| error.get("statusCode").and_then(Value::as_i64))
        });
        // 机器可读分类：`error.code` → `event.code`
        let code = error
            .and_then(|error| {
                error
                    .get("code")
                    .filter(|value| is_truthy(value))
                    .or_else(|| event.get("code").filter(|value| is_truthy(value)))
            })
            .map(string_value)
            .filter(|value| !value.is_empty());
        let (status, kind) = map_cc_status(reported);
        crate::server::logging::verbose(
            "[CommandCode]",
            &format!(
                "上游流内错误：{message}（上游状态 {}，映射为 {status} {kind}）",
                reported
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string())
            ),
        );
        self.fail_message(&message, kind, status, code.as_deref())
    }

    /// 把「协议级失败」落成 chat 错误帧 + `[DONE]`（只做一次）
    fn fail_message(
        &mut self,
        message: &str,
        kind: &str,
        status: i64,
        code: Option<&str>,
    ) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        // ── 为什么错误帧前**不**补 usage 帧（与成功路径的唯一差别）──────
        // 带 `usage` 且 `choices` 为空的帧正是 OpenAI 流式的**收尾帧形态**，
        // 部分客户端据此判定「流已结束」—— 在它之后补错误帧等于让那些客户端
        // 永远看不到失败，把一次错误渲染成一次空回答。错误路径只发
        // 「错误帧 + [DONE]」（与 `ForwardStream` 的断流收尾、Anthropic 那台
        // 的 `fail_message` 逐字同形）。
        let mut error = json!({
            "message": message,
            "type": kind,
            // `status` 是**额外**字段：帧的形态是聚合器与 ForwardStream 认的
            // `{error:{message,type}}` 超集，多这一个键让流式客户端与抓包能直接
            // 看出上游报的是 429 还是 503（下游 HTTP 状态在流式路径上已定稿成 200）
            "status": status,
        });
        if let Some(code) = code {
            if let Some(object) = error.as_object_mut() {
                object.insert("code".to_string(), Value::String(code.to_string()));
            }
        }
        let mut frame = json!({ "error": error });
        if status == 429 {
            // 限流必须带 retry_after，否则 SDK 不知道该等多久（参考实现同款）
            if let Some(object) = frame.as_object_mut() {
                object.insert("retry_after".to_string(), Value::from(30));
            }
        }
        let mut out = vec![chat_frame(&frame)];
        out.push(bytes::Bytes::from_static(b"data: [DONE]\n\n"));
        out
    }

    /// chat 口径的 usage（含「outputTokens 为 0 时清零 input/cached」的防误计费规则）
    fn usage_payload(&self) -> Value {
        match self.usage.clone() {
            Some(usage) => usage,
            None => json!({
                "prompt_tokens": 0,
                "completion_tokens": 0,
                "total_tokens": 0,
            }),
        }
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

/// message 里的 `<NNN>` 前缀（参考的 `parseEmbeddedErrorJSON(message)?.status`）
fn embedded_status(message: &str) -> Option<i64> {
    let rest = message.trim().strip_prefix('<')?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() != 3 {
        return None;
    }
    digits.parse::<i64>().ok()
}

/// 上游 usage → chat 口径（参考 `normalizeUsage` + usage 帧的字段映射）。
///
/// `inputTokens` 是**总数（含缓存）**，chat 的 `prompt_tokens` 口径本来就含缓存，
/// 因此它可以直接沿用；缓存另开 `prompt_tokens_details.cached_tokens`
/// （请求统计的「缓存」列按它取值）。
/// `outputTokens === 0` 时清零 input/cached：上游异常时的 0 输出不该产生
/// 一笔输入计费（参考实现同款防误计费规则）。
fn normalized_usage(usage: &Value) -> Value {
    let input = number_of(usage, &["inputTokens"]);
    let mut output = number_of(usage, &["outputTokens", "completionTokens"]);
    let mut cached = number_of(usage, &["cachedInputTokens", "cacheReadTokens"]);
    let mut input = input;
    if output == 0 {
        input = 0;
        cached = 0;
    }
    if output < 0 {
        output = 0;
        input = 0;
        cached = 0;
    }
    json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": input + output,
        "prompt_tokens_details": { "cached_tokens": cached },
    })
}

/// 从对象里按候选键读一个整数（缺失 / 非数字给 0）
fn number_of(value: &Value, keys: &[&str]) -> i64 {
    for key in keys {
        if let Some(number) = value.get(*key).and_then(Value::as_i64) {
            return number;
        }
        if let Some(number) = value
            .get(*key)
            .and_then(Value::as_f64)
            .filter(|number| number.is_finite())
        {
            return number as i64;
        }
    }
    0
}
