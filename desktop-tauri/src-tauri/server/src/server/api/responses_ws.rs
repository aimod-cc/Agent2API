//! `/v1/responses` 的 WebSocket 入口（Responses API 的 WS 传输）。
//!
//! ── 与 POST 入口的差别只有三处 ──────────────────────────────
//!   1. 入站是一串 `response.create` 消息，而不是一个 HTTP 请求体；
//!   2. 回程是文本帧，而不是 `text/event-stream`；
//!   3. `previous_response_id` 只能靠**当前连接**自己维护的历史补齐。
//!
//! 协议翻译、模型解析、账号选路、在途记账、断线兜底、取消登记全在
//! [`responses_endpoint`] 里，所以本模块不复写那条链路：把消息整理成 POST
//! 的请求体，调用同一个 handler，再把它产出的 SSE 帧解出 `data:` 载荷按
//! 文本帧下发。事件内容因此与 HTTP 流式逐字一致（含 `sequence_number`）。
//!
//! ── 为什么 `previous_response_id` 必须由连接内状态兜住 ────────
//! 网关本身无状态（上游是各家第三方服务，没有哪家实现 Responses 的服务端
//! 状态），`core::protocol::responses` 对这类字段直接报 400。WS 的核心用法
//! 恰恰是「增量 input + 上一轮 id」，所以这里按 `stream_id` 记一份累积的
//! input/output 项与最近一次 response id：命中就把历史拼到本轮 `input` 前面，
//! 再交给无状态的 POST 链路；未命中回 `previous_response_not_found` error
//! 事件（Codex 把它当可重试错误，随后改发完整上下文），不断连。
//!
//! ── `generate: false` 预热 ──────────────────────────────────
//! Codex 打开 WS 后会先发一轮预热：带齐 instructions 与 tools、不要求生成，
//! 拿回一个 response id，下一轮再用它当 `previous_response_id` 只发增量。
//! 上游都是 Chat 接口，没有「只准备不生成」的能力，转发出去会真的生成一段
//! 回复（白花额度，还会把一条凭空的 assistant 消息塞进历史）。所以预热在
//! 本模块就地应答：不碰上游，网关自己发 `resp_` 号，回 `response.created`
//! 与空 output 的 `response.completed`，本轮 input 记进历史。Codex 之后的
//! 每一轮都会重发 tools / instructions（只有 input 走增量），历史里只存
//! 对话项就够。
//!
//! ── 错误事件的形状 ──────────────────────────────────────────
//! 与官方 WS 一致：`{"type":"error","status":<HTTP 码>,"error":{type,code,
//! message,param}}`。Codex 只认带 `status` 的包装错误，扁平的
//! `{"type":"error","message":..}` 会被它忽略，一直等到空闲超时。
//!
//! ── P1 的并发模型 ──────────────────────────────────────────
//! 同一连接上的 `response.create` 串行处理：一条流收完才读下一条消息。
//! `stream_id` 的并行复用（每流一个任务 + 写出汇聚）留到下一阶段 ——
//! Codex 的单会话本来就是串行的，先解决握手失败与多轮上下文这两件主要事。

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::{json, Value};

use crate::server::config::model_key_source::Source;
use crate::server::core::key_scope::KeyScope;
use crate::server::core::protocol::responses::{response_envelope, response_event, usage_to_responses};
use crate::server::core::protocol::{random_id, SseLineBuffer};
use crate::server::logging;
use crate::server::request_stats::ApiKeyIdentity;
use crate::server::ServerState;

use super::protocol::responses_endpoint;

/// 客户端没带 `stream_id` 时的缺省槽位（单会话客户端的常见形态）。
const DEFAULT_STREAM_ID: &str = "default";

/// 错误响应体的读取上限：网关的错误体只有一行 JSON，64KB 足够。
const MAX_ERROR_BODY: usize = 64 * 1024;

/// 续轮引用的 response id 不在本连接时的错误码（与官方一致，Codex 据此重发完整上下文）。
const PREVIOUS_RESPONSE_NOT_FOUND: &str = "previous_response_not_found";

/// 一条连接内某个 `stream_id` 的会话状态。
#[derive(Default)]
struct StreamState {
    /// 累积的对话项：已发出过的 input 与已完成回合的 output，按时间顺序。
    history: Vec<Value>,
    /// 最近一次完成的 response id，`previous_response_id` 的匹配键。
    last_response_id: Option<String>,
}

