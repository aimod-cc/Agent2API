//! 上游 SOLO SSE → OpenAI 形状的转换，以及非流式聚合。
//!
//! ── 这条流上有三件事必须先想清楚再写代码 ──────────────────
//! 1. **上游不发 `[DONE]`**。结束帧是 `event: done`，`[DONE]` 是这里补的；
//!    上游中途断掉（没有 done）也要补，否则客户端永远等在最后一片上。
//! 2. **业务错误藏在 200 的流里**（`event: error`）。它不是 chunk，
//!    而是一条 `event: error` + `[DONE]`；分类见 `errors.rs`。
//! 3. **`token_usage` 是"欠着"的**：读到它并不立刻发帧，而是挂在下一帧
//!    （正常就是那条 finish 帧）上一起走。所以"usage 之后流就断了"这种
//!    上游行为会让 usage **静默消失** —— 向量里就有这一例，照抄行为、
//!    在 M7 的对拍报告里记一笔，不偷偷改。
//!
//! 帧的**键顺序**不是契约（Go 的 map 序列化按 key 排序，serde 按插入顺序），
//! 所以测试比的是解析后的 JSON；`id` 与 `created` 每次运行都不同，
//! 向量里已归一成 `<ID>` / `<CREATED>`。

use serde_json::{Value, json};

use super::errors::{ErrorKind, error_event_fields, stream_error_kind};

/// 一条已解析的上游事件。`metadata` / `timing_cost` / `extra_info` / 未知名
/// 都归到 `Other`：当前实现**不处理**它们（`extra_info` 在两个 switch 里都落空，
/// 这是参考实现的既有行为，向量 `extra_info 帧（当前实现不处理）` 钉着）。
#[derive(Clone, Debug, PartialEq)]
pub enum SoloEvent {
    Output {
        response: String,
        reasoning: String,
        tool_calls: Option<Value>,
    },
    TokenUsage(Value),
    Done {
        finish_reason: String,
    },
    Error {
        code: i64,
        message: String,
    },
    Other,
}

/// 解析一条完整事件（`data` 是同一事件内多行 `data:` 累加后的结果）。
/// JSON 不合法时返回 `None` —— 参考实现是"忽略这一帧"，不是"报错断流"。
pub fn parse_solo_event(event: &str, data: &str) -> Option<SoloEvent> {
    if data.trim().is_empty() {
        // 只有 event 行没有 data：仍算一个事件边界（done 之类可能不带载荷）。
        return Some(match event {
            "done" => SoloEvent::Done { finish_reason: String::new() },
            _ => SoloEvent::Other,
        });
    }
    let parsed: Value = serde_json::from_str(data).ok()?;
    Some(match event {
        "output" => SoloEvent::Output {
            response: parsed.get("response").and_then(Value::as_str).unwrap_or_default().to_string(),
            reasoning: parsed
                .get("reasoning_content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            tool_calls: parsed.get("tool_calls").cloned().filter(|value| !value.is_null()),
        },
        "token_usage" => SoloEvent::TokenUsage(parsed),
        "done" => SoloEvent::Done {
            finish_reason: parsed.get("finish_reason").and_then(Value::as_str).unwrap_or_default().to_string(),
        },
        "error" => {
            let (code, message) = error_event_fields(&parsed);
            SoloEvent::Error { code, message }
        }
        _ => SoloEvent::Other,
    })
}

/// 一行一行的 SSE 状态机（跨行累积 `event:` 与多个 `data:`）。
#[derive(Default, Debug)]
pub struct SseScanner {
    event: String,
    data: String,
}

impl SseScanner {
    /// 喂一行（不含行尾换行）。返回该行**触发**的事件；一行可以触发一个，
    /// 也可以像大多数行那样返回 `None`（累积中）。
    pub fn feed_line(&mut self, line: &str) -> Option<SoloEvent> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if self.event.is_empty() {
                self.reset();
                return None;
            }
            let event = parse_solo_event(&self.event, &self.data);
            self.reset();
            return event;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            self.event = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            // 刻意不 trim：参考实现是**直接拼接**，多行 data 之间因此留下
            // 一个空格（`{"response":` + ` "y"}`），拼出来才是合法 JSON。
            self.data.push_str(rest);
        }
        // `:` 开头是注释行，忽略；其它前缀（含 `id:`）同样忽略。
        None
    }

    fn reset(&mut self) {
        self.event.clear();
        self.data.clear();
    }
}

