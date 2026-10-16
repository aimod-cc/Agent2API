//! OrcaTerm 对话转发：建会话 → assistant SSE 流式 → OpenAI 形状。
//!
//! ── 上游是「建会话 → SSE」两步（2026-10-16 实测，curl 直连复现）────
//!   1. `POST {LIGHTAI}/assistant/conversation`
//!      body `{"id":"<uuid>","title":"新对话","ts":<ms>,"customTitle":"",
//!             "aiTitle":"","tags":["orcaterm"]}`
//!      → `{"code":0,"data":{…}}`；
//!   2. `POST {LIGHTAI}/assistant/chat?name=orcaterm&mode=0&csrfCode=&sseresume=true`
//!      body `{"conversationId":"cid-<uuid>-orcaterm","user":{…setting…},
//!             "input":{"type":"start","input":"<文本>","id":"<seqid>",…},
//!             "stream":true}` → `text/event-stream`。
//! SSE 帧是 `data: {json}`，字段 `content` / `reasoningContent` /
//! `reasoning`（true = 思考增量）/ `type:"ai"` / `code:0`；结束帧
//! `data: [DONE]`；错误帧**不是 JSON**：`data: [ERROR]: [LLM_ERROR]: …`。
//!
//! ── 多轮：历史拍平为单条输入（与 KukuAI 同一口径）──────────────
//! 上游按 conversationId 在服务端留存历史、输入通道只有一条 `start` 消息
//! （实测同会话连发两轮 `start` 能接上上下文），但 OpenAI 客户端每轮自带
//! 全量 messages —— 按请求内会话处理（每个请求新建会话，历史拍平进输入，
//! 见 `build_prompt`），无注册表无指纹链（取舍见 `mod.rs` 模块头）。
//!
//! ── 思考增量 ───────────────────────────────────────────────
//! 上游先流 `reasoning:true` 的思考增量、再流正文增量。思考增量按
//! DeepSeek 系的**事实标准**放进 `delta.reasoning_content` 下发（正文增量
//! 仍是 `delta.content`）；没有思考档位绑定（上游不在请求体上认档位字段，
//! 不接 [`ProviderAdapter::reasoning_patch`]，见 adapter）。
//!
//! ── 401 自愈 ───────────────────────────────────────────────
//! 建会话 / 对话拿到 401 时强制续期一次（`credentials::refresh(force=true)`）
//! 并重发同一步；只重试一次（与无状态路径「TokenExpired → 同账号重试一次」
//! 的既有语义一致）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic；持锁不跨 await。

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Map, Value, json};
use tokio_stream::wrappers::ReceiverStream;

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::ForwardOutcome;
use crate::server::errors::GatewayError;

use super::credentials::{self, OrcatermCredentials};
use super::http::{post_json_value, sse_post};
use super::{ASSISTANT_QUERY, LIGHTAI_BASE, models};

/// 上游流空闲兜底（本项目编排层对出站流有统一 idle_guard；这里是兜底值）
const STREAM_IDLE_MS: u64 = 120_000;

/// 一次 401 自愈重试的上限（每步各一次，不循环）
const AUTH_RETRY: usize = 1;

