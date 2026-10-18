//! 上游智谱清言 SSE（`parts` 逐帧文本）→ 标准 chat SSE 增量帧（状态机）。
//!
//! ── 为什么需要新的一台状态机（而不是复用任何一台）──────────────
//! 本家的流是 SSE（`data: {...}` 逐帧），但**帧语义与 OpenAI 完全不同**：
//!
//! ```text
//!   智谱清言：每帧给 `parts:[{id, logic_id, status, content:[{type,text|think}]}]`
//!   OpenAI ：每帧给「这一拍的增量 delta」
//!     {choices:[{delta:{content:"新增的几个字"}, index:0}]}
//! ```
//!
//! ── 帧语义（live 抓帧实测，2026-10）─────────────────────────
//! 同一轮回答里混着**两种**帧，靠 `status` 区分，**不能一律按快照差分**
//! （那正是「AI 回答两遍」的成因，见 `advance` 的注释）：
//!
//! ```text
//!   [1] top=init   part=init    text="1"            ← 增量帧：本帧就是新增内容
//!   [2] top=init   part=init    text="\n"           ← 增量帧
//!   [3] top=init   part=init    text="2\n3"         ← 增量帧
//!   [4] top=init   part=finish  text="1\n2\n3"      ← 全文快照帧（注意 part 先 finish）
//!   [5] top=finish part=finish  text="1\n2\n3"      ← 全文快照帧 + 终态
//! ```
//!
//! 因此规则是：**`part.status == "init"` 的帧原样下发并追加进基准；
//! 出现 finish 的帧（顶层或 part）给的是整段全文，只补基准之后的后缀。**
//! 长回答实测：23 个 init 帧拼接 = 157 字，与收尾快照的 157 字逐字相同。
//!
//! ── 分段（`logic_id`）与占位段 ──────────────────────────────
//! 一轮回答是**分段**的，每段一个 `logic_id`（实测四段：占位段 → 表单段 →
//! 思考段 → 正文段）。换段即换基准：新段首帧若被拿去和旧段比对，会被误判成
//! 「对不上」而整段重发。另有「占位段」（`meta_data.engine.action == "loading"`，
//! 内容是「正在梳理您的需求～」这句状态文案）—— 官方客户端会把它替换掉，
//! 我们一旦下发就收不回来，所以整段跳过（见 `is_loading_placeholder`）。
//!
//! 一帧可能带多个 part：本实现按**整帧文本**处理（所有 `text` 块按顺序拼成一条），
//! 因为下游 chat 协议只有一条正文流。`think` 块单独走 `reasoning_content` 通道，
//! 与正文各自维护基准。
//!
//! ── 收尾判定 ────────────────────────────────────────────────
//! `status == "finish"`（顶层）为终态：发 `finish_reason` 帧 + usage 帧 +
//! `data: [DONE]`。上游**不发 `[DONE]`**（测过完整流），所以由这里补。
//! 一个内容帧都没给就结束 → 按空响应报错（不把空回答记成成功）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use super::{chat_frame, content_parts, json_number_of, random_id, string_field, SseLineBuffer};

/// 上游每帧的 `parts[*].content[*]` 里，哪些 `type` 算正文 / 思考
const KIND_TEXT: &str = "text";
const KIND_THINK: &str = "think";