/// 转换后的一帧出站内容。
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    /// 一个 OpenAI chunk（`data: {…}`）。
    Chunk(Value),
    /// 流内业务错误：`event: error` + 一句转义后的文案。
    Error {
        code: i64,
        message: String,
        kind: ErrorKind,
    },
    /// `data: [DONE]`
    Done,
}

impl Frame {
    /// 落到线上的字节（不含结尾的两个换行，由调用方补分隔）。
    pub fn wire(&self) -> String {
        match self {
            Self::Chunk(chunk) => format!("data: {chunk}"),
            Self::Error { code, message, .. } => {
                // 与参考实现同一条文案：`solo error code=<码> msg=<文案>`，
                // 载荷是**一个 JSON 字符串**而不是对象。
                format!("event: error\ndata: {}", json!(format!("solo error code={code} msg={message}")))
            }
            Self::Done => "data: [DONE]".to_string(),
        }
    }
}

/// 流式转换器。
pub struct SoloStream {
    id: String,
    created: i64,
    scanner: SseScanner,
    /// `token_usage` 读到后欠着，随下一帧一起发。
    pending_usage: Option<Value>,
    /// 上游给过完成事件（`done` / `error` 都算收过场）。
    saw_done: bool,
    /// 上游产出过内容（正文 / 思考 / 工具调用任一非空）。
    ///
    /// 与 `saw_done` 一起构成「未收尾」判据的两半（见 `forward.rs` 的
    /// `missing_terminal`）：**产出过内容却始终没说完成**才算截断 ——
    /// 「一句不吐就结束」归既有的空回答链管，不算到这里来。
    saw_output: bool,
}

impl SoloStream {
    pub fn new(id: String, created: i64) -> Self {
        Self { id, created, scanner: SseScanner::default(), pending_usage: None, saw_done: false, saw_output: false }
    }

    /// 上游给过完成事件（`done` / `error`）。
    pub fn saw_done(&self) -> bool {
        self.saw_done
    }

    /// 上游产出过内容。EOF 收尾时与 [`Self::saw_done`] 一起判「未收尾」。
    pub fn saw_output(&self) -> bool {
        self.saw_output
    }

    /// 喂一行，返回该行产生的帧。
    pub fn feed_line(&mut self, line: &str) -> Vec<Frame> {
        let Some(event) = self.scanner.feed_line(line) else {
            return Vec::new();
        };
        self.apply(event)
    }

    fn apply(&mut self, event: SoloEvent) -> Vec<Frame> {
        match event {
            SoloEvent::Output { response, reasoning, tool_calls } => {
                let mut delta = serde_json::Map::new();
                if !response.is_empty() {
                    delta.insert("content".to_string(), json!(response));
                }
                if !reasoning.is_empty() {
                    delta.insert("reasoning_content".to_string(), json!(reasoning));
                }
                if let Some(calls) = tool_calls {
                    delta.insert("tool_calls".to_string(), normalize_stream_tool_calls(calls));
                }
                if delta.is_empty() {
                    return Vec::new();
                }
                self.saw_output = true;
                vec![self.chunk(Value::Object(delta), None)]
            }
            SoloEvent::TokenUsage(usage) => {
                self.pending_usage = Some(usage);
                Vec::new()
            }
            SoloEvent::Done { finish_reason } => {
                self.saw_done = true;
                vec![self.chunk(json!({}), Some(finish_reason)), Frame::Done]
            }
            SoloEvent::Error { code, message } => {
                let kind = stream_error_kind(code, &message);
                self.saw_done = true;
                // 注意：这里**不终止读取**，所以后面再来 `done` 会再发一整套
                // 帧（向量 `流内错误 4008` 记的就是这个形状）。转发层在首包门
                // 之前看到这条 error 就会改判成 HTTP 错误，正常不会走到第二步。
                vec![Frame::Error { code, message, kind }, Frame::Done]
            }
            SoloEvent::Other => Vec::new(),
        }
    }

