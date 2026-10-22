//! Coze 对话转发：**网页版 claw 的「即发即忘 + 消息拉取」协议** → OpenAI 形状。
//!
//! ── ⚠️ 这里修正过一次结论（2026-10-22）──────────────────────
//! 第一版走的是 `api.coze.cn/v3/chat`（OpenAPI，**bot 维度**：`bot_id` 当模型），
//! 那条链要求用户先把 bot 发布成 API，与网页版聊天是两条平行产品线。
//! 复核后发现网页版真正的推理是 claw：
//! `session/get_main` → `select_model` → `message/chat` → 轮询 `message/list`。
//! 完整证据见 `_recon/coze-code-contract-2026-10-21.md` §5。
//!
//! ── 上游时序（活 PAT 实测）──────────────────────────────────
//! ```text
//! ① POST /api/coze_claw/session/get_main {claw_id}          → data.session.id
//!    （主会话直接复用；session/create 实测回 1000003 数据库错误，不能用）
//! ② POST /api/coze_claw/resource/select_model
//!    {claw_id, name: <模型名>, set_auto_fallback: true}      → {code:0}
//!    **模型是会话级状态**：切完 get_model_list 的 selected 就变了
//! ③ POST /api/coze_claw/message/chat {session_id, content, files: []}
//!    → {code:0, data:{message_id:"<uuid>"}}                 ← 没有正文
//! ④ POST /api/coze_claw/message/list {session_id, page_size}
//!    → data.messages[] 倒序；reply 的 content 是**字符串化 JSON**：
//!      {"message_data":{"reply":{"content":"…","reply_to_message_id":"<uuid>"}}}
//! ```
//!
//! ── 为什么必须按 `reply_to_message_id` 配对 ──────────────────
//! `message/list` 是**整段会话的历史**（用户自己的对话也在里面）。取「最新一条
//! reply」在并发请求或多轮对话时会串到别人的回复上，甚至读到上一轮的内容。
//! 唯一可靠的判据是第 ③ 步那个 `message_id`。
//!
//! ── 流式怎么给 ──────────────────────────────────────────────
//! 上游没有 SSE（网页版靠 WebSocket 推增量）。本网关以**固定间隔轮询**
//! `message/list`，拿到新 reply 的正文后按片切成 OpenAI SSE 帧下发 ——
//! 客户端看到的仍是标准流式。切片是按字切的（不是按词），只为让「边生成边显示」
//! 的观感成立；正文内容与上游完全一致，不做任何改写。
//!
//! ── 多轮 ────────────────────────────────────────────────────
//! `message/chat` 只收**本条** `content`：历史是服务端会话本身。
//! 但网关要保证「客户端发来的 messages 数组」语义正确（同一个会话被多个客户端
//! 复用、或消息被编辑过），所以把 `messages[]` 里最后一条 user 之前的历史
//! **拍平**进 content（`[用户] … / [助手] …`），与 KukuAI 同一口径。
//!
//! ── PAT 自愈 ────────────────────────────────────────────────
//! PAT 只有 10 分钟寿命。任一上游 401 时强制重铸一枚再重试一次
//! （`ensure_pat(force=true)`），仍失败才判定登录态失效。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic；持锁不跨 await。

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{Map, Value, json};
use tokio_stream::wrappers::ReceiverStream;

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::ForwardOutcome;
use crate::server::errors::GatewayError;

use super::credentials;
use super::http;
use super::WEB_BASE;

/// 拉回复的轮询间隔（上游生成一段回复通常 1~15 秒；250ms 兼顾延迟与请求量）
const POLL_INTERVAL_MS: u64 = 250;

/// 拉回复的总耐心（超过就当上游挂了；与各家 SSE 空闲兜底同量级）
const POLL_TIMEOUT: Duration = Duration::from_secs(180);

/// 单次上游 JSON 请求超时
const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// `message/list` 一次拉多少条（回复在最前，4 条足够覆盖并发交错）
const LIST_PAGE_SIZE: u32 = 8;

/// 流式下发的切片长度（字符数）：只为观感，不改内容
const STREAM_SLICE_CHARS: usize = 4;