/// 上游 SSE 字节流 → 标准 chat SSE 增量帧（状态机）。
pub struct ChatFromChatglmStream {
    buffer: SseLineBuffer,
    /// 输出帧的 model（发给上游的真名；ForwardStream 的回写层按需改写）
    model: String,
    id: String,
    created: i64,
    /// 已下发过首帧（role assistant）
    started: bool,
    finished: bool,
    /// 当前内容段的 key（上游 `logic_id`，缺失时退回落 `part.id`）。
    ///
    /// 上游一轮回答是**分段**的（实测：`占位段 → 表单段 → 思考段 → 正文段`，
    /// 每段一个 `logic_id`）。换段意味着上一段结束、新的一段开始，
    /// 差分基准必须跟着清零，否则新段的首帧会被拿去和旧段比对。
    segment_key: String,
    /// **当前段**已下发的正文全文（差分基准）
    segment_text: String,
    /// **当前段**已下发的思考全文（差分基准）
    segment_thinking: String,
    /// 上游最后一次给出的会话 id（收尾帧里回填，便于排查）
    conversation_id: String,
    /// 上游最后一次给出的业务错误（若有）
    upstream_error: Option<String>,
    /// usage（上游不给 token 计数，恒为空 —— 见 `usage_payload`）
    usage: Option<Value>,
}

impl ChatFromChatglmStream {
    pub fn new(model: &str) -> Self {
        Self {
            buffer: SseLineBuffer::new(),
            model: model.to_string(),
            id: String::new(),
            created: crate::server::logging::now_ms() / 1000,
            started: false,
            finished: false,
            segment_key: String::new(),
            segment_text: String::new(),
            segment_thinking: String::new(),
            conversation_id: String::new(),
            upstream_error: None,
            usage: None,
        }
    }

    /// 吃一段上游字节，吐出要下发的 chat SSE 帧
    pub fn push(&mut self, chunk: &[u8]) -> Vec<bytes::Bytes> {
        let mut out = Vec::new();
        for payload in self.buffer.push(chunk) {
            // `None` = 上游发来了 `[DONE]`（本家实测不发，但收下不亏）
            let Some(line) = payload else {
                out.extend(self.finish());
                continue;
            };
            // 本家的保活帧是裸 `HEARTBEAT`（**不带花括号**），不是 JSON
            if line.trim().eq_ignore_ascii_case("HEARTBEAT") {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(value) => out.extend(self.consume(&value)),
                Err(error) => {
                    crate::server::logging::verbose(
                        "[ChatGLM]",
                        &format!("上游 SSE 载荷不是合法 JSON（已忽略）：{error}"),
                    );
                }
            }
            if self.finished {
                break;
            }
        }
        out
    }