    /// 流结束（EOF）。上游没给过 `done` 时补一个 `[DONE]`，保证客户端能收尾。
    pub fn finish(&mut self) -> Vec<Frame> {
        if self.saw_done {
            return Vec::new();
        }
        self.saw_done = true;
        vec![Frame::Done]
    }

    fn chunk(&mut self, delta: Value, finish_reason: Option<String>) -> Frame {
        let mut choice = serde_json::Map::new();
        choice.insert("index".to_string(), json!(0));
        choice.insert("delta".to_string(), delta);
        if let Some(reason) = finish_reason {
            choice.insert("finish_reason".to_string(), json!(reason));
        }
        let mut chunk = serde_json::Map::new();
        chunk.insert("id".to_string(), json!(self.id));
        chunk.insert("object".to_string(), json!("chat.completion.chunk"));
        chunk.insert("created".to_string(), json!(self.created));
        chunk.insert("model".to_string(), json!(""));
        chunk.insert("choices".to_string(), Value::Array([Value::Object(choice)].to_vec()));
        if let Some(usage) = self.pending_usage.take() {
            chunk.insert("usage".to_string(), usage);
        }
        Frame::Chunk(Value::Object(chunk))
    }
}

/// 流式 tool_call 片段的字段整形：`function_call` → `function`，
/// 并剥掉 SOLO 专属的 `namespace` / `partial_arguments`（上游带着它们）。
fn normalize_stream_tool_calls(calls: Value) -> Value {
    let Value::Array(list) = calls else {
        // 上游偶尔给单个对象；按参考实现的处理方式包成数组。
        return Value::Array(vec![normalize_one_call(calls)]);
    };
    Value::Array(list.into_iter().map(normalize_one_call).collect())
}

fn normalize_one_call(call: Value) -> Value {
    let Value::Object(mut call) = call else {
        return call;
    };
    if let Some(function) = call.remove("function_call") {
        call.insert("function".to_string(), strip_solo_only_fields(function));
    } else if let Some(function) = call.get("function").cloned() {
        call.insert("function".to_string(), strip_solo_only_fields(function));
    }
    Value::Object(call)
}

fn strip_solo_only_fields(function: Value) -> Value {
    let Value::Object(mut function) = function else {
        return function;
    };
    function.remove("namespace");
    function.remove("partial_arguments");
    Value::Object(function)
}

/// 一条流内错误（非流式聚合时作为失败返回）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamError {
    pub code: i64,
    pub message: String,
}

impl StreamError {
    pub fn kind(&self) -> ErrorKind {
        stream_error_kind(self.code, &self.message)
    }

    /// 与参考实现逐字同形的文案（对拍时要能直接对照日志）。
    pub fn to_text(&self) -> String {
        format!("solo error code={} msg={}", self.code, self.message)
    }
}

/// 聚合结果：给客户端的 `chat.completion`，外加「上游这一轮是怎么收的场」。
///
/// 两个标志位是**转发层**的判据（`forward.rs` 的 `missing_terminal`）：正文形状
/// 按参考实现逐字不变（没给 `done` 时 `finish_reason` 仍是 `stop`），而
/// 「这份内容可不可信」留在转发层判 —— 转换层只如实报告事实，对拍向量一条都不用改。
pub struct Aggregated {
    pub body: Value,
    /// 上游给过完成事件（`done`；`error` 走的是 `Err` 那条路）
    pub saw_done: bool,
    /// 上游产出过内容（正文 / 思考 / 工具调用任一非空）
    pub saw_output: bool,
}