/// 转发入口（有状态路径 `forward_conversation` 转调）。
pub async fn run_chat(
    store: &AccountStore,
    account_id: &str,
    body: &Value,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<ForwardOutcome, GatewayError> {
    let mut credentials = credentials::snapshot_for(store, account_id)?;
    // 临期主动续期（结果回写账号记录；单飞保证并发只刷一次）
    credentials = credentials::refresh(store, &credentials, account_id, false).await?;

    let client_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let model = models::resolve_model(&client_model);
    let prompt = build_prompt(body.get("messages"));

    // ── 建会话（401 自愈：强制续期后重试一次）────────────────
    let mut conversation = create_conversation(&credentials, proxy.as_ref()).await;
    let mut attempts = 0;
    while let Err(error) = conversation {
        let retried = error.status_code == 401
            && attempts < AUTH_RETRY
            && refresh_for_retry(store, &mut credentials, account_id).await;
        if !retried {
            return Err(error);
        }
        attempts += 1;
        conversation = create_conversation(&credentials, proxy.as_ref()).await;
    }
    let conversation = conversation?;

    // ── SSE 对话流（同样 401 自愈）────────────────────────────
    let response = match open_chat_stream(&credentials, &conversation, &model, &prompt, proxy.as_ref()).await {
        Ok(response) => response,
        Err(error) => {
            let retried = error.status_code == 401
                && refresh_for_retry(store, &mut credentials, account_id).await;
            if !retried {
                return Err(error);
            }
            // 会话可能已在上一步建出（无妨：换 token 后重开流，conversationId 仍有效）
            open_chat_stream(&credentials, &conversation, &model, &prompt, proxy.as_ref()).await?
        }
    };

    if let Some(capture) = telemetry.capture().as_deref() {
        capture.reset_request(
            &format!("{LIGHTAI_BASE}/assistant/chat?{ASSISTANT_QUERY}"),
            super::orcaterm_id(),
            &credentials::lightai_headers(&credentials, "redacted"),
            body,
        );
        capture.attach_response(response.status().as_u16(), response.headers());
    }

    if !stream {
        // 非流式：遍历同样的上游流，聚合为完整 JSON
        return aggregate(response, &client_model).await;
    }

    // 流式：后台任务喂 mpsc，主路径返回 ReceiverStream（与 KukuAI / Trae 同结构）
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let requested = client_model;
    crate::spawn_task(async move {
        let outcome = drive_stream(response, &requested, &sender).await;
        if let Err(error) = outcome {
            crate::server::logging::verbose(
                "[OrcaTerm]",
                &format!("对话流处理中断：{}", error.message),
            );
        }
    });
    Ok(ForwardOutcome::Stream {
        status: 200,
        stream: Box::new(ReceiverStream::new(receiver)),
    })
}

/// 401 一步的自愈动作：强制续期；成功返回 true（调用方重发同一步）。
async fn refresh_for_retry(
    store: &AccountStore,
    credentials: &mut OrcatermCredentials,
    account_id: &str,
) -> bool {
    match credentials::refresh(store, credentials, account_id, true).await {
        Ok(next) => {
            *credentials = next;
            true
        }
        Err(error) => {
            crate::server::logging::verbose(
                "[OrcaTerm]",
                &format!("401 后续期失败：{}", error.message),
            );
            false
        }
    }
}

/// 建会话（`POST /assistant/conversation`）→ `cid-<uuid>-orcaterm`。
async fn create_conversation(
    credentials: &OrcatermCredentials,
    proxy: Option<&ResolvedProxy>,
) -> Result<String, GatewayError> {
    let uuid = random_uuid();
    let url = format!("{LIGHTAI_BASE}/assistant/conversation");
    let body = json!({
        "id": uuid,
        "title": "新对话",
        "ts": crate::server::logging::now_ms(),
        "customTitle": "",
        "aiTitle": "",
        "tags": ["orcaterm"],
    });
    let value = post_json_value(
        &url,
        &credentials::lightai_headers(credentials, &random_uuid()),
        &body,
        proxy,
        None,
    )
    .await?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = value.get("message").and_then(Value::as_str).unwrap_or("未知错误");
        return Err(GatewayError::with_status(
            502,
            format!("OrcaTerm 建会话失败（code={code}：{message}）"),
        ));
    }
    Ok(format!("cid-{uuid}-orcaterm"))
}