/// `GET /v1/responses`（带 `Upgrade: websocket`）。
///
/// 鉴权由 `require_api_key` 在握手前完成（`/v1/*` 分支），未通过不会走到这里。
/// 可见性跟随参数里的 `model_key_source::Source`（`pub(crate)`）：本入口只在
/// 同 crate 的路由表里挂载。
pub(crate) async fn responses_ws(
    State(state): State<ServerState>,
    headers: HeaderMap,
    // 与 responses_endpoint 同一组扩展：中间件在握手请求上已放好
    key_scope: Option<Extension<KeyScope>>,
    key_identity: Option<Extension<ApiKeyIdentity>>,
    // 与 POST 入口同一个扩展：本次请求命中的 Key 来源（网关 Key / 环境变量 Key），
    // 记账按它区分用量归属
    key_source: Option<Extension<Arc<Source>>>,
    // axum 没给 `WebSocketUpgrade` 实现 `OptionalFromRequestParts`，用
    // `Result<T, T::Rejection>` 才能在 handler 里自己决定拒绝形态。
    upgrade: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    // 普通 GET（没有 Upgrade 头）不是本入口的用法：明确回 426，而不是走 404。
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(_) => return (StatusCode::UPGRADE_REQUIRED, "需要 WebSocket 升级请求").into_response(),
    };
    upgrade.on_upgrade(move |socket| serve(socket, state, headers, key_scope, key_identity, key_source))
}

/// 连接主循环：读消息 → 处理 → 直到客户端关闭或写失败。
async fn serve(
    mut socket: WebSocket,
    state: ServerState,
    headers: HeaderMap,
    scope: Option<Extension<KeyScope>>,
    identity: Option<Extension<ApiKeyIdentity>>,
    source: Option<Extension<Arc<Source>>>,
) {
    let mut streams: HashMap<String, StreamState> = HashMap::new();
    while let Some(message) = socket.recv().await {
        let text = match message {
            Ok(Message::Text(text)) => text.to_string(),
            // 少数客户端把 JSON 放二进制帧：同样按文本解析
            Ok(Message::Binary(bytes)) => match String::from_utf8(bytes.to_vec()) {
                Ok(text) => text,
                Err(_) => continue,
            },
            Ok(Message::Close(_)) | Err(_) => break,
            // Ping / Pong 由底层回，不需要业务处理
            Ok(_) => continue,
        };
        let keep = handle_message(
            &mut socket,
            &state,
            &headers,
            scope.clone(),
            identity.clone(),
            source.clone(),
            &mut streams,
            &text,
        )
        .await;
        if !keep {
            break;
        }
    }
}