/// 转发入口（有状态路径 `forward_conversation` 转调）。
pub async fn run_chat(
    store: &AccountStore,
    account_id: &str,
    body: &Value,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<ForwardOutcome, GatewayError> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if model.is_empty() {
        return Err(GatewayError::with_status(
            400,
            "Coze 转发缺少 model 字段（在模型页「获取模型」后选一个，如 deepseek-v4-flash-0731）",
        ));
    }
    let prompt = build_prompt(body.get("messages"));
    if prompt.trim().is_empty() {
        return Err(GatewayError::with_status(
            400,
            "Coze 转发缺少消息内容（messages 里没有可发送的 user 消息）",
        ));
    }

    // 整条链包在「PAT 失效就重铸重试一次」里
    let mut pat = credentials::ensure_pat(store, account_id, false).await?;
    let attempt = match send_round(&pat, &model, &prompt, &proxy, telemetry).await {
        Ok(context) => context,
        Err(error) if error.status_code == 401 => {
            pat = credentials::ensure_pat(store, account_id, true).await?;
            send_round(&pat, &model, &prompt, &proxy, telemetry).await?
        }
        Err(error) => return Err(error),
    };

    // 非流式：把回复等齐了一次性回
    if !stream {
        let reply = poll_reply(&pat, &attempt).await?;
        return Ok(ForwardOutcome::Completion {
            body: completion_body(&attempt.client_model, &reply, rough_usage(&prompt, &reply)),
        });
    }

    // 流式：后台任务轮询，主路径返回 mpsc 流
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    crate::spawn_task(async move {
        if let Err(error) = drive_stream(&pat, attempt, &sender).await {
            crate::server::logging::verbose("[Coze]", &format!("对话流处理中断：{}", error.message));
        }
    });
    Ok(ForwardOutcome::Stream {
        status: 200,
        stream: Box::new(ReceiverStream::new(receiver)),
    })
}

/// 一轮发送的结果（后续轮询需要的全部上下文）。
struct Round {
    /// 客户端请求的模型名（回包 `model` 字段要原样回显）
    client_model: String,
    /// 主会话 id（拉回复用）
    session_id: String,
    /// 第 ③ 步拿到的 message_id（拉回复时按 `reply_to_message_id` 精确配对）
    message_id: String,
    /// 本次发送的正文（估算 usage 用）
    prompt: String,
}

