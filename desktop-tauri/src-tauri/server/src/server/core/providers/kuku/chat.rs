//! KukuAI 对话转发：建会话 → 分配算力 → SSE 流式 → OpenAI 形状。
//!
//! ── 上游是**会话式三步走**（kuku2api 2026-09-13 实测，本模块照抄）────
//!   1. `POST /wenchain/genflowpro/sendmsg`（带 `bdstoken/uinfo/uk` 会话三件套）
//!      建会话，返回 `{session_id, reply_id}`；
//!   2. `POST /wenchain/genflow/idallochstr` 分配算力（+ `sessionswitch`，
//!      浏览器会调，容错 —— 失败不影响对话）；
//!   3. `POST /wenchain/genflowpro/sse/getchatcontent`（SSE）→ 流式文本。
//! SSE 帧是 `data: {type, data}` JSON，事件只有四类：
//!   `TEXT_BLOCK_DELTA`（`data.delta` 增量）/ `REPLY_END` / `DIALOGUE_END`
//!   （结束）/ `ERROR`（终止流，已发内容保留 —— 与参考实现一致）。
//!
//! ── 多轮与模型 ──────────────────────────────────────────────
//! 上游**没有多轮接续接口**：每次请求新建会话，历史拍平成单条 prompt
//! （`build_prompt`：system → `[系统指令]`、assistant → `[助手之前的回复]`，
//! 与参考实现同一口径）。模型参数 `model_name` 直接给上游（`models::resolve_model`
//! 归一，未知显式回退 `auto`）。工具调用第一版**不做**注入（上游有产品级人格
//! 保护，注入式协议不可靠，见 `mod.rs` 模块头）。
//!
//! ── 与 CatPaw / Trae 的形态差异 ──────────────────────────────
//! 同样是 `is_stateful = true`，但 KukuAI 的会话是**请求内**的（建完即用、
//! 用毕即弃），不需要会话注册表 / 轮次状态机：`run_chat` 单函数完成
//! 「建会话 → 流式 → 收尾」，不跨请求保持任何状态。
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

use super::credentials::{self, KukuCredentials, request_headers};
use super::session::{self, TokenTriple};
use super::{APP_ID, BASE_URL, CHANNEL, DEFAULT_MODEL, DEFAULT_THINK_MODE, WEB_QUERY};

/// 客户端会话通道名（kuku2api 实测值）
const CHANNEL_NAME: &str = "kuku_web_genflowpro_v1";

/// 建会话请求超时
const SESSION_TIMEOUT_MS: u64 = 60_000;

/// 上游流空闲兜底（本项目编排层对出站流有统一 idle_guard；这里是兜底值）
const STREAM_IDLE_MS: u64 = 120_000;

/// 转发入口（有状态路径 `forward_conversation` 转调）。
pub async fn run_chat(
    store: &AccountStore,
    account_id: &str,
    body: &Value,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<ForwardOutcome, GatewayError> {
    let credentials = credentials::snapshot_for(store, account_id)?;
    let client_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_MODEL)
        .to_string();
    let model = super::models::resolve_model(&client_model);
    let prompt = build_prompt(body.get("messages"));
    let triple = session::ensure_tokens(&credentials, proxy.as_ref(), false).await?;
    let query = format!("{WEB_QUERY}{}", session::token_query(&triple));

    // ── 建会话 ──────────────────────────────────────────────
    let created = create_session(&credentials, &query, &triple, &prompt, &model, proxy.as_ref()).await?;
    // 分配算力 + 切会话（容错：失败不阻断对话，浏览器也会调这两步）
    if let Err(error) = allocate(&credentials, &query, proxy.as_ref()).await {
        crate::server::logging::verbose("[Kuku]", &format!("idallochstr 失败（容错）：{}", error.message));
    }
    if let Err(error) = switch_session(&credentials, &query, &created.session_id, proxy.as_ref()).await {
        crate::server::logging::verbose("[Kuku]", &format!("sessionswitch 失败（容错）：{}", error.message));
    }

    // ── SSE 流 ──────────────────────────────────────────────
    let url = format!(
        "{BASE_URL}/wenchain/genflowpro/sse/getchatcontent?{query}"
    );
    let payload = chat_payload(&prompt, &model, &created.session_id, &created.reply_id);
    let headers = request_headers(&credentials);
    let response = super::http::sse_post(&url, &headers, &payload, proxy.as_ref(), "KukuAI 对话").await?;

    if let Some(capture) = telemetry.capture().as_deref() {
        capture.reset_request(&url, super::kuku_id(), &headers, body);
        capture.attach_response(response.status().as_u16(), response.headers());
    }

    if !stream {
        // 非流式：遍历同样的上游流，聚合为完整 JSON
        return aggregate(response, &client_model).await;
    }

    // 流式：后台任务喂 mpsc，主路径返回 ReceiverStream（与 Trae / Accio 同结构）
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let requested = client_model;
    crate::spawn_task(async move {
        let outcome = drive_stream(response, &requested, &sender).await;
        if let Err(error) = outcome {
            crate::server::logging::verbose("[Kuku]", &format!("对话流处理中断：{}", error.message));
        }
    });
    Ok(ForwardOutcome::Stream {
        status: 200,
        stream: Box::new(ReceiverStream::new(receiver)),
    })
}