/// 处理一条客户端消息。返回 `false` = 写端已不可用，连接该收尾了。
async fn handle_message(
    socket: &mut WebSocket,
    state: &ServerState,
    headers: &HeaderMap,
    scope: Option<Extension<KeyScope>>,
    identity: Option<Extension<ApiKeyIdentity>>,
    key_source: Option<Extension<Arc<Source>>>,
    streams: &mut HashMap<String, StreamState>,
    text: &str,
) -> bool {
    let Ok(mut payload) = serde_json::from_str::<Value>(text) else {
        return send_error(socket, DEFAULT_STREAM_ID, 400, "invalid_request_error", None, "消息不是 JSON 对象", None).await;
    };
    if !payload.is_object() {
        return send_error(socket, DEFAULT_STREAM_ID, 400, "invalid_request_error", None, "消息不是 JSON 对象", None).await;
    }
    let kind = payload.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
    if kind != "response.create" {
        let stream_id = stream_id_of(&payload).to_string();
        let message = format!("不支持的消息类型: {kind}");
        return send_error(socket, &stream_id, 400, "invalid_request_error", None, &message, Some("type")).await;
    }
    let stream_id = stream_id_of(&payload).to_string();
    let previous = payload
        .get("previous_response_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(|id| id.to_string());

    // 续轮只认本连接的历史：网关不在任何地方持久化响应。
    if let Some(previous) = previous.as_deref() {
        let matched = streams
            .get(&stream_id)
            .and_then(|entry| entry.last_response_id.as_deref())
            == Some(previous);
        if !matched {
            let message = format!("previous_response_id 不在本连接的历史中: {previous}");
            return send_error(
                socket,
                &stream_id,
                400,
                "invalid_request_error",
                Some(PREVIOUS_RESPONSE_NOT_FOUND),
                &message,
                Some("previous_response_id"),
            )
            .await;
        }
    }

    // 命中续轮才拼历史；不带 previous_response_id 视为一轮全新的对话。
    let mut history = if previous.is_some() {
        streams.get(&stream_id).map(|entry| entry.history.clone()).unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut input_items: Vec<Value> = Vec::new();
    append_input(&mut input_items, payload.get("input"));

    // 预热：就地应答，不转发上游（理由见模块头）。
    if is_warmup(&payload) {
        let response_id = random_id("resp");
        let keep = send_warmup(socket, &stream_id, &response_id, &payload).await;
        let entry = streams.entry(stream_id.clone()).or_default();
        if previous.is_none() {
            entry.history.clear();
        }
        entry.history.extend(input_items);
        entry.last_response_id = Some(response_id);
        logging::verbose(
            "[ResponsesWS]",
            &format!("stream={stream_id} 预热就地应答 history_items={}", entry.history.len()),
        );
        return keep;
    }

    history.extend(input_items.iter().cloned());

    if let Some(map) = payload.as_object_mut() {
        // `stream_id` 是 WS 信封字段，不能带进上游请求体；
        // `previous_response_id` 已由本连接消化，留给协议层会被判 400；
        // `stream` 在 WS 上隐含为真 —— POST 链路据此走 SSE 分支；
        // `generate` 只在预热时有意义，走到这里就是正常生成。
        map.remove("stream_id");
        map.remove("previous_response_id");
        map.remove("generate");
        map.insert("stream".to_string(), Value::Bool(true));
        if !history.is_empty() {
            map.insert("input".to_string(), Value::Array(history));
        }
    }

    let body = match serde_json::to_vec(&payload) {
        Ok(body) => Bytes::from(body),
        Err(_) => return send_error(socket, &stream_id, 500, "server_error", None, "请求体序列化失败", None).await,
    };
    let response = responses_endpoint(State(state.clone()), headers.clone(), scope, identity, key_source, body).await;

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if !content_type.starts_with("text/event-stream") {
        // 早失败（模型校验、协议翻译、上游错误）走的是 JSON 响应：
        // 把它的状态码与错误体原样折进包装错误事件。
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), MAX_ERROR_BODY)
            .await
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes).to_string();
        let parsed = serde_json::from_str::<Value>(&text).ok();
        let field = |key: &str| {
            parsed
                .as_ref()
                .and_then(|value| value.pointer(&format!("/error/{key}")))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let message = field("message").unwrap_or_else(|| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                format!("请求失败（HTTP {}）", status.as_u16())
            } else {
                trimmed.to_string()
            }
        });
        let error_type = field("type").unwrap_or_else(|| error_type_for(status.as_u16()).to_string());
        let code = field("code");
        return send_error(
            socket,
            &stream_id,
            status.as_u16(),
            &error_type,
            code.as_deref(),
            &message,
            None,
        )
        .await;
    }

    // SSE 帧 → 文本帧：只取 `data:` 载荷，`event:` 行在 JSON 的 type 字段里已有。
    let mut stream = response.into_body().into_data_stream();
    let mut buffer = SseLineBuffer::new();
    let mut response_id: Option<String> = None;
    let mut output: Option<Vec<Value>> = None;
    let mut keep = true;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            break;
        };
        for frame in buffer.push(&chunk) {
            let Some(data) = frame else { continue };
            let Ok(value) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            observe_event(&value, &mut response_id, &mut output);
            if !send_event(socket, &stream_id, value).await {
                keep = false;
                break;
            }
        }
        if !keep {
            break;
        }
    }
    if keep {
        for frame in buffer.finish() {
            let Some(data) = frame else { continue };
            let Ok(value) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            observe_event(&value, &mut response_id, &mut output);
            if !send_event(socket, &stream_id, value).await {
                keep = false;
                break;
            }
        }
    }

    // 只有跑完的回合才进历史：失败 / 中途断开会话不产生可续的上一轮。
    let entry = streams.entry(stream_id.clone()).or_default();
    entry.history.extend(input_items);
    if let Some(output) = output {
        entry.history.extend(output);
    }
    if let Some(id) = response_id {
        entry.last_response_id = Some(id);
    }
    logging::verbose(
        "[ResponsesWS]",
        &format!(
            "stream={stream_id} history_items={} last_response={}",
            entry.history.len(),
            entry.last_response_id.as_deref().unwrap_or("-"),
        ),
    );
    keep
}