/// ①②③ 三步：取会话 → 选模型 → 发消息。
async fn send_round(
    pat: &str,
    model: &str,
    prompt: &str,
    proxy: &Option<ResolvedProxy>,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<Round, GatewayError> {
    let headers = credentials::pat_headers(pat);
    let claw_id = super::models::first_claw_id_for(&headers, proxy.as_ref()).await?;

    // ① 取主会话
    let session_url = format!("{WEB_BASE}/api/coze_claw/session/get_main");
    let session_value = http::post_json_value(
        &session_url,
        &headers,
        &json!({ "claw_id": claw_id }),
        proxy.as_ref(),
        Some(REQUEST_TIMEOUT_MS),
    )
    .await?;
    let session_id = session_value
        .get("data")
        .and_then(|data| data.get("session"))
        .and_then(|session| session.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            business_error(&session_value, "取主会话失败", "该账号在扣子上还没有可用会话")
        })?;

    // ② 选模型（会话级状态；每轮都写一次，避免被别的客户端切走）
    let select_url = format!("{WEB_BASE}/api/coze_claw/resource/select_model");
    let select_value = http::post_json_value(
        &select_url,
        &headers,
        &json!({ "claw_id": claw_id, "name": model, "set_auto_fallback": true }),
        proxy.as_ref(),
        Some(REQUEST_TIMEOUT_MS),
    )
    .await?;
    let select_code = select_value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if select_code != 0 {
        return Err(business_error(
            &select_value,
            "切换模型失败",
            "该模型可能已下线或当前账号没有访问权",
        ));
    }

    // ③ 发消息
    let chat_url = format!("{WEB_BASE}/api/coze_claw/message/chat");
    let chat_body = json!({ "session_id": session_id, "content": prompt, "files": [] });
    let chat_value = http::post_json_value(
        &chat_url,
        &headers,
        &chat_body,
        proxy.as_ref(),
        Some(REQUEST_TIMEOUT_MS),
    )
    .await?;
    let message_id = chat_value
        .get("data")
        .and_then(|data| data.get("message_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| business_error(&chat_value, "发送消息失败", "上游没有返回 message_id"))?;

    if let Some(capture) = telemetry.capture().as_deref() {
        capture.reset_request(&chat_url, super::coze_id(), &headers, &chat_body);
        capture.attach_response(200, &axum::http::HeaderMap::new());
    }

    Ok(Round {
        client_model: model.to_string(),
        session_id,
        message_id,
        prompt: prompt.to_string(),
    })
}

/// ④ 轮询直到拿到本条消息对应的回复。
///
/// 返回完整正文。**不再接受回调**：闭包跨 await 会让整个 Future 失去 `Send`
/// （`forward_conversation` 有 `Send` 约束），而流式路径本来也是拿到全文再切片
/// （上游 reply 是整段返回，不是增量），所以这里只收一个结果。
async fn poll_reply(pat: &str, round: &Round) -> Result<String, GatewayError> {
    let headers = credentials::pat_headers(pat);
    let url = format!("{WEB_BASE}/api/coze_claw/message/list");
    let body = json!({ "session_id": round.session_id, "page_size": LIST_PAGE_SIZE });
    let deadline = std::time::Instant::now() + POLL_TIMEOUT;
    let mut previous = String::new();
    let mut stable_rounds = 0u32;
    loop {
        let value =
            http::post_json_value(&url, &headers, &body, None, Some(REQUEST_TIMEOUT_MS)).await?;
        let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            return Err(business_error(&value, "拉取回复失败", "上游返回错误"));
        }
        if let Some(text) = reply_text_for(&value, &round.message_id) {
            // 上游不给「已结束」标志：同一条 reply 连续两轮内容不再变化即视为说完
            // （流式回复期间它每次都会变长；停下来了就是说完）。
            if text == previous {
                stable_rounds += 1;
                if stable_rounds >= 2 && !text.is_empty() {
                    return Ok(text);
                }
            } else {
                stable_rounds = 0;
                previous = text;
            }
        }
        if std::time::Instant::now() >= deadline {
            if previous.is_empty() {
                return Err(GatewayError::with_status(
                    504,
                    "Coze 回复超时（180 秒内没有收到任何内容）——该模型可能繁忙或任务较重",
                ));
            }
            crate::server::logging::verbose(
                "[Coze]",
                &format!(
                    "回复超时，已收到 {} 字，按现状收尾",
                    previous.chars().count()
                ),
            );
            return Ok(previous);
        }
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}

/// 在 `message/list` 的返回里找**本条消息**的回复正文。
///
/// 判据是 `reply_to_message_id == 我们发的 message_id`（模块头说明过为什么
/// 不能取「最新一条 reply」）。`content` 是字符串化的 JSON，要二次解析。
fn reply_text_for(value: &Value, message_id: &str) -> Option<String> {
    let items = value
        .get("data")
        .and_then(|data| data.get("messages"))
        .and_then(Value::as_array)?;
    for item in items {
        if item.get("message_type").and_then(Value::as_str) != Some("reply") {
            continue;
        }
        let raw = item.get("content").and_then(Value::as_str).unwrap_or("");
        let Ok(parsed) = serde_json::from_str::<Value>(raw) else { continue };
        let reply = parsed
            .get("message_data")
            .and_then(|data| data.get("reply"));
        let Some(reply) = reply else { continue };
        let target = reply
            .get("reply_to_message_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !target.eq_ignore_ascii_case(message_id) {
            continue;
        }
        if let Some(text) = reply.get("content").and_then(Value::as_str) {
            return Some(text.to_string());
        }
    }
    None
}

/// 流式下发：把等到的回复切片写进 mpsc。
async fn drive_stream(
    pat: &str,
    round: Round,
    sender: &tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
) -> Result<(), GatewayError> {
    let client_model = round.client_model.clone();
    let created = crate::server::logging::now_ms() / 1000;
    let mut sent_any = false;
    // 先发一帧「角色」开头的空 delta（OpenAI 客户端据此建气泡）
    if !emit(sender, &role_frame(&client_model, created)).await {
        return Ok(());
    }
    // 拿到全文再切片（上游 reply 是整段返回，见 `poll_reply` 的说明）
    let reply = poll_reply(pat, &round).await?;
    let mut piece = String::new();
    for ch in reply.chars() {
        piece.push(ch);
        if piece.chars().count() >= STREAM_SLICE_CHARS {
            if !emit(sender, &content_frame(&client_model, created, &piece)).await {
                return Ok(());
            }
            sent_any = true;
            piece.clear();
        }
    }
    if !piece.is_empty() {
        if !emit(sender, &content_frame(&client_model, created, &piece)).await {
            return Ok(());
        }
        sent_any = true;
    }
    let _ = sent_any;
    let _ = emit(sender, &final_frame(&client_model, created, rough_usage(&round.prompt, &reply))).await;
    let _ = emit(sender, "data: [DONE]\n\n").await;
    Ok(())
}

/// 往 mpsc 写一帧；返回 false 表示对端已断开（调用方应停止）。
async fn emit(
    sender: &tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
    frame: &str,
) -> bool {
    sender.send(Ok(Bytes::from(frame.to_string()))).await.is_ok()
}

/// 一帧 OpenAI chunk（`delta` 内嵌到 `choices[0].delta`）。
fn chunk_frame(model: &str, created: i64, delta: Value, finish: Option<&str>) -> String {
    let mut choice = Map::new();
    choice.insert("index".to_string(), json!(0));
    choice.insert("delta".to_string(), delta);
    if let Some(reason) = finish {
        choice.insert("finish_reason".to_string(), json!(reason));
    }
    let body = json!({
        "id": format!("chatcmpl-coze-{created}"),
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [Value::Object(choice)],
    });
    format!("data: {body}\n\n")
}

fn role_frame(model: &str, created: i64) -> String {
    chunk_frame(model, created, json!({ "role": "assistant" }), None)
}

fn content_frame(model: &str, created: i64, text: &str) -> String {
    chunk_frame(model, created, json!({ "content": text }), None)
}

fn final_frame(model: &str, created: i64, usage: Value) -> String {
    let mut choice = Map::new();
    choice.insert("index".to_string(), json!(0));
    choice.insert("delta".to_string(), json!({}));
    choice.insert("finish_reason".to_string(), json!("stop"));
    let body = json!({
        "id": format!("chatcmpl-coze-{created}"),
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [Value::Object(choice)],
        "usage": usage,
    });
    format!("data: {body}\n\n")
}

/// 非流式回包（标准 chat.completion 形状）。
fn completion_body(model: &str, reply: &str, usage: Value) -> Value {
    let created = crate::server::logging::now_ms() / 1000;
    json!({
        "id": format!("chatcmpl-coze-{created}"),
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": reply },
            "finish_reason": "stop",
        }],
        "usage": usage,
    })
}