/// 打开对话 SSE 流。
async fn open_chat_stream(
    credentials: &OrcatermCredentials,
    conversation_id: &str,
    model: &str,
    prompt: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<reqwest::Response, GatewayError> {
    let seq_id = random_uuid();
    let url = format!("{LIGHTAI_BASE}/assistant/chat?{ASSISTANT_QUERY}");
    let body = json!({
        "conversationId": conversation_id,
        "user": {
            "id": credentials.user_id,
            "setting": {
                "model": model,
                "mcpServers": [],
                "uiServers": [],
                "tools": [],
                "approvedTools": [],
                "approvedMCPTools": [],
                "enableAutoSubtaskExecution": false,
                "env": [],
                "hideToolMessage": true,
                "language": "zh",
            },
        },
        "input": {
            "type": "start",
            "input": prompt,
            "id": seq_id,
            "emphasisPrompt": "",
            "files": [],
        },
        "stream": true,
    });
    sse_post(
        &url,
        &credentials::lightai_headers(credentials, &seq_id),
        &body,
        proxy,
        "OrcaTerm 对话",
    )
    .await
}

/// 把 OpenAI messages 压成单条输入（上游按会话留历史，本家按请求内会话处理，
/// 历史以对话记录形式并入 —— 与 KukuAI 的 `build_prompt` 同一取舍）。
///
/// 单条消息原样直通（最常见的单轮用法不引入任何包装）；多轮时把历史
/// 拍平成带角色前缀的记录，让上游把整段当作对话上下文来回应最后一条用户消息。
pub fn build_prompt(messages: Option<&Value>) -> String {
    let Some(Value::Array(items)) = messages else {
        return "你好".to_string();
    };
    let usable: Vec<&Value> = items
        .iter()
        .filter(|message| content_text(message).map(|text| !text.trim().is_empty()).unwrap_or(false))
        .collect();
    if usable.is_empty() {
        return "你好".to_string();
    }
    if usable.len() == 1 {
        return content_text(usable[0]).unwrap_or_else(|| "你好".to_string());
    }
    let mut lines: Vec<String> = Vec::new();
    lines.push("[以下是本轮之前的对话记录，供你了解上下文]".to_string());
    for message in usable {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        let Some(text) = content_text(message) else { continue };
        match role {
            "system" => lines.push(format!("[系统指令] {text}")),
            "assistant" => lines.push(format!("[助手] {text}")),
            _ => lines.push(format!("[用户] {text}")),
        }
    }
    lines.push("[对话记录结束；请回应其中最后一条用户消息]".to_string());
    lines.join("\n\n")
}

/// 取一条消息的可读文本（content 字符串或 text 类型部件数组）。
fn content_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let mut out: Vec<String> = Vec::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    out.push(text.to_string());
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out.join("\n"))
            }
        }
        _ => None,
    }
}

// ─── SSE 解析 ────────────────────────────────────────────────

/// 一条已解析的上游事件。
enum OrcaEvent {
    /// 思考增量（`reasoning:true` 帧）
    Reasoning(String),
    /// 正文增量（`reasoning:false` 帧的 `content`）
    Delta(String),
    /// 结束（`data: [DONE]`）
    Done,
    /// 流内错误（`data: [ERROR]: …`；HTTP 恒 200，错误在流内）
    Error(String),
}

/// 按 `\n\n` 分帧、逐帧取 `data:` 载荷的解析器。
#[derive(Default)]
struct SseScanner {
    buffer: String,
}

impl SseScanner {
    fn push(&mut self, text: &str) -> Vec<OrcaEvent> {
        self.buffer.push_str(text);
        let mut events = Vec::new();
        while let Some(split_at) = self.buffer.find("\n\n") {
            let raw = self.buffer.drain(..=split_at + 1).collect::<String>();
            for line in raw.split('\n') {
                let Some(payload) = line.strip_prefix("data:") else { continue };
                let payload = payload.trim_start();
                if payload.is_empty() {
                    continue;
                }
                if payload == "[DONE]" {
                    events.push(OrcaEvent::Done);
                    continue;
                }
                if let Some(error) = payload.strip_prefix("[ERROR]:") {
                    events.push(OrcaEvent::Error(error.trim().to_string()));
                    continue;
                }
                let Ok(parsed) = serde_json::from_str::<Value>(payload) else {
                    continue;
                };
                if parsed.get("type").and_then(Value::as_str) != Some("ai") {
                    continue;
                }
                let content = parsed.get("content").and_then(Value::as_str).unwrap_or("");
                let reasoning = parsed
                    .get("reasoningContent")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if parsed.get("reasoning").and_then(Value::as_bool).unwrap_or(false) {
                    if !reasoning.is_empty() {
                        events.push(OrcaEvent::Reasoning(reasoning.to_string()));
                    }
                } else if !content.is_empty() {
                    events.push(OrcaEvent::Delta(content.to_string()));
                }
            }
        }
        events
    }
}

// ─── OpenAI chunk 产出 ──────────────────────────────────────

/// 一帧出站内容。
enum Frame {
    Chunk(Value),
    /// OpenAI 形态的流内错误帧（`data: {"error":{…}}`）—— 流式请求整程
    /// 没有正文、只有上游错误时下发，客户端据此显示失败原因而不是空白。
    Error(String),
    Done,
}