/// 从请求里取 `stream_id`。
fn stream_id_of(payload: &Value) -> &str {
    payload
        .get("stream_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(DEFAULT_STREAM_ID)
}

/// 把 Responses 的 `input`（字符串 / 数组 / 单个对象）追加成项数组。
///
/// 与 `core::protocol::responses::messages_from_input` 接受的三种形状对齐：
/// 字符串视为一条 user 消息，数组逐项展开，单个对象直接入列。
fn append_input(items: &mut Vec<Value>, input: Option<&Value>) {
    let Some(input) = input else {
        return;
    };
    if let Some(text) = input.as_str() {
        items.push(json!({ "role": "user", "content": text }));
        return;
    }
    if let Some(array) = input.as_array() {
        items.extend(array.iter().cloned());
        return;
    }
    if input.is_object() {
        items.push(input.clone());
    }
}

/// 从事件里记下这一轮的 response id 与最终 output。
///
/// 只认 `response.created` / `response.completed` / `response.incomplete`：
/// `response.failed` 的回合不能作为下一轮的续写起点。
fn observe_event(value: &Value, response_id: &mut Option<String>, output: &mut Option<Vec<Value>>) {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
    if kind == "response.created" || kind == "response.completed" || kind == "response.incomplete" {
        if let Some(id) = value.pointer("/response/id").and_then(Value::as_str) {
            *response_id = Some(id.to_string());
        }
    }
    if matches!(kind, "response.completed" | "response.incomplete") {
        if let Some(items) = value.pointer("/response/output").and_then(Value::as_array) {
            *output = Some(items.clone());
        }
    }
}

/// 下发一个事件帧；`stream_id` 不是缺省槽位时按协议回填信封字段。
async fn send_event(socket: &mut WebSocket, stream_id: &str, mut value: Value) -> bool {
    if stream_id != DEFAULT_STREAM_ID {
        if let Some(map) = value.as_object_mut() {
            map.insert("stream_id".to_string(), Value::String(stream_id.to_string()));
        }
    }
    socket.send(Message::Text(value.to_string().into())).await.is_ok()
}

/// 是不是 `generate: false` 预热（只认显式的 false，缺省就是正常生成）。
fn is_warmup(payload: &Value) -> bool {
    payload.get("generate").and_then(Value::as_bool) == Some(false)
}

/// 预热的就地应答：`response.created` + 空 output 的 `response.completed`。
///
/// 信封走与真实回合同一个构造器（请求侧字段如实回显），`sequence_number`
/// 从 0 起递增 —— 客户端看到的是一轮合法但没有产出的 Responses 回合。
async fn send_warmup(socket: &mut WebSocket, stream_id: &str, response_id: &str, request: &Value) -> bool {
    let model = request.get("model").and_then(Value::as_str).unwrap_or_default();
    let created = logging::now_ms() / 1000;
    let mut sequence = 0;
    let in_progress = response_envelope(response_id, model, "in_progress", Vec::new(), Value::Null, request, created, None);
    let completed = response_envelope(response_id, model, "completed", Vec::new(), usage_to_responses(None), request, created, None);
    for (event, response) in [("response.created", in_progress), ("response.completed", completed)] {
        let frame = response_event(&mut sequence, event, json!({ "response": response }));
        let Some(value) = frame_payload(&frame) else {
            continue;
        };
        if !send_event(socket, stream_id, value).await {
            return false;
        }
    }
    true
}

/// 从 `event: ..\ndata: {..}\n\n` 形态的帧里取出 `data` 那一段 JSON。
fn frame_payload(frame: &[u8]) -> Option<Value> {
    let mut buffer = SseLineBuffer::new();
    let mut frames = buffer.push(frame);
    frames.extend(buffer.finish());
    frames
        .into_iter()
        .flatten()
        .find_map(|data| serde_json::from_str::<Value>(&data).ok())
}

/// HTTP 状态码 → 官方错误分类（错误体里没带 `type` 时的回退）。
fn error_type_for(status: u16) -> &'static str {
    match status {
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        400..=499 => "invalid_request_error",
        _ => "server_error",
    }
}