    /// 上游流结束：按「有没有见过终态」收尾
    pub fn finish(&mut self) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        // 先冲刷残留半行（上游可能最后一帧不带换行）
        let mut out = Vec::new();
        for payload in self.buffer.finish() {
            if let Some(line) = payload {
                if line.trim().eq_ignore_ascii_case("HEARTBEAT") {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    out.extend(self.consume(&value));
                }
            }
            if self.finished {
                return out;
            }
        }
        if self.finished {
            return out;
        }
        // 没有终态就 EOF：上游契约里是「没走完」。但本家实测**正常流末尾一定带
        // `status:"finish"`**，因此走到这里只可能是真截断 —— 如实报错，
        // 不补 finish_reason（那等于把截断谎报成完整回答）。
        out.extend(self.fail_message("上游流在收到完成信号之前结束（响应被截断）", 502));
        out
    }

    /// 一帧 → 零到多个 chat 帧
    fn consume(&mut self, event: &Value) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        let Some(object) = event.as_object() else {
            return Vec::new();
        };
        // 会话 id（首帧给出，后续帧重复；留最后一次）
        let conversation_id = string_field(event, "conversation_id");
        if !conversation_id.is_empty() {
            self.conversation_id = conversation_id;
        }
        // 业务错误：本家把失败也放在 200 的 SSE 里（如 -1001 积分不足），
        // 字段名实测是 `error_code` + `error`（见 `engine/stream` 的探测）
        if let Some(code) = object.get("error_code").and_then(Value::as_i64) {
            if code != 0 {
                let message = string_field(event, "error");
                let message = if message.trim().is_empty() {
                    format!("上游返回错误码 {code}")
                } else {
                    message
                };
                self.upstream_error = Some(message.clone());
                return self.fail_with_code(&message, code);
            }
        }
        if object.get("error").is_some_and(|value| {
            !value.is_null() && !value.as_str().map(str::trim).unwrap_or("x").is_empty()
        }) {
            let message = string_field(event, "error");
            self.upstream_error = Some(message.clone());
            return self.fail_with_code(&message, 502);
        }

        // 正文 / 思考：本帧文本 + 本帧是「增量」还是「全文快照」（见 `advance`）
        let (full_text, full_thinking) = collect_text(event);
        let is_snapshot = frame_is_snapshot(event);
        let (text_delta, thinking_delta) =
            self.advance(event, &full_text, &full_thinking, is_snapshot);

        // 终态判定：顶层 status == "finish"（实测顶层先变成 finish，其次 part）
        let frame_status = string_field(event, "status").to_lowercase();
        let is_finish = frame_status == "finish" || frame_status == "finished";

        let mut out = Vec::new();
        // 增量帧先发（内容永远先于收尾帧）
        if !text_delta.is_empty() || !thinking_delta.is_empty() {
            out.extend(self.start());
            let mut delta = serde_json::Map::new();
            if !thinking_delta.is_empty() {
                delta.insert(
                    "reasoning_content".to_string(),
                    Value::String(thinking_delta),
                );
            }
            if !text_delta.is_empty() {
                delta.insert("content".to_string(), Value::String(text_delta));
            }
            out.push(self.delta_frame(Value::Object(delta)));
        }
        if is_finish {
            out.extend(self.complete());
        }
        out
    }

    /// 本帧 → 要下发的 `(正文增量, 思考增量)`，并推进当前段的累积基准。
    ///
    /// ── 三种帧（实测，逐条对应真实流）──────────────────────────
    /// 1. **占位段**（`part.meta_data.engine.action == "loading"`）：
    ///    上游在真正回答前先发一段状态文案（实测原文：「让我们先核对下本轮任务的
    ///    目标与偏好，正在梳理您的需求～」）。它**不是回答的一部分** —— 官方客户端
    ///    也只是短暂显示再替换掉。我们一旦下发就收不回来，所以整段跳过。
    /// 2. **增量帧**（`part.status == "init"`）：本帧文本就是新增的一小段，
    ///    原样下发并追加进基准。
    /// 3. **全文快照帧**（`part.status` / 顶层 `status` 为 finish）：本帧给的是
    ///    **整段全文**，只补「还没下发过」的那一截。
    ///
    /// ── 为什么第 3 条不能像以前那样"对不上就发全文"（真实故障）──────
    /// 曾经对每帧一律做前缀差分，而收尾帧给的是全文、与前一段增量对不上，
    /// 于是整段回答被**再下发一遍** —— 用户看到的就是「AI 回答了两遍」。
    /// 快照对不上只可能是上游改写/回退，那种情况下重复一遍比漏几个字更糟
    /// （重复是稳定可见的故障，改写极少见且下游无法撤回已发内容）。
    fn advance(
        &mut self,
        event: &Value,
        full_text: &str,
        full_thinking: &str,
        is_snapshot: bool,
    ) -> (String, String) {
        if is_loading_placeholder(event) {
            return (String::new(), String::new());
        }
        // 换段（logic_id 变了）⇒ 新的一段开始，基准清零
        let key = segment_key_of(event);
        if !key.is_empty() && key != self.segment_key {
            self.segment_key = key;
            self.segment_text.clear();
            self.segment_thinking.clear();
        }
        let text = advance_segment(&mut self.segment_text, full_text, is_snapshot);
        let thinking = advance_segment(&mut self.segment_thinking, full_thinking, is_snapshot);
        (text, thinking)
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

    /// 正常收尾：finish 帧 + usage 帧 + `[DONE]`
    fn complete(&mut self) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        // 一个内容帧都没给就收尾：这是「成功但什么都没有」的流，
        // 记成成功会让下游按空回答计费（判据与 Command Code 那台同款）
        if !self.started {
            return self.fail_message("上游返回空响应（没有任何内容）", 429);
        }
        self.finished = true;
        let mut out = self.start();
        out.push(self.finish_frame("stop"));
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

    /// 流内错误 → 错误帧 + `[DONE]`（只做一次）
    fn fail_with_code(&mut self, message: &str, code: i64) -> Vec<bytes::Bytes> {
        // -1001「剩余积分不足」是本家最常见的失败：它是**额度**问题而不是
        // 服务故障，因此给 429（下游据此退避 / 换账号，比塌成 502 更有用）
        let status = if code == -1001 { 429 } else { 502 };
        let kind = if status == 429 {
            "rate_limit_error"
        } else {
            "upstream_error"
        };
        self.fail_message_with_kind(message, kind, status, Some(code))
    }

    fn fail_message(&mut self, message: &str, status: i64) -> Vec<bytes::Bytes> {
        let kind = if status == 429 {
            "rate_limit_error"
        } else {
            "upstream_error"
        };
        self.fail_message_with_kind(message, kind, status, None)
    }

    /// 落一个协议级失败帧（错误帧 + `[DONE]`，只做一次）。
    ///
    /// 错误路径**不**补 usage 帧：带 `usage` 且 `choices` 为空的帧正是 OpenAI
    /// 流式的收尾形态，部分客户端据此判定「流已结束」—— 在它之后补错误帧
    /// 等于让那些客户端看不到失败（与 Command Code 那台的论证逐字相同）。
    fn fail_message_with_kind(
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
            "status": status,
        });
        if let Some(code) = code {
            if let Some(object) = error.as_object_mut() {
                object.insert("code".to_string(), Value::from(code));
            }
        }
        let mut frame = json!({ "error": error });
        if status == 429 {
            if let Some(object) = frame.as_object_mut() {
                object.insert("retry_after".to_string(), Value::from(30));
            }
        }
        let mut out = vec![chat_frame(&frame)];
        out.push(bytes::Bytes::from_static(b"data: [DONE]\n\n"));
        out
    }

    /// 上游（实测）不给 token 计数 —— 如实回 0，不编造数字
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

    /// 组装一条 chat 增量帧
    fn delta_frame(&self, delta: Value) -> bytes::Bytes {
        chat_frame(&json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": Value::Null }],
        }))
    }

    /// 组装收尾帧
    fn finish_frame(&self, reason: &str) -> bytes::Bytes {
        chat_frame(&json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }],
        }))
    }
}