impl Frame {
    /// 落到线上的字节（含 `\n\n` 帧尾）。
    fn wire(&self) -> String {
        match self {
            Frame::Chunk(chunk) => format!("data: {chunk}\n\n"),
            Frame::Error(message) => format!(
                "data: {}\n\n",
                json!({ "error": { "message": message, "type": "upstream_error" } })
            ),
            Frame::Done => "data: [DONE]\n\n".to_string(),
        }
    }
}

/// 流式转换器：喂上游字节，产出 OpenAI chunk。
///
/// ── 信封解包（本家独有的下游形态）──────────────────────────
/// 上游是 agent 协议：模型正文倾向于输出 ACP 动作信封
/// （```json 围栏或裸 JSON，形如 `{"action":"completion","taskCompletion":"…"}`
/// / `{"action":"ask","question":"…"}` / `{"action":"default","content":"…"}`），
/// 桌面端 UI 自己解析它 —— 但 OpenAI 客户端要的是纯文本，直接透传等于把
/// 一坨 JSON 当答案。因此本转换器在**第一个正文增量**到达时判定形态
/// （以 ``` / `{` 开头 → 信封形态，转缓冲模式；否则逐增量直播），
/// 结束时对缓冲内容做解包（取值链照抄前端 `Th`/`Ah`：completion→
/// taskCompletion、default→content、其余按 taskCompletion/content/question；
/// 解不出文本就原样下发缓冲内容，宁可透传也不错杀）。思考增量不受影响，
/// 始终逐增量直播。
struct OrcaStream {
    id: String,
    created: i64,
    model: String,
    scanner: SseScanner,
    saw_end: bool,
    /// 正文缓冲（信封模式下累积；直播模式恒空）
    content: String,
    /// 形态判定结果（第一个正文增量到达时定档）
    buffered: Option<bool>,
    /// 最后一条流内错误（`data: [ERROR]: …`；正文为空时它就是这次请求的
    /// 真实结果 —— 下发成显式错误而不是静默空回答，见 `final_content_frames`）
    last_error: Option<String>,
}

impl OrcaStream {
    fn new(model: &str) -> Self {
        Self {
            id: format!("chatcmpl-{}", crate::server::logging::now_ms() * 1000 % 1_000_000_000_000_000),
            created: crate::server::logging::now_ms() / 1000,
            model: model.to_string(),
            scanner: SseScanner::default(),
            saw_end: false,
            content: String::new(),
            buffered: None,
            last_error: None,
        }
    }

    /// 喂一段字节，返回该段触发的帧。
    fn feed(&mut self, text: &str) -> Vec<Frame> {
        if self.saw_end {
            return Vec::new();
        }
        let mut frames = Vec::new();
        for event in self.scanner.push(text) {
            match event {
                OrcaEvent::Reasoning(delta) => {
                    frames.push(self.chunk(delta, None, true));
                }
                OrcaEvent::Delta(delta) => match self.buffered {
                    // 形态未定：看开头是不是信封形态（``` 围栏 / JSON 对象）
                    None => {
                        if self.content.is_empty() && !delta.trim_start().is_empty() {
                            let head = delta.trim_start();
                            let envelope_shaped = head.starts_with("```") || head.starts_with('{');
                            self.buffered = Some(envelope_shaped);
                        }
                        self.content.push_str(&delta);
                        if !self.buffered.unwrap_or(false) {
                            frames.push(self.chunk(delta, None, false));
                        }
                    }
                    // 信封形态：只缓冲，不发帧
                    Some(true) => {
                        self.content.push_str(&delta);
                    }
                    // 直播形态：逐增量下发
                    Some(false) => {
                        frames.push(self.chunk(delta, None, false));
                    }
                },
                OrcaEvent::Done => {
                    self.saw_end = true;
                    frames.extend(self.final_content_frames(Some("stop")));
                }
                OrcaEvent::Error(message) => {
                    crate::server::logging::verbose(
                        "[OrcaTerm]",
                        &format!("上游 ERROR 事件：{message}"),
                    );
                    self.last_error = Some(message);
                    self.saw_end = true;
                    frames.extend(self.final_content_frames(None));
                }
            }
        }
        frames
    }