/// 建会话（sendmsg）→ `{session_id, reply_id}`。
///
/// `triple` 是会话三件套：query 里拼一份、`client_added` 里嵌一份
/// （参考实现两处都带真实值，上游据此做客户端形态校验）。
async fn create_session(
    credentials: &KukuCredentials,
    query: &str,
    triple: &TokenTriple,
    prompt: &str,
    model: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<NewSession, GatewayError> {
    let url = format!("{BASE_URL}/wenchain/genflowpro/sendmsg?{query}");
    let headers = request_headers(credentials);
    let csi = random_uuid();
    let now_ms = crate::server::logging::now_ms();
    let show_text = json!({
        "skills": [], "experts": [], "model_name": model,
        "model_display_name": model, "think_mode": DEFAULT_THINK_MODE,
        "richInputContent": [{"type": "text", "text": prompt}], "fileInfo": []
    });
    let client_added = json!({
        "client_session_id": csi, "permission_type": 0, "clienttype": 400,
        "app_id": APP_ID, "web": 1, "channel": CHANNEL, "version": "1.4.4",
        "bdstoken": triple.bdstoken, "uinfo": triple.uinfo, "uk": triple.uk
    });
    let body = json!({
        "type": "message", "sub_type": "chat_create",
        "client_session_id": csi, "session_id": "", "uk": "", "cid": 0,
        "channel": CHANNEL_NAME, "device_type": 400,
        "created_at": now_ms, "msg_type": 1, "sync_type": 0, "sync_id": "",
        "v": random_hex(24),
        "data": {
            "project_type": 1, "text": prompt,
            "rich_input_params": [{
                "id": "textId", "version": "1.0", "type": "text",
                "text": prompt, "data": {"content": prompt}
            }],
            "fsid": [], "quotes": [], "skills": [], "experts": [],
            "model_name": model, "model_display_name": model,
            "think_mode": DEFAULT_THINK_MODE,
            "show_text": show_text.to_string(),
            "premake_data": {}, "custom_instructions": "",
            "memory_sign": true, "skill_dig_sign": true,
            "client_added": client_added.to_string()
        }
    });
    let value = super::http::post_json_value(&url, &headers, &body, proxy, Some(SESSION_TIMEOUT_MS)).await?;
    let code = value
        .get("status")
        .and_then(|status| status.get("code"))
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    if code != 0 {
        let message = value
            .get("status")
            .and_then(|status| status.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("未知错误")
            .to_string();
        return Err(GatewayError::with_status(
            502,
            format!("KukuAI 建会话失败（status.code={code}：{message}）"),
        ));
    }
    let info = value.get("data").cloned().unwrap_or(Value::Null);
    let session_id = info
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let reply_id = info
        .get("reply_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if session_id.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "KukuAI 建会话返回缺少 session_id（上游响应异常）",
        ));
    }
    Ok(NewSession { session_id, reply_id })
}