/// 求一段文本相对**当前段已下发内容**的增量，并推进基准。
///
/// - `is_snapshot == false`（增量帧）：本帧就是新内容，原样下发；
/// - `is_snapshot == true`（全文快照帧）：以已下发内容为前缀，只补后缀；
///   对不上（上游改写/回退）时**不补发** —— 补发就是「回答两遍」（见 `advance`）。
fn advance_segment(emitted: &mut String, frame: &str, is_snapshot: bool) -> String {
    if frame.is_empty() {
        return String::new();
    }
    if !is_snapshot {
        emitted.push_str(frame);
        return frame.to_string();
    }
    // `emitted` 为空时 `strip_prefix` 返回整段，因此新段的第一个快照帧会整段下发
    match frame.strip_prefix(emitted.as_str()) {
        Some(rest) => {
            emitted.push_str(rest);
            rest.to_string()
        }
        None => String::new(),
    }
}

/// 本帧是不是「全文快照」帧。
///
/// 实测：`part.status` 会先于顶层 `status` 变 finish（从 1 数到 5 的流里，
/// 第 6 帧 `top=init / part=finish` 且文本已是全文），因此两处都要认。
fn frame_is_snapshot(event: &Value) -> bool {
    if is_finish_status(&string_field(event, "status")) {
        return true;
    }
    event
        .get("parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            !parts.is_empty()
                && parts
                    .iter()
                    .all(|part| is_finish_status(&string_field(part, "status")))
        })
}