/// 把 OpenAI messages 拍平成一条 prompt（上游 `message/chat` 只收单条 content）。
///
/// 历史部分用 `[用户] …` / `[助手] …` 标注（与 KukuAI 同一口径）；
/// system 放最前当指令。
fn build_prompt(messages: Option<&Value>) -> String {
    let Some(Value::Array(items)) = messages else {
        return String::new();
    };
    let mut system_parts: Vec<String> = Vec::new();
    let mut turns: Vec<(String, String)> = Vec::new();
    for message in items {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_string();
        let Some(text) = message_text(message.get("content")) else { continue };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        match role.as_str() {
            "system" => system_parts.push(text.to_string()),
            "assistant" => turns.push(("助手".to_string(), text.to_string())),
            _ => turns.push(("用户".to_string(), text.to_string())),
        }
    }
    // 最后一条 user 就是本轮要发的内容；它之前的历史拍平进前缀
    let last_user = turns.iter().rposition(|(who, _)| who == "用户");
    let mut out = String::new();
    if !system_parts.is_empty() {
        out.push_str("[系统指令] ");
        out.push_str(&system_parts.join("\n"));
        out.push_str("\n\n");
    }
    if let Some(index) = last_user {
        let (history, current) = turns.split_at(index);
        if !history.is_empty() {
            out.push_str("[对话历史]\n");
            for (who, text) in history {
                out.push_str(&format!("[{who}] {text}\n"));
            }
            out.push('\n');
        }
        if let Some((_, text)) = current.first() {
            out.push_str(text);
        }
    } else {
        // 没有 user 消息（只有 system 或只有 assistant）：把全部内容发过去
        for (who, text) in &turns {
            out.push_str(&format!("[{who}] {text}\n"));
        }
        let trimmed = out.trim().to_string();
        return trimmed;
    }
    out.trim().to_string()
}

/// 粗估 usage（上游 claw **不返回 token 用量**，`message/list` 里没有这个字段）。
///
/// 口径：按「中文 1 字 ≈ 1 token、英文 4 字符 ≈ 1 token」的通用经验值折算，
/// 并在注释与字段名上如实标注为估算 —— 不如实标成 `estimated` 会让下游
/// 计费类客户端把它当真实用量记账。想换成真实值时，先确认上游哪个接口给
/// `usage`（`get_model_config` 只认 Cookie，未验证）。
fn rough_usage(prompt: &str, reply: &str) -> Value {
    let prompt_tokens = rough_tokens(prompt);
    let completion_tokens = rough_tokens(reply);
    json!({
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": prompt_tokens + completion_tokens,
        "estimated": true,
    })
}

/// 单段文本的粗估 token 数（中日韩字符按 1、其余按 4 字符 1 个）。
fn rough_tokens(text: &str) -> i64 {
    let mut wide = 0i64;
    let mut narrow = 0i64;
    for ch in text.chars() {
        if (ch as u32) > 0x2E80 {
            wide += 1;
        } else {
            narrow += 1;
        }
    }
    wide + (narrow + 3) / 4
}

/// 取一条消息的可读文本（content 字符串或 text 类型的部件数组）。
fn message_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
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

/// 上游业务错误 → 网关错误（401 家族归成登录失效，其余带原文）。
fn business_error(value: &Value, what: &str, hint: &str) -> GatewayError {
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    let message = value
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or("未知错误")
        .to_string();
    if code == 700012006 || message.contains("登录") || message.contains("认证") {
        return GatewayError::with_status(
            401,
            format!("Coze {what}（code={code}：{message}）——请重新登录该账号"),
        );
    }
    GatewayError::with_status(502, format!("Coze {what}（code={code}：{message}）{hint}"))
}