/// 组装官方形状的包装错误事件。
fn error_event(status: u16, error_type: &str, code: Option<&str>, message: &str, param: Option<&str>) -> Value {
    json!({
        "type": "error",
        "status": status,
        "error": {
            "type": error_type,
            "code": code,
            "message": message,
            "param": param,
        },
    })
}

/// 下发一条包装错误事件（形状见模块头）。
async fn send_error(
    socket: &mut WebSocket,
    stream_id: &str,
    status: u16,
    error_type: &str,
    code: Option<&str>,
    message: &str,
    param: Option<&str>,
) -> bool {
    logging::verbose("[ResponsesWS]", &format!("error {status}: {message}"));
    send_event(socket, stream_id, error_event(status, error_type, code, message, param)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_string_becomes_user_message() {
        let mut items = Vec::new();
        append_input(&mut items, Some(&json!("你好")));
        assert_eq!(items, vec![json!({ "role": "user", "content": "你好" })]);
    }

    #[test]
    fn input_array_is_flattened() {
        let mut items = Vec::new();
        append_input(
            &mut items,
            Some(&json!([
                { "role": "user", "content": "a" },
                { "type": "function_call_output", "call_id": "c1", "output": "ok" }
            ])),
        );
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn input_object_is_appended() {
        let mut items = Vec::new();
        append_input(&mut items, Some(&json!({ "role": "user", "content": "x" })));
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn missing_input_appends_nothing() {
        let mut items = Vec::new();
        append_input(&mut items, None);
        assert!(items.is_empty());
    }

    #[test]
    fn observe_keeps_id_and_output() {
        let mut id = None;
        let mut output = None;
        observe_event(
            &json!({ "type": "response.created", "response": { "id": "resp_1" } }),
            &mut id,
            &mut output,
        );
        assert_eq!(id.as_deref(), Some("resp_1"));
        assert!(output.is_none());

        observe_event(
            &json!({ "type": "response.completed", "response": { "id": "resp_1", "output": [{ "type": "message" }] } }),
            &mut id,
            &mut output,
        );
        assert_eq!(output.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn observe_ignores_failed_turns() {
        let mut id = None;
        let mut output = None;
        observe_event(
            &json!({ "type": "response.failed", "response": { "id": "resp_bad", "output": [] } }),
            &mut id,
            &mut output,
        );
        assert!(id.is_none());
        assert!(output.is_none());
    }

    #[test]
    fn stream_id_falls_back_to_default() {
        assert_eq!(stream_id_of(&json!({})), DEFAULT_STREAM_ID);
        assert_eq!(stream_id_of(&json!({ "stream_id": "" })), DEFAULT_STREAM_ID);
        assert_eq!(stream_id_of(&json!({ "stream_id": "s1" })), "s1");
    }

    #[test]
    fn warmup_only_when_generate_is_explicit_false() {
        assert!(is_warmup(&json!({ "generate": false })));
        assert!(!is_warmup(&json!({ "generate": true })));
        assert!(!is_warmup(&json!({})));
        assert!(!is_warmup(&json!({ "generate": "false" })));
    }

    #[test]
    fn error_event_matches_codex_wrapped_shape() {
        let event = error_event(
            400,
            "invalid_request_error",
            Some(PREVIOUS_RESPONSE_NOT_FOUND),
            "not found",
            Some("previous_response_id"),
        );
        assert_eq!(event["type"], "error");
        assert_eq!(event["status"], 400);
        assert_eq!(event["error"]["code"], PREVIOUS_RESPONSE_NOT_FOUND);
        assert_eq!(event["error"]["param"], "previous_response_id");
        assert_eq!(event["error"]["message"], "not found");
    }

    #[test]
    fn error_type_follows_status() {
        assert_eq!(error_type_for(401), "authentication_error");
        assert_eq!(error_type_for(429), "rate_limit_error");
        assert_eq!(error_type_for(400), "invalid_request_error");
        assert_eq!(error_type_for(502), "server_error");
    }

    #[test]
    fn frame_payload_extracts_data_json() {
        let mut sequence = 0;
        let frame = response_event(&mut sequence, "response.created", json!({ "response": { "id": "resp_x" } }));
        let value = frame_payload(&frame).expect("payload");
        assert_eq!(value["type"], "response.created");
        assert_eq!(value["sequence_number"], 0);
        assert_eq!(value["response"]["id"], "resp_x");
    }
}