fn is_finish_status(status: &str) -> bool {
    matches!(status.to_lowercase().as_str(), "finish" | "finished")
}

/// 本帧是不是「回答前的临时占位段」（实测标记：`engine.action == "loading"`）。
///
/// 这类段有自己的 `logic_id`（与正文段不同），内容是一句状态文案。**不能下发**：
/// 官方客户端会把它替换掉，而下游收到的是最终正文，混进去就是一段永久的噪音。
fn is_loading_placeholder(event: &Value) -> bool {
    event
        .get("parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                part.pointer("/meta_data/engine/action")
                    .and_then(Value::as_str)
                    == Some("loading")
            })
        })
}

/// 本帧所属内容段的 key：`logic_id` 优先，缺失退回落 `part.id`。
///
/// 两个都没有（上游形态变了）时返回空串，调用方按「不换段」处理 ——
/// 宁可沿用旧基准，也不要在读到空串时把基准清掉（那会让整段重新下发一遍）。
fn segment_key_of(event: &Value) -> String {
    let Some(parts) = event.get("parts").and_then(Value::as_array) else {
        return String::new();
    };
    for part in parts {
        let logic = string_field(part, "logic_id");
        if !logic.is_empty() {
            return logic;
        }
        let id = string_field(part, "id");
        if !id.is_empty() {
            return id;
        }
    }
    String::new()
}

/// 把一帧里所有 part 的 text / think 块按顺序拼成两条全文。
///
/// 返回 `(正文全文, 思考全文)`。
fn collect_text(event: &Value) -> (String, String) {
    let mut text = String::new();
    let mut thinking = String::new();
    let Some(parts) = event.get("parts").and_then(Value::as_array) else {
        return (text, thinking);
    };
    for part in parts {
        let content = part.get("content").unwrap_or(&Value::Null);
        for item in content_parts(content) {
            let kind = string_field(item, "type").to_lowercase();
            if kind == KIND_TEXT {
                text.push_str(&string_field(item, "text"));
            } else if kind == KIND_THINK {
                thinking.push_str(&string_field(item, "think"));
            }
        }
    }
    (text, thinking)
}

