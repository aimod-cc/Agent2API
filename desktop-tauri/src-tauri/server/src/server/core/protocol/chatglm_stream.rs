//! 上游智谱清言 SSE（`parts` **全量快照**）→ 标准 chat SSE 增量帧（状态机）。
//!
//! ── 为什么需要新的一台状态机（而不是复用任何一台）──────────────
//! 本家的流是 SSE（`data: {...}` 逐帧），但**帧语义与 OpenAI 完全不同**：
//!
//! ```text
//!   智谱清言：每帧给「到目前为止的完整 parts 快照」
//!     {id, conversation_id, assistant_id, status, parts:[{id, logic_id, role,
//!      model, status, content:[{type,text|think|...}]}]}
//!   OpenAI ：每帧给「这一拍的增量 delta」
//!     {choices:[{delta:{content:"新增的几个字"}, index:0}]}
//! ```
//!
//! 因此本机的活儿是**差分**：记住上一帧的完整文本，与新快照比对，
//! 只把**新增的那一段**作为 `delta.content` 下发。
//! **不能直接累加**（那会把整段文本反复下发成重复内容），也**不能原样透传**
//! （下游按 delta 语义拼接，会把三次快照拼成三份全文）。
//!
//! ── 差分口径（实测帧序列推导）──────────────────────────────
//! 实测一轮 `输出：AAA` 的帧演进（见 `_recon` 记录）：
//!
//! ```text
//!   [1] top=init   part=init    text="AAA"     ← 首帧就可能是全文
//!   [2] top=init   part=finish  text="AAA"
//!   [3] top=finish part=finish  text="AAA"
//! ```
//! 以及长回答（从 1 数到 20）的渐进形态：`"1"` → `"1\n"` → `"1\n2\n3"` …
//! 可见快照是**单调增长**的，因此差分规则是：
//!   - 新文本以旧文本为前缀 → 下发 `新[旧.len()..]`（正常增量）；
//!   - 新文本**不以**旧文本为前缀（上游重写/回退，实测存在于 parts 被整段
//!     替换的场景）→ 下发**全文**（宁可重复也不能丢内容：下游拼接时重复
//!     好过缺字，且这种回退极少见）；
//!   - 文本变短 → 同上，下发全文。
//!
//! ── 与段落（parts / logic_id）的关系 ─────────────────────────
//! 一帧可能带多个 part（多段内容，如正文 + 工具结果）。本实现按**整帧文本**差分
//! （把所有 part 的 text 块按顺序拼成一条），而不是按 part 分别差分：
//!   - 下游的 chat 协议只有一条正文流，按 part 分别跟踪没有对应的下发形态；
//!   - 实测多 part 场景（工具调用后续写）里，整帧文本同样是单调增长的，
//!     整帧差分给出的正是「新增的那一段」。
//! `think` 块单独走 `reasoning_content` 通道，按同样规则差分。
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
    /// 上一帧的正文全文（差分的基准）
    last_text: String,
    /// 上一帧的思考全文（差分的基准）
    last_thinking: String,
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
            last_text: String::new(),
            last_thinking: String::new(),
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

        // 正文 / 思考：本帧全文（快照）
        let (full_text, full_thinking) = collect_text(event);
        let (text_delta, thinking_delta) = self.diff(&full_text, &full_thinking);

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

    /// 与上一帧快照求差，返回 `(正文增量, 思考增量)`。
    ///
    /// 规则见模块头：以旧值为前缀时发后缀；否则发全文（宁可重复不丢内容）。
    fn diff(&mut self, full_text: &str, full_thinking: &str) -> (String, String) {
        let text = diff_segment(&self.last_text, full_text);
        let thinking = diff_segment(&self.last_thinking, full_thinking);
        self.last_text = full_text.to_string();
        self.last_thinking = full_thinking.to_string();
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

/// 求一段文本相对上一版的增量（规则见模块头）
fn diff_segment(previous: &str, current: &str) -> String {
    if current.is_empty() {
        return String::new();
    }
    if previous.is_empty() {
        return current.to_string();
    }
    if let Some(rest) = current.strip_prefix(previous) {
        return rest.to_string();
    }
    // 不是简单增长（上游重写 / 回退）：发全文，宁重复不丢内容
    current.to_string()
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

    /// **最核心的一条**：全量快照必须差分，拼起来恰好是最终全文（不能重复）
    #[test]
    fn snapshot_frames_become_deltas() {
        let mut machine = ChatFromChatglmStream::new("glm-5.3-flash");
        let mut frames = Vec::new();
        // 递增快照（长回答的实测形态）
        for text in ["1", "1\n", "1\n2\n3", "1\n2\n3\n4"] {
            frames.extend(machine.push(&sse(&format!(
                r#"{{"id":"m","conversation_id":"c","status":"init","parts":[{{"content":[{{"type":"text","text":"{}"}}]}}]}}"#,
                text.replace('\n', "\\n")
            ))));
        }
        frames.extend(machine.push(&sse(
            r#"{"id":"m","conversation_id":"c","status":"finish","parts":[{"content":[{"type":"text","text":"1\n2\n3\n4"}]}]}"#,
        )));
        let (text, _, finish) = collect(&frames);
        // 差分后的拼接结果 == 最终全文（若误用累加，这里会是 "11\n1\n2\n3…"）
        assert_eq!(text, "1\n2\n3\n4");
        assert_eq!(finish.as_deref(), Some("stop"));
    }

    /// 首帧就是全文（实测形态）时也能正确下发
    #[test]
    fn full_text_on_first_frame() {
        let mut machine = ChatFromChatglmStream::new("m");
        let frames = machine.push(&sse(
            r#"{"id":"m","status":"init","parts":[{"content":[{"type":"text","text":"AAA"}]}]}"#,
        ));
        let (text, _, _) = collect(&frames);
        assert_eq!(text, "AAA");
        // 再来一帧完全相同的快照 → 无增量
        let frames = machine.push(&sse(
            r#"{"id":"m","status":"init","parts":[{"content":[{"type":"text","text":"AAA"}]}]}"#,
        ));
        let (text, _, _) = collect(&frames);
        assert_eq!(text, "", "相同快照不应产生增量");
    }

    /// 思考内容走 reasoning_content 通道，同样差分
    #[test]
    fn thinking_uses_reasoning_channel() {
        let mut machine = ChatFromChatglmStream::new("m");
        machine.push(&sse(
            r#"{"status":"init","parts":[{"content":[{"type":"think","think":"想"}]}]}"#,
        ));
        let frames = machine.push(&sse(
            r#"{"status":"init","parts":[{"content":[{"type":"think","think":"想一会"}]}]}"#,
        ));
        let (text, thinking, _) = collect(&frames);
        assert_eq!(text, "");
        assert_eq!(thinking, "一会", "思考也应是增量");
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

    /// 上游重写（快照不再是前缀）时下发全文（宁可重复不丢内容）
    #[test]
    fn rewritten_snapshot_falls_back_to_full_text() {
        let mut machine = ChatFromChatglmStream::new("m");
        machine.push(&sse(
            r#"{"status":"init","parts":[{"content":[{"type":"text","text":"ABC"}]}]}"#,
        ));
        let frames = machine.push(&sse(
            r#"{"status":"init","parts":[{"content":[{"type":"text","text":"XYZ"}]}]}"#,
        ));
        let (text, _, _) = collect(&frames);
        assert_eq!(text, "XYZ", "非前缀增长应发全文");
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

    /// 差分工具本身的三条规则
    #[test]
    fn diff_rules() {
        // 首帧（无基准）→ 全文
        assert_eq!(diff_segment("", "abc"), "abc");
        // 正常增长（旧值是前缀）→ 只发后缀
        assert_eq!(diff_segment("abc", "abcdef"), "def");
        // 完全相同的快照 → 无增量（最要紧的一条：不能重复下发）
        assert_eq!(diff_segment("abc", "abc"), "");
        // 上游重写（旧值不是前缀）→ 发全文
        assert_eq!(diff_segment("abc", "xyz"), "xyz");
        // 文本变短（回退）→ 也发全文（宁可重复，不可丢内容）
        assert_eq!(diff_segment("abcdef", "abc"), "abc");
        // 空的新帧 → 无增量
        assert_eq!(diff_segment("abc", ""), "");
    }
}