/// 非流式：把整条 SSE 折成一个 `chat.completion`。
///
/// 上游没有非流式端点（`stream` 被强制成 true），所以这一条是**唯一**的
/// 非流式实现路径，不是"流式的替代品"。
pub fn aggregate(text: &str, id: &str, created: i64) -> Result<Aggregated, StreamError> {
    let mut scanner = SseScanner::default();
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut finish_reason = "stop".to_string();
    let mut usage: Option<Value> = None;
    let mut calls: Vec<Value> = Vec::new();
    let mut upstream_error: Option<StreamError> = None;
    let mut saw_done = false;

    // 与 Go 的 `ReadString('\n')` 对齐：按行喂，**EOF 那一次还会再喂一个空行**
    // （那一下把最后一个没被空行闭合的事件推出去）。少喂这一行，
    // 结尾没有空行的流就会丢掉最后一帧。
    for line in text.split('\n').chain(std::iter::once("")) {
        let Some(event) = scanner.feed_line(line) else {
            continue;
        };
        match event {
            SoloEvent::Output { response, reasoning: thought, tool_calls } => {
                content.push_str(&response);
                reasoning.push_str(&thought);
                if let Some(calls_value) = tool_calls {
                    merge_tool_calls(&mut calls, calls_value);
                }
            }
            SoloEvent::TokenUsage(payload) => usage = Some(payload),
            SoloEvent::Done { finish_reason: reason } => {
                saw_done = true;
                if !reason.is_empty() {
                    finish_reason = reason;
                }
            }
            SoloEvent::Error { code, message } => upstream_error = Some(StreamError { code, message }),
            SoloEvent::Other => {}
        }
    }

    if let Some(error) = upstream_error {
        return Err(error);
    }
    // 只报事实：这一轮产出过内容吗（正文 / 思考 / 工具调用任一非空）。
    // 放在 `calls` 被搬进 message 之前算。
    let saw_output = !content.is_empty() || !reasoning.is_empty() || !calls.is_empty();
    let mut message = serde_json::Map::new();
    message.insert("role".to_string(), json!("assistant"));
    message.insert("content".to_string(), json!(content));
    if !reasoning.is_empty() {
        message.insert("reasoning_content".to_string(), json!(reasoning));
    }
    if !calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(calls));
    }
    let mut response = serde_json::Map::new();
    response.insert("id".to_string(), json!(id));
    response.insert("object".to_string(), json!("chat.completion"));
    response.insert("created".to_string(), json!(created));
    response.insert("model".to_string(), json!(""));
    response.insert(
        "choices".to_string(),
        json!([{"index": 0, "message": Value::Object(message), "finish_reason": finish_reason}]),
    );
    if let Some(usage) = usage {
        response.insert("usage".to_string(), usage);
    }
    Ok(Aggregated { body: Value::Object(response), saw_done, saw_output })
}

/// 按 `index` 合并流式 tool_call 片段：`id` / `type` / `function.name` 直覆盖，
/// `function.arguments` **拼接**。
fn merge_tool_calls(calls: &mut Vec<Value>, incoming: Value) {
    let list: Vec<Value> = match incoming {
        Value::Array(items) => items,
        Value::Object(one) => vec![Value::Object(one)],
        _ => return,
    };
    for call in list {
        let Value::Object(call) = call else { continue };
        let index = call.get("index").and_then(Value::as_i64).unwrap_or(0);
        let slot = match calls.iter_mut().find(|existing| {
            existing.get("index").and_then(Value::as_i64).unwrap_or(0) == index
        }) {
            Some(existing) => existing,
            None => {
                calls.push(json!({"index": index}));
                calls.last_mut().expect("刚推进去")
            }
        };
        let Some(target) = slot.as_object_mut() else {
            continue;
        };
        if let Some(id) = call.get("id").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            target.insert("id".to_string(), json!(id));
        }
        if let Some(kind) = call.get("type").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            target.insert("type".to_string(), json!(kind));
        }
        let delta_function = call
            .get("function")
            .or_else(|| call.get("function_call"))
            .cloned()
            .map(strip_solo_only_fields)
            .and_then(|value| value.as_object().cloned());
        let Some(delta_function) = delta_function else {
            continue;
        };
        let merged = target.entry("function".to_string()).or_insert_with(|| json!({}));
        let Some(merged) = merged.as_object_mut() else {
            continue;
        };
        if let Some(name) = delta_function.get("name").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            merged.insert("name".to_string(), json!(name));
        }
        if let Some(arguments) = delta_function.get("arguments").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            let previous = merged.get("arguments").and_then(Value::as_str).unwrap_or_default();
            merged.insert("arguments".to_string(), json!(format!("{previous}{arguments}")));
        }
    }
}