    /// 收尾帧：信封模式在这里解包并一次性下发正文，直播/空内容直接给结束帧。
    ///
    /// ── 流内错误的两种处置 ──────────────────────────────────
    /// 上游 HTTP 恒 200，错误在流内（`data: [ERROR]: …`）。正文**为空**时
    /// 这个错误就是整次请求的结果 —— 下发一帧 OpenAI 形态的 error 对象再收尾，
    /// 让客户端显示「模型不可用」而不是一个空白答案（面板上的 Hy4-preview
    /// 上游未挂载，选中它走的就是这条路）；已发出正文后的 ERROR 仍按既有
    /// 口径保留内容、静默收尾（与 KukuAI 一致）。`finish_reason` 由调用方给
    /// （正常收尾 = stop；ERROR 路径给 None）。
    fn final_content_frames(&mut self, finish_reason: Option<&str>) -> Vec<Frame> {
        let mut frames = Vec::new();
        if self.buffered == Some(true) {
            // 用 clone 而不是 take：非流式聚合路径会丢弃 feed 产出的帧、只读
            // self.content —— take 会把缓冲搬空，聚合端拿到的是空串
            let raw = self.content.clone();
            match unwrap_envelope(&raw) {
                Some(text) => frames.push(self.chunk(text, finish_reason, false)),
                // 解不出信封文本 = 不是信封（或字段认不出）：原样下发缓冲内容，
                // 宁可透传也不错杀用户自己要的 JSON
                None => {
                    if !raw.is_empty() {
                        frames.push(self.chunk(raw, finish_reason, false));
                    } else if let Some(reason) = finish_reason {
                        frames.push(self.chunk(String::new(), Some(reason), false));
                    }
                }
            }
        } else if let Some(reason) = finish_reason {
            // 直播模式：内容已实时下发，只补结束帧；信封判定未触发（无正文）
            // 时也一样 —— 空结束帧只表达 finish_reason
            frames.push(self.chunk(String::new(), Some(reason), false));
        }
        if self.content.is_empty() {
            if let Some(message) = &self.last_error {
                frames.push(Frame::Error(message.clone()));
            }
        }
        frames.push(Frame::Done);
        frames
    }

    /// 流结束（EOF）：上游没给过结束帧时补收尾，保证客户端能收尾。
    fn finish(&mut self) -> Vec<Frame> {
        if self.saw_end {
            return Vec::new();
        }
        self.saw_end = true;
        self.final_content_frames(Some("stop"))
    }

    fn chunk(&mut self, delta: String, finish_reason: Option<&str>, reasoning: bool) -> Frame {
        let mut choice = Map::new();
        choice.insert("index".to_string(), json!(0));
        let mut delta_body = Map::new();
        if !delta.is_empty() {
            if reasoning {
                delta_body.insert("reasoning_content".to_string(), json!(delta));
            } else {
                delta_body.insert("content".to_string(), json!(delta));
            }
        }
        choice.insert("delta".to_string(), Value::Object(delta_body));
        if let Some(reason) = finish_reason {
            choice.insert("finish_reason".to_string(), json!(reason));
        }
        let mut chunk = Map::new();
        chunk.insert("id".to_string(), json!(self.id));
        chunk.insert("object".to_string(), json!("chat.completion.chunk"));
        chunk.insert("created".to_string(), json!(self.created));
        chunk.insert("model".to_string(), json!(self.model));
        chunk.insert("choices".to_string(), Value::Array(vec![Value::Object(choice)]));
        Frame::Chunk(Value::Object(chunk))
    }
}

/// ACP 动作信封解包：整段正文（剥一层 ``` 围栏后）是带 `action` 键的 JSON
/// 对象时，抽出给人看的文本。取值链照抄桌面端 UI 的 `Th`/`Ah`：
/// `completion → taskCompletion`、`default → content`、其余动作按
/// `taskCompletion / content / question` 取第一个非空；都不是 → None
/// （调用方原样透传，不错杀非信封内容）。
fn unwrap_envelope(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let inner = if trimmed.starts_with("```") {
        trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else {
        trimmed
    };
    let parsed: Value = serde_json::from_str(inner).ok()?;
    let object = parsed.as_object()?;
    let action = object.get("action").and_then(Value::as_str)?;
    let text = match action {
        "completion" => object.get("taskCompletion").and_then(Value::as_str),
        "default" => object.get("content").and_then(Value::as_str),
        _ => ["taskCompletion", "content", "question"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str)),
    }?
    .trim();
    (!text.is_empty()).then(|| text.to_string())
}