/// 保留：usage 归一（本家暂不用，留给将来上游补 token 计数时接）
#[allow(dead_code)]
fn normalized_usage(value: &Value) -> Value {
    let prompt = json_number_of(value, &["prompt_tokens", "input_tokens"]);
    let completion = json_number_of(value, &["completion_tokens", "output_tokens"]);
    let total = json_number_of(value, &["total_tokens"]).max(prompt + completion);
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把下行帧拼成 (正文, 思考) 便于断言
    fn collect(frames: &[bytes::Bytes]) -> (String, String, Option<String>) {
        let mut text = String::new();
        let mut thinking = String::new();
        let mut finish = None;
        for frame in frames {
            let raw = String::from_utf8_lossy(frame);
            let Some(payload) = raw.trim().strip_prefix("data: ") else {
                continue;
            };
            if payload == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            let Some(choice) = value
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|list| list.first())
            else {
                continue;
            };
            if let Some(delta) = choice.get("delta") {
                if let Some(content) = delta.get("content").and_then(Value::as_str) {
                    text.push_str(content);
                }
                if let Some(reason) = delta.get("reasoning_content").and_then(Value::as_str) {
                    thinking.push_str(reason);
                }
            }
            if let Some(reason) = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                finish = Some(reason.to_string());
            }
        }
        (text, thinking, finish)
    }

    fn sse(payload: &str) -> Vec<u8> {
        format!("data: {payload}\n\n").into_bytes()
    }

    /// **回归：回答不能重复下发**（用户实测「AI 回答了两遍」）。
    ///
    /// 帧序列逐条照抄 live 抓帧：前五帧 `part=init` 是**增量**，收尾两帧给**全文**。
    /// 旧实现把每帧都按快照做前缀差分，收尾全文与前一段增量对不上就「发全文」，
    /// 于是整段回答被再下发一遍 —— 拼接结果正好是两遍回答。
    #[test]
    fn deltas_then_finish_snapshot_do_not_repeat_the_answer() {
        let mut machine = ChatFromChatglmStream::new("glm-5.3-flash");
        let mut frames = Vec::new();
        for text in ["1", "\n", "2\n3", "\n4\n", "5"] {
            frames.extend(machine.push(&sse(&format!(
                r#"{{"id":"m","conversation_id":"c","status":"init","parts":[{{"logic_id":"s","status":"init","content":[{{"type":"text","text":"{}"}}]}}]}}"#,
                text.replace('\n', "\\n")
            ))));
        }
        // 收尾第一拍：part 先 finish，这一帧已经是全文
        frames.extend(machine.push(&sse(
            r#"{"id":"m","conversation_id":"c","status":"init","parts":[{"logic_id":"s","status":"finish","content":[{"type":"text","text":"1\n2\n3\n4\n5"}]}]}"#,
        )));
        // 收尾第二拍：顶层终态，又来一遍同样的全文
        frames.extend(machine.push(&sse(
            r#"{"id":"m","conversation_id":"c","status":"finish","parts":[{"logic_id":"s","status":"finish","content":[{"type":"text","text":"1\n2\n3\n4\n5"}]}]}"#,
        )));
        let (text, _, finish) = collect(&frames);
        assert_eq!(text, "1\n2\n3\n4\n5", "完整回答只能出现一次");
        assert_eq!(finish.as_deref(), Some("stop"));
    }

    /// 短回答：首帧增量本身就是全文，收尾快照不得再补一遍（旧实现这里也发两遍）
    #[test]
    fn whole_answer_in_first_delta_is_not_repeated() {
        let mut machine = ChatFromChatglmStream::new("m");
        let mut frames = machine.push(&sse(
            r#"{"id":"m","status":"init","parts":[{"logic_id":"s","status":"init","content":[{"type":"text","text":"AAA"}]}]}"#,
        ));
        frames.extend(machine.push(&sse(
            r#"{"id":"m","status":"init","parts":[{"logic_id":"s","status":"finish","content":[{"type":"text","text":"AAA"}]}]}"#,
        )));
        frames.extend(machine.push(&sse(
            r#"{"id":"m","status":"finish","parts":[{"logic_id":"s","status":"finish","content":[{"type":"text","text":"AAA"}]}]}"#,
        )));
        let (text, _, _) = collect(&frames);
        assert_eq!(text, "AAA");
    }

    /// 收尾快照比增量累计更长时，只补缺口那一段
    #[test]
    fn finish_snapshot_only_emits_the_missing_tail() {
        let mut machine = ChatFromChatglmStream::new("m");
        for text in ["前", "半"] {
            machine.push(&sse(&format!(
                r#"{{"status":"init","parts":[{{"logic_id":"s","status":"init","content":[{{"type":"text","text":"{text}"}}]}}]}}"#
            )));
        }
        let frames = machine.push(&sse(
            r#"{"status":"finish","parts":[{"logic_id":"s","status":"finish","content":[{"type":"text","text":"前半加尾"}]}]}"#,
        ));
        let (text, _, _) = collect(&frames);
        assert_eq!(text, "加尾");
    }

    /// 占位段（`engine.action == "loading"`）整段跳过，正文段照常下发
    #[test]
    fn loading_placeholder_is_skipped() {
        let mut machine = ChatFromChatglmStream::new("m");
        let frames = machine.push(&sse(
            r#"{"status":"init","parts":[{"logic_id":"ph","status":"finish","meta_data":{"engine":{"type":"ppt","action":"loading"}},"content":[{"type":"text","text":"让我们先核对下本轮任务的目标与偏好，正在梳理您的需求～"}]}]}"#,
        ));
        let (text, _, _) = collect(&frames);
        assert_eq!(text, "", "占位文案不是回答，下发就收不回来了");
        // 正文段（另一个 logic_id）开始 → 基准清零，首帧整段下发
        let frames = machine.push(&sse(
            r#"{"status":"init","parts":[{"logic_id":"body","status":"init","content":[{"type":"text","text":"真正的回答"}]}]}"#,
        ));
        let (text, _, _) = collect(&frames);
        assert_eq!(text, "真正的回答");
    }

    /// 换段（logic_id 变了）必须重置基准，否则新段的首帧会被旧段基准吞掉
    #[test]
    fn segment_change_resets_the_baseline() {
        let mut machine = ChatFromChatglmStream::new("m");
        machine.push(&sse(
            r#"{"status":"init","parts":[{"logic_id":"think","status":"init","content":[{"type":"think","think":"先想"}]}]}"#,
        ));
        let frames = machine.push(&sse(
            r#"{"status":"init","parts":[{"logic_id":"body","status":"finish","content":[{"type":"text","text":"正文全文"}]}]}"#,
        ));
        let (text, thinking, _) = collect(&frames);
        assert_eq!(text, "正文全文", "新段的第一个快照帧应整段下发");
        assert_eq!(thinking, "", "新段没有思考内容");
    }

    /// 思考内容走 reasoning_content 通道，与正文同一套「增量 / 快照」规则
    #[test]
    fn thinking_uses_reasoning_channel() {
        let mut machine = ChatFromChatglmStream::new("m");
        machine.push(&sse(
            r#"{"status":"init","parts":[{"logic_id":"t","status":"init","content":[{"type":"think","think":"想"}]}]}"#,
        ));
        let frames = machine.push(&sse(
            r#"{"status":"init","parts":[{"logic_id":"t","status":"finish","content":[{"type":"think","think":"想一会"}]}]}"#,
        ));
        let (text, thinking, _) = collect(&frames);
        assert_eq!(text, "");
        assert_eq!(thinking, "一会", "快照只补后缀");
    }

    /// 正文与思考同帧给出时各走各的通道
    #[test]
    fn text_and_thinking_are_separated() {
        let mut machine = ChatFromChatglmStream::new("m");
        let frames = machine.push(&sse(
            r#"{"status":"init","parts":[{"content":[{"type":"think","think":"推理"},{"type":"text","text":"答案"}]}]}"#,
        ));
        let (text, thinking, _) = collect(&frames);
        assert_eq!(text, "答案");
        assert_eq!(thinking, "推理");
    }

    /// HEARTBEAT 帧被跳过（不带花括号的裸文本）
    #[test]
    fn heartbeat_is_ignored() {
        let mut machine = ChatFromChatglmStream::new("m");
        let frames = machine.push(b"data: HEARTBEAT\n\n");
        assert!(frames.is_empty());
    }

    /// 快照对不上（上游改写/回退）时**不补发**：整段重发的故障比漏几个字更糟
    #[test]
    fn mismatched_snapshot_is_not_replayed() {
        let mut machine = ChatFromChatglmStream::new("m");
        machine.push(&sse(
            r#"{"status":"init","parts":[{"logic_id":"s","status":"init","content":[{"type":"text","text":"ABC"}]}]}"#,
        ));
        let frames = machine.push(&sse(
            r#"{"status":"finish","parts":[{"logic_id":"s","status":"finish","content":[{"type":"text","text":"XYZ"}]}]}"#,
        ));
        let (text, _, _) = collect(&frames);
        assert_eq!(text, "", "对不上的快照不得整段重发");
    }

    /// 多 part 按顺序拼接后差分
    #[test]
    fn multiple_parts_are_concatenated() {
        let mut machine = ChatFromChatglmStream::new("m");
        let frames = machine.push(&sse(
            r#"{"status":"init","parts":[{"content":[{"type":"text","text":"前半"}]},{"content":[{"type":"text","text":"后半"}]}]}"#,
        ));
        let (text, _, _) = collect(&frames);
        assert_eq!(text, "前半后半");
    }

    /// 业务错误码（积分不足）→ 429 错误帧
    #[test]
    fn quota_error_becomes_rate_limit() {
        let mut machine = ChatFromChatglmStream::new("m");
        let frames = machine.push(&sse(
            r#"{"status":-1001,"message":"剩余积分不足","error_code":-1001,"error":"剩余积分不足，可购买积分充值包获得更多积分"}"#,
        ));
        let raw = String::from_utf8_lossy(&frames[0]).to_string();
        assert!(raw.contains("rate_limit_error"), "实际: {raw}");
        assert!(raw.contains("积分不足"));
        assert!(raw.contains("429"));
    }

    /// 正常流末尾带 finish；没有终态就 EOF 要报截断（不谎报成功）
    #[test]
    fn truncated_stream_reports_error() {
        let mut machine = ChatFromChatglmStream::new("m");
        machine.push(&sse(
            r#"{"status":"init","parts":[{"content":[{"type":"text","text":"半截"}]}]}"#,
        ));
        let frames = machine.finish();
        let raw = String::from_utf8_lossy(&frames[0]).to_string();
        assert!(raw.contains("截断") || raw.contains("upstream_error"), "实际: {raw}");
    }

    /// 一个内容都没有就 finish → 空响应报错（不记成空回答成功）
    #[test]
    fn empty_stream_reports_error() {
        let mut machine = ChatFromChatglmStream::new("m");
        let frames = machine.push(&sse(
            r#"{"status":"finish","parts":[{"content":[]}]}"#,
        ));
        let raw = String::from_utf8_lossy(&frames[0]).to_string();
        assert!(raw.contains("空响应"), "实际: {raw}");
    }

    /// 收尾必须带 [DONE]（上游自己不发）
    #[test]
    fn done_marker_is_appended() {
        let mut machine = ChatFromChatglmStream::new("m");
        machine.push(&sse(
            r#"{"status":"init","parts":[{"content":[{"type":"text","text":"x"}]}]}"#,
        ));
        let frames = machine.push(&sse(r#"{"status":"finish","parts":[]}"#));
        let joined = frames
            .iter()
            .map(|frame| String::from_utf8_lossy(frame).to_string())
            .collect::<String>();
        assert!(joined.contains("data: [DONE]"));
    }

    /// 空响应判据不会被「只有 role 首帧」绕过
    #[test]
    fn role_only_start_is_not_content() {
        let mut machine = ChatFromChatglmStream::new("m");
        // 只给空 parts，直接 finish
        let frames = machine.push(&sse(r#"{"status":"finish","parts":[]}"#));
        let raw = String::from_utf8_lossy(&frames[0]).to_string();
        assert!(raw.contains("error"));
    }

    /// 差分工具本身：增量原样、快照只补后缀、对不上不发
    #[test]
    fn advance_segment_rules() {
        let mut emitted = String::new();
        // 增量帧：本帧就是新内容，原样下发并累积
        assert_eq!(advance_segment(&mut emitted, "abc", false), "abc");
        assert_eq!(advance_segment(&mut emitted, "def", false), "def");
        // 快照帧：只补还没下发过的后缀
        assert_eq!(advance_segment(&mut emitted, "abcdefgh", true), "gh");
        // 与已下发内容完全一致 → 无增量（最要紧的一条：不能重复下发）
        assert_eq!(advance_segment(&mut emitted, "abcdefgh", true), "");
        // 对不上（上游改写 / 回退）→ 不发，绝不整段重发
        assert_eq!(advance_segment(&mut emitted, "xyz", true), "");
        // 空帧 → 无增量
        assert_eq!(advance_segment(&mut emitted, "", true), "");
        assert_eq!(advance_segment(&mut emitted, "", false), "");
        // 新段的第一个快照帧（基准为空）→ 整段下发
        let mut fresh = String::new();
        assert_eq!(advance_segment(&mut fresh, "整段", true), "整段");
    }
}