#[cfg(test)]
mod tests {
    //! 帧序列逐条对向量。这里最值钱的几条是"上游没给 done 也要收尾"、
    //! "坏 JSON 的帧被忽略而不是断流"、"error 之后 usage 会丢"。
    use super::*;

    const VECTORS: &str = include_str!("vectors/trae-vectors.json");

    fn document() -> Value {
        serde_json::from_str(VECTORS).expect("向量文件必须是合法 JSON")
    }

    /// 跑一遍转换器，返回**归一后**的帧文本（`<ID>` / `<CREATED>`），与向量同口径。
    fn run(input: &str) -> Vec<String> {
        let mut stream = SoloStream::new("chatcmpl-123".to_string(), 1_700_000_000);
        let mut frames = Vec::new();
        for line in input.split_inclusive('\n') {
            let line = line.strip_suffix('\n').unwrap_or(line);
            frames.extend(stream.feed_line(line));
        }
        frames.extend(stream.finish());
        frames
            .iter()
            .map(|frame| {
                let text = frame.wire();
                let text = text.replace("\"id\":\"chatcmpl-123\"", "\"id\":\"<ID>\"");
                text.replace("\"created\":1700000000", "\"created\":<CREATED>")
            })
            .collect()
    }

    #[test]
    fn stream_frames_match_the_reference_implementation() {
        let document = document();
        let cases = document["stream"].as_array().expect("stream 段是数组");
        assert!(cases.len() >= 14, "用例数不对，实际 {}", cases.len());
        for case in cases {
            let name = case["name"].as_str().unwrap_or("?");
            let want: Vec<String> = case["frames"].as_array().expect("frames 是数组").iter().map(|value| value.as_str().unwrap().to_string()).collect();
            let got = run(case["input"].as_str().unwrap());
            assert_eq!(want, got, "用例：{name}");
        }
    }

    /// 聚合结果里每次运行都不同的两个字段先归一，再比解析后的 JSON。
    /// （先做字符串替换再解析是不行的 —— 替换出来的 `<CREATED>` 不是合法 JSON。）
    fn canonical(mut document: Value) -> Value {
        if let Some(map) = document.as_object_mut() {
            map.insert("id".to_string(), json!("<ID>"));
            map.insert("created".to_string(), json!(0));
        }
        document
    }

    #[test]
    fn aggregate_documents_match_the_reference_implementation() {
        let document = document();
        let cases = document["aggregate"].as_array().expect("段存在");
        // 下界：这一段被清空时循环不执行、测试照样绿（`stream` 段有同样的下界，
        // 这里原先漏了，而 aggregate 是唯一会改"非流式回包形状"的对照）。
        assert!(cases.len() >= 12, "aggregate 用例只剩 {} 条", cases.len());
        for case in cases {
            let name = case["name"].as_str().unwrap_or("?");
            let input = case["input"].as_str().unwrap();
            match aggregate(input, "chatcmpl-123", 1_700_000_000) {
                Ok(got) => {
                    // 向量里的 `created` 是占位符 `<CREATED>`（生成时把每次不同的
                    // 时间戳归一掉了），它不是合法 JSON —— 先换成数字再解析。
                    let raw = case["output"].as_str().unwrap().replace("<CREATED>", "0");
                    let want: Value = serde_json::from_str(&raw).expect("向量 output 是合法 JSON");
                    assert_eq!(canonical(want), canonical(got.body), "用例：{name}");
                }
                Err(error) => {
                    assert_eq!(
                        case["error"].as_str().unwrap_or("<不该失败>"),
                        error.to_text(),
                        "用例：{name}"
                    );
                }
            }
        }
    }