// ─── 流式 / 非流式驱动 ──────────────────────────────────────

/// 流式：遍历上游流，把帧写进 mpsc。
async fn drive_stream(
    response: reqwest::Response,
    model: &str,
    sender: &tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
) -> Result<(), GatewayError> {
    let source = response
        .bytes_stream()
        .map(|item| item.map_err(|error| std::io::Error::other(crate::server::core::egress::describe_error_detail(&error))));
    let mut source = crate::server::core::upstream::stall::idle_guard(
        Box::pin(source),
        std::time::Duration::from_millis(STREAM_IDLE_MS),
    );
    let mut stream = OrcaStream::new(model);
    while let Some(item) = source.next().await {
        let chunk = item.map_err(|error| {
            GatewayError::with_status(502, format!("OrcaTerm 上游流式传输中断: {error}"))
        })?;
        let text = String::from_utf8_lossy(&chunk).to_string();
        for frame in stream.feed(&text) {
            send_frame(sender, &frame).await?;
        }
    }
    for frame in stream.finish() {
        send_frame(sender, &frame).await?;
    }
    Ok(())
}

async fn send_frame(
    sender: &tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
    frame: &Frame,
) -> Result<(), GatewayError> {
    sender
        .send(Ok(Bytes::from(frame.wire())))
        .await
        .map_err(|error| GatewayError::with_status(502, format!("OrcaTerm 对话流下发失败：{error}")))
}

/// 非流式：遍历同样的上游流，聚合为完整 JSON。
async fn aggregate(response: reqwest::Response, client_model: &str) -> Result<ForwardOutcome, GatewayError> {
    let source = response
        .bytes_stream()
        .map(|item| item.map_err(|error| std::io::Error::other(crate::server::core::egress::describe_error_detail(&error))));
    let mut source = crate::server::core::upstream::stall::idle_guard(
        Box::pin(source),
        std::time::Duration::from_millis(STREAM_IDLE_MS),
    );
    let mut stream = OrcaStream::new(client_model);
    while let Some(item) = source.next().await {
        let chunk = item.map_err(|error| {
            GatewayError::with_status(502, format!("OrcaTerm 上游流式传输中断: {error}"))
        })?;
        let text = String::from_utf8_lossy(&chunk).to_string();
        let _ = stream.feed(&text);
    }
    let _ = stream.finish();
    // 整程只有错误、没有正文：上抛成真正的 HTTP 错误（比返回一个空 content
    // 的 200 诚实 —— 客户端能看到「模型不可用」而不是一次假成功）
    if stream.content.is_empty() {
        if let Some(message) = &stream.last_error {
            return Err(GatewayError::with_status(502, format!("OrcaTerm 上游错误：{message}")));
        }
    }
    // 信封解包与非流式同口径：completion/ask/default 信封抽正文，其余原样
    let content = match unwrap_envelope(&stream.content) {
        Some(text) => text,
        None => stream.content.clone(),
    };
    let id = stream.id.clone();
    let created = stream.created;
    let model = stream.model.clone();
    let mut message = Map::new();
    message.insert("role".to_string(), json!("assistant"));
    message.insert("content".to_string(), json!(content));
    let mut choice = Map::new();
    choice.insert("index".to_string(), json!(0));
    choice.insert("message".to_string(), Value::Object(message));
    choice.insert("finish_reason".to_string(), json!("stop"));
    let mut body = Map::new();
    body.insert("id".to_string(), json!(id));
    body.insert("object".to_string(), json!("chat.completion"));
    body.insert("created".to_string(), json!(created));
    body.insert("model".to_string(), json!(model));
    body.insert("choices".to_string(), Value::Array(vec![Value::Object(choice)]));
    Ok(ForwardOutcome::Completion { body: Value::Object(body) })
}

// ─── 随机串 ──────────────────────────────────────────────────

/// UUID v4 形态（会话 id / x-seqid 用；实测普通 UUID 即可，无需特定格式）。
fn random_uuid() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::getrandom(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}