/// 分配算力（容错）。
async fn allocate(
    credentials: &KukuCredentials,
    query: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<(), GatewayError> {
    let url = format!("{BASE_URL}/wenchain/genflow/idallochstr?{query}");
    let headers = request_headers(credentials);
    let body = json!({ "channel": CHANNEL_NAME, "gen_type": 2 });
    super::http::post_json_value(&url, &headers, &body, proxy, Some(SESSION_TIMEOUT_MS)).await?;
    Ok(())
}

/// 切会话（浏览器会调，容错）。
async fn switch_session(
    credentials: &KukuCredentials,
    query: &str,
    session_id: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<(), GatewayError> {
    let url = format!("{BASE_URL}/api/genflowpro/workspace/sessionswitch?{query}");
    let headers = request_headers(credentials);
    let body = json!({ "session_id": session_id, "op": 1, "full_access_enabled": 0 });
    super::http::post_json_value(&url, &headers, &body, proxy, Some(SESSION_TIMEOUT_MS)).await?;
    Ok(())
}

/// 一次建会话的结果。
struct NewSession {
    session_id: String,
    reply_id: String,
}

/// getchatcontent 的请求体（sendmsg 的 `data` 段 + 会话标识）。
fn chat_payload(prompt: &str, model: &str, session_id: &str, reply_id: &str) -> Value {
    let show_text = json!({
        "skills": [], "experts": [], "model_name": model,
        "model_display_name": model, "think_mode": DEFAULT_THINK_MODE,
        "richInputContent": [{"type": "text", "text": prompt}], "fileInfo": []
    });
    json!({
        "project_type": 1, "text": prompt,
        "rich_input_params": [{
            "id": "textId", "version": "1.0", "type": "text",
            "text": prompt, "data": {"content": prompt}
        }],
        "fsid": [], "quotes": [], "skills": [], "experts": [],
        "model_name": model, "model_display_name": model,
        "think_mode": DEFAULT_THINK_MODE,
        "show_text": show_text.to_string(),
        "premake_data": {}, "custom_instructions": "",
        "memory_sign": true, "skill_dig_sign": true,
        "session_id": session_id, "reply_id": reply_id
    })
}

/// 把 OpenAI messages 压成单条 prompt（上游无多轮接续接口，参考实现同口径）。
pub fn build_prompt(messages: Option<&Value>) -> String {
    let Some(Value::Array(items)) = messages else {
        return "你好".to_string();
    };
    if items.is_empty() {
        return "你好".to_string();
    }
    if items.len() == 1 {
        return content_text(&items[0]).unwrap_or_else(|| "你好".to_string());
    }
    let mut lines: Vec<String> = Vec::new();
    for message in items {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        let text = content_text(message);
        let Some(text) = text else { continue };
        if text.trim().is_empty() {
            continue;
        }
        match role {
            "system" => lines.push(format!("[系统指令] {text}")),
            "assistant" => lines.push(format!("[助手之前的回复] {text}")),
            _ => lines.push(text),
        }
    }
    if lines.is_empty() {
        return "你好".to_string();
    }
    lines.join("\n\n")
}

/// 取一条消息的可读文本（content 字符串或 text 类型的部件数组）。
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
enum KukuEvent {
    Delta(String),
    Done,
    Error(String),
}

/// 按 `\n\n` 分帧、逐帧取 `data: ` 前缀 JSON 的解析器。
#[derive(Default)]
struct SseScanner {
    buffer: String,
}

impl SseScanner {
    fn push(&mut self, text: &str) -> Vec<KukuEvent> {
        self.buffer.push_str(text);
        let mut events = Vec::new();
        while let Some(split_at) = self.buffer.find("\n\n") {
            let raw = self.buffer.drain(..=split_at + 1).collect::<String>();
            for line in raw.split('\n') {
                let Some(payload) = line.strip_prefix("data: ") else { continue };
                let Ok(parsed) = serde_json::from_str::<Value>(payload) else { continue };
                if let Some(event) = event_from_json(&parsed) {
                    events.push(event);
                }
            }
        }
        events
    }
}

/// 一条 `data:` JSON → 事件（认不得的 type 忽略，与参考实现一致）。
fn event_from_json(parsed: &Value) -> Option<KukuEvent> {
    match parsed.get("type").and_then(Value::as_str) {
        Some("TEXT_BLOCK_DELTA") => {
            let delta = parsed
                .get("data")
                .and_then(|data| data.get("delta"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if delta.is_empty() {
                None
            } else {
                Some(KukuEvent::Delta(delta))
            }
        }
        Some("REPLY_END") | Some("DIALOGUE_END") => Some(KukuEvent::Done),
        Some("ERROR") => {
            let message = parsed
                .get("data")
                .and_then(|data| data.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("上游错误")
                .to_string();
            Some(KukuEvent::Error(message))
        }
        _ => None,
    }
}

// ─── OpenAI chunk 产出 ──────────────────────────────────────

/// 一帧出站内容。
enum Frame {
    Chunk(Value),
    Done,
}

impl Frame {
    /// 落到线上的字节（含 `\n\n` 帧尾）。
    fn wire(&self) -> String {
        match self {
            Frame::Chunk(chunk) => format!("data: {chunk}\n\n"),
            Frame::Done => "data: [DONE]\n\n".to_string(),
        }
    }
}

/// 流式转换器：喂上游字节，产出 OpenAI chunk。
struct KukuStream {
    id: String,
    created: i64,
    model: String,
    scanner: SseScanner,
    saw_end: bool,
    /// 非流式聚合用（也用于流式路径的错误兜底判断：发过内容就不再改判 HTTP 错误）
    content: String,
}

impl KukuStream {
    fn new(model: &str) -> Self {
        Self {
            id: format!("chatcmpl-{}", crate::server::logging::now_ms() * 1000 % 1_000_000_000_000_000),
            created: crate::server::logging::now_ms() / 1000,
            model: model.to_string(),
            scanner: SseScanner::default(),
            saw_end: false,
            content: String::new(),
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
                KukuEvent::Delta(delta) => {
                    self.content.push_str(&delta);
                    frames.push(self.chunk(delta, None));
                }
                KukuEvent::Done => {
                    self.saw_end = true;
                    frames.push(self.chunk(String::new(), Some("stop")));
                    frames.push(Frame::Done);
                }
                KukuEvent::Error(message) => {
                    crate::server::logging::verbose(
                        "[Kuku]",
                        &format!("上游 ERROR 事件：{message}"),
                    );
                    self.saw_end = true;
                    frames.push(Frame::Done);
                }
            }
        }
        frames
    }

    /// 流结束（EOF）：上游没给过结束帧时补 `[DONE]`，保证客户端能收尾。
    fn finish(&mut self) -> Vec<Frame> {
        if self.saw_end {
            return Vec::new();
        }
        self.saw_end = true;
        vec![Frame::Done]
    }

    fn chunk(&mut self, delta: String, finish_reason: Option<&str>) -> Frame {
        let mut choice = Map::new();
        choice.insert("index".to_string(), json!(0));
        if delta.is_empty() {
            choice.insert("delta".to_string(), json!({}));
        } else {
            choice.insert("delta".to_string(), json!({ "content": delta }));
        }
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
    let mut stream = KukuStream::new(model);
    while let Some(item) = source.next().await {
        let chunk = item.map_err(|error| {
            GatewayError::with_status(502, format!("KukuAI 上游流式传输中断: {error}"))
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
        .map_err(|error| GatewayError::with_status(502, format!("KukuAI 对话流下发失败：{error}")))
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
    let mut stream = KukuStream::new(client_model);
    while let Some(item) = source.next().await {
        let chunk = item.map_err(|error| {
            GatewayError::with_status(502, format!("KukuAI 上游流式传输中断: {error}"))
        })?;
        let text = String::from_utf8_lossy(&chunk).to_string();
        let _ = stream.feed(&text);
    }
    let _ = stream.finish();
    let content = stream.content.clone();
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

/// UUID v4 形态（`client_session_id` 用）。
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

/// 随机十六进制串（`v` 字段用，参考实现 24 位）。
fn random_hex(length: usize) -> String {
    let mut out = String::new();
    while out.len() < length {
        let mut bytes = [0u8; 16];
        let _ = getrandom::getrandom(&mut bytes);
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
            if out.len() >= length {
                break;
            }
        }
    }
    out
}