    /// 两个标志位是转发层的判据（`forward.rs::missing_terminal`）。
    ///
    /// 参考实现把「没给 done 也结束」当**正常形状**（body 一字不改，向量钉着），
    /// 但**事实**必须能拿到：上游这一轮到底收没收场、产出过内容没有。
    /// 缺了它，一次截断在下游看起来就是一次正常完成（上游 issue #286）。
    #[test]
    fn aggregate_reports_whether_upstream_ever_declared_completion() {
        let done = "event: output\ndata: {\"response\":\"好\"}\n\nevent: done\ndata: {\"finish_reason\":\"stop\"}\n\n";
        let with_done = aggregate(done, "id", 0).expect("聚合应当成功");
        assert!(with_done.saw_done && with_done.saw_output);

        // 给了内容却没说完成：body 与带 done 的一模一样（那是向量钉的形状），
        // 差别只在这两个标志位上 —— 转发层据此报错而不是当成功。
        let no_done = aggregate("event: output\ndata: {\"response\":\"半句\"}\n\n", "id", 0).expect("聚合应当成功");
        assert!(!no_done.saw_done, "没见过 done 就不该说自己收过场");
        assert!(no_done.saw_output);
        assert_eq!("stop", no_done.body["choices"][0]["finish_reason"].as_str().unwrap_or(""), "body 形状不变");

        // 「一句不吐就结束」不算截断候选：空回答由既有的兜底链管，不能在这里报错。
        let silent = aggregate("event: token_usage\ndata: {\"prompt_tokens\":1}\n\n", "id", 0).expect("聚合应当成功");
        assert!(!silent.saw_done && !silent.saw_output);
    }

    #[test]
    fn an_error_frame_carries_the_classified_kind() {
        let frames = run("event: error\ndata: {\"code\":4008,\"message\":\"Your requests have exceeded the quota\"}\n\n");
        assert_eq!(2, frames.len());
        assert!(frames[0].starts_with("event: error"));
        assert!(frames[1].contains("[DONE]"));
        // 分类结果同样要能拿到（转发层据此冷却账号）
        let error = StreamError { code: 4008, message: "Your requests have exceeded the quota".to_string() };
        assert_eq!(ErrorKind::PlanLimit, error.kind());
    }

    #[test]
    fn a_usage_event_without_a_done_is_dropped_not_invented() {
        // 参考实现把 usage 欠在下一帧上；没有下一帧就随它去。
        let frames = run("event: token_usage\ndata: {\"prompt_tokens\":1}\n\n");
        assert_eq!(vec!["data: [DONE]".to_string()], frames, "不能凭空造一个带 usage 的 finish 帧");
    }

    #[test]
    fn malformed_frame_data_is_skipped_without_killing_the_stream() {
        let frames = run("event: output\ndata: {not json}\n\nevent: done\ndata: {\"finish_reason\":\"stop\"}\n\n");
        // 坏帧被忽略（不产 chunk），后面的 done 照常 → finish 帧 + [DONE] 两帧。
        assert_eq!(2, frames.len(), "坏帧只被跳过，不断流：{frames:?}");
        assert!(frames[0].contains("\"finish_reason\":\"stop\""), "{frames:?}");
        assert!(frames[1].contains("[DONE]"), "{frames:?}");
    }

    #[test]
    fn tool_call_deltas_are_merged_across_index_gaps() {
        let mut calls = Vec::new();
        merge_tool_calls(&mut calls, json!([{"index":1,"id":"b","function_call":{"name":"g","arguments":"{}","namespace":"n"}}]));
        merge_tool_calls(&mut calls, json!([{"index":0,"id":"a","function":{"name":"f","arguments":"{\"x\""}}]));
        merge_tool_calls(&mut calls, json!([{"index":0,"function":{"arguments":":1}"}}]));
        assert_eq!(2, calls.len());
        assert_eq!("f", calls[1]["function"]["name"]);
        assert_eq!("{\"x\":1}", calls[1]["function"]["arguments"], "arguments 是拼接的");
        assert!(calls[1]["function"].get("namespace").is_none(), "SOLO 专属字段要剥掉");
    }
}
