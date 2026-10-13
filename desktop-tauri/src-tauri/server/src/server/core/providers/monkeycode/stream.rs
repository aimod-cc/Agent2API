//! MonkeyCode 任务流（WebSocket）的连接与驱动（会话转发的第二步）。
//!
//! 下行消息与 ACP 事件 → OpenAI 帧的翻译在 `translate.rs`；本文件管：
//! 握手（URL / 头 / 超时）、起始消息（auto-approve + user-input）、心跳回复的
//! 发送时机、空闲 / 总时长超时，以及流式 / 聚合两种出口的收尾。
//!
//! ── 上游长什么样（参考 `task-runner.ts` 与 `docs/04-websocket/*`）──────
//! ```text
//!   WS   wss://<站点>/api/v1/users/tasks/stream?id=<taskId>&mode=new
//!   连接成功后客户端**必须先发** auto-approve + user-input，上游才开始跑；
//!   下行是 {type, kind?, data?, timestamp?} 的 JSON 文本帧：
//!     ping                      心跳（客户端回一条 {"type":"ping"}）
//!     task-started              轮次开始
//!     task-running kind=acp_event            data 是 ACP 事件的 JSON 串
//!     task-running kind=acp_ask_user_question data 是提问的 JSON 串
//!     task-ended                轮次结束（终态；data 可能带最终 usage）
//!     task-error                轮次出错（终态）
//! ```
//!
//! ── 为什么必须有状态、为什么走 WS ──────────────────────────────
//! 一次客户端请求 = 「建任务 → 挂任务流 → 等轮次结束」，任务 id 由上游生成、
//! 事件由 VM 内的 coding agent 实时推送 —— 不是「一次 HTTP 请求一次回答」，
//! 单请求构造（`build_chat_request`）容纳不了这条链（`is_stateful` 为 true）。
//!
//! ── 终态怎么判（不挂死的关键）─────────────────────────────────
//! 四个出口，**每个都会给下游一个收尾**（`finish` 帧 + `[DONE]`，或错误帧）：
//!   1. `task-ended` → 正常收尾；
//!   2. `task-error` → 异常收尾（错误帧）；
//!   3. 连接被关 / 传输错误（没等到 task-ended）→ 异常收尾；
//!   4. 空闲超时（默认 300 秒没有任何下行消息；上游每 10 秒有 ping，
//!      所以空闲即断线）或总时长超过 1 小时（与 `resource.life` 对齐，
//!      参考的 `TASK_TIMEOUT_MS` 同值）→ 异常收尾。
//! 参考在这几处是「静默 resolve、靠客户端等 EOF」；本网关必须显式收尾 ——
//! 客户端的 SSE 解析器等的是 `[DONE]`，只断连接会被渲染成「回答被截断」。
//!
//! ── 自动批准 / 自动回复（MonkeyCode 能不能用的关键）──────────────
//!   - `auto-approve`：连接建立后立刻发一次（参考 `on("open")`）。Agent 的
//!     工具调用因此不再等用户确认 —— 不发这一条，任务会停在第一个工具调用；
//!   - Agent 提问（`kind=acp_ask_user_question`）→ 自动回 `reply-question`
//!     （`answers_json: ""` / `cancelled: false`，参考同款），任务继续走。
//!
//! ── 与参考的两处有意差异（都写在实现处）────────────────────────
//!   1. 思考块（`agent_thought_chunk`）走 `delta.reasoning_content` 通道
//!      （参考把它拼成 `[Thinking] …` 塞进正文）—— 本仓的思考通道约定见
//!      `catpaw::openai` / `trae::stream`，网关的 reasoning 合并与展示都认它；
//!   2. 错误一律显式收尾（见上），不学参考的「静默 resolve」。
//!
//! ── 与 `translate.rs` 的分工 ───────────────────────────────────
//! 本文件管连接与驱动（握手 / 起始消息 / 心跳 / 超时 / 帧出口）；ACP 事件
//! → OpenAI 帧的映射表与下行信封的处理在 `translate.rs`（纯状态机，不碰网络）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic；持锁不跨 await
//! （本文件不持任何锁）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};

use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::errors::GatewayError;
use crate::server::logging;

use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::credentials::MonkeyCodeCredentials;
use super::translate::{handle_message, Handled, Terminal, DONE_FRAME};
pub use super::translate::Translator;
use super::endpoints;
use super::region::Region;
use super::task;

/// 任务流连接类型（TLS 由 tokio-tungstenite 按 `wss` scheme 自动包装）
pub type TaskSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// 任务总时长上限：与建任务的 `resource.life`（1 小时）对齐。
///
/// 参考 `TASK_TIMEOUT_MS = 3600000`，注释原文「matches resource.life」——
/// 到点后上游 VM 本来也要回收，继续等只会挂死下游。
const TASK_TIMEOUT_MS: u64 = 60 * 60 * 1000;

/// 空闲兜底下限（配置值异常小的时候用；上游每 10 秒有 ping，正常不会触发）
const MIN_IDLE_MS: u64 = 30_000;

/// 已建连的任务流（`chat.rs` 装配后交给 `drive_stream` / `drive_aggregate`）。
pub struct TaskStream {
    socket: TaskSocket,
    pub task_id: String,
}

/// 连接任务流（`mode=new`：本请求新建任务轮次）。
///
/// ── 失败翻译 ────────────────────────────────────────────────
///   - 握手被拒：401/403 → 401（登录态失效，重新粘贴 session）；
///     其余状态 → 502（带上游状态码）；
///   - 连不上 / DNS / TLS / 握手超时 → 502 / 504。
///
/// ── 代理为什么不作用于 WS（如实说明）─────────────────────────
/// `proxy` 在编排层的语义是「账号级出网代理」；本家**只有建任务那一次 HTTP
/// 调用**经过它（reqwest 直连 egress 层）。任务流是 WebSocket，tokio-tungstenite
/// 没有代理支持，参考实现（Node `ws`）同样不给任务流挂代理 —— 因此这里与
/// 参考一致地直连，并在配了代理时留一条 verbose 记录，避免「以为走了代理」。
pub async fn connect(
    region: Region,
    credentials: &MonkeyCodeCredentials,
    task_id: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<TaskStream, GatewayError> {
    let url = endpoints::task_stream_url(region, task_id, "new");
    let mut request = url.as_str().into_client_request().map_err(|error| {
        GatewayError::with_status(502, format!("MonkeyCode 任务流地址无效（{url}）：{error}"))
    })?;
    for (name, value) in endpoints::ws_headers(region, &credentials.session) {
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            GatewayError::with_status(500, format!("MonkeyCode 请求头名非法：{name}"))
        })?;
        let header_value = HeaderValue::from_str(&value).map_err(|_| {
            GatewayError::with_status(500, format!("MonkeyCode 请求头 {name} 含非法字符"))
        })?;
        request.headers_mut().insert(header_name, header_value);
    }
    if proxy.is_some() {
        logging::verbose(
            "[MonkeyCode]",
            "任务流 WebSocket 直连（账号代理只作用于建任务调用，与参考实现一致）",
        );
    }
    let timeouts = crate::server::config::timeout_settings();
    let wait = Duration::from_millis(timeouts.connect_ms().clamp(5_000, 120_000));
    let (socket, _response) = tokio::time::timeout(
        wait,
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, None),
    )
    .await
    .map_err(|_| GatewayError::with_status(504, "MonkeyCode 任务流连接超时（WebSocket 握手未完成）"))?
    .map_err(connect_error)?;
    logging::verbose(
        "[MonkeyCode]",
        &format!("任务流已连接 task={task_id}（mode=new）"),
    );
    Ok(TaskStream {
        socket,
        task_id: task_id.to_string(),
    })
}

/// WS 连接 / 握手错误 → 网关错误。
fn connect_error(error: tokio_tungstenite::tungstenite::Error) -> GatewayError {
    use tokio_tungstenite::tungstenite::Error as WsError;
    match error {
        WsError::Http(response) => {
            let status = response.status().as_u16();
            if status == 401 || status == 403 {
                GatewayError::with_status(
                    401,
                    format!("MonkeyCode 任务流鉴权失败（HTTP {status}）：请在账号页重新粘贴 session"),
                )
            } else {
                GatewayError::with_status(
                    502,
                    format!("MonkeyCode 任务流握手被拒（HTTP {status}）"),
                )
            }
        }
        other => GatewayError::with_status(502, format!("MonkeyCode 任务流连接失败：{other}")),
    }
}

/// 连接建立后的起始交互（参考 `task-runner.ts` 的 `on("open")`）：
/// 先 `auto-approve`（工具调用自动批准），再 `user-input`（`mode=new` 下
/// **必须**由客户端先发它，上游才开始执行）。
pub async fn start_task(stream: &mut TaskStream, prompt: &str) -> Result<(), GatewayError> {
    let auto_approve = json!({ "type": "auto-approve" }).to_string();
    stream
        .socket
        .send(Message::text(auto_approve))
        .await
        .map_err(|error| send_error("auto-approve", error))?;
    let input = json!({
        "type": "user-input",
        "data": task::user_input_payload(prompt),
    })
    .to_string();
    stream
        .socket
        .send(Message::text(input))
        .await
        .map_err(|error| send_error("user-input", error))?;
    logging::verbose(
        "[MonkeyCode]",
        &format!("任务 {} 已发送 auto-approve + user-input", stream.task_id),
    );
    Ok(())
}

fn send_error(what: &str, error: tokio_tungstenite::tungstenite::Error) -> GatewayError {
    GatewayError::with_status(502, format!("MonkeyCode 任务流发送 {what} 失败：{error}"))
}


// ─── 驱动（流式 / 非流式）─────────────────────────────────────

/// 帧出口：流式走 mpsc（背压不丢帧），非流式只推进状态机。
enum Mode<'a> {
    Stream(&'a tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>),
    Collect,
}

impl Mode<'_> {
    fn is_stream(&self) -> bool {
        matches!(self, Mode::Stream(_))
    }
}

/// 流式驱动（挂在 `ForwardOutcome::Stream` 的后台任务上）。
///
/// 无返回值：所有失败都已转成客户端可见的流内错误帧（HTTP 头早发出去了）
/// 或请求日志的 `note_error`。客户端断开时停止消费并关闭 WS（参考的 abort
/// 分支同样只 close；本家不需要向 上游回报终态）。
pub async fn drive_stream(
    stream: TaskStream,
    translator: Translator,
    sender: tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
    telemetry: &Arc<RequestTelemetry>,
) {
    if let Err(error) = drive(stream, translator, Mode::Stream(&sender), telemetry).await {
        logging::verbose("[MonkeyCode]", &format!("任务流转发结束：{}", error.message));
    }
}

/// 非流式驱动：内部仍走同一条任务流，收尾时聚合成完整 `chat.completion`。
///
/// 与流式路径的关键差别：这里**还没下发任何字节**（HTTP 头都没发），所以
/// 失败可以带着状态码返回，由编排层与入口如实报给客户端。
pub async fn drive_aggregate(
    stream: TaskStream,
    translator: Translator,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<Value, GatewayError> {
    let translator = drive(stream, translator, Mode::Collect, telemetry).await?;
    Ok(translator.completion_body())
}

/// 任务流消费主循环（两种出口共用）。
async fn drive(
    mut stream: TaskStream,
    mut translator: Translator,
    mode: Mode<'_>,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<Translator, GatewayError> {
    let mut pending: Vec<Bytes> = Vec::new();
    if mode.is_stream() {
        if let Some(frame) = translator.role_frame() {
            pending.push(frame);
        }
        if !emit(&mode, &mut pending).await {
            let _ = stream.socket.close(None).await;
            return Err(GatewayError::with_status(499, "客户端已断开，任务流转发停止"));
        }
    }
    let idle = Duration::from_millis(
        crate::server::config::timeout_settings()
            .stream_idle_ms()
            .max(MIN_IDLE_MS),
    );
    let total = Duration::from_millis(TASK_TIMEOUT_MS);
    let started = Instant::now();
    // 循环是**表达式**：每个出口都用 break 带出终态，不存在「循环走完没终态」
    // 的路径（初始占位值是死代码，编译器会警告 —— 这也是这行的由来）
    let terminal = loop {
        if started.elapsed() >= total {
            break Terminal::Failed(
                "任务流超过 1 小时上限（与上游 resource.life 对齐）".to_string(),
            );
        }
        let remaining = total.saturating_sub(started.elapsed());
        let wait = idle.min(remaining);
        match tokio::time::timeout(wait, stream.socket.next()).await {
            Err(_) => {
                break if started.elapsed() >= total {
                    Terminal::Failed(
                        "任务流超过 1 小时上限（与上游 resource.life 对齐）".to_string(),
                    )
                } else {
                    Terminal::Failed(format!(
                        "任务流空闲超时（{} 秒没有任何下行消息）",
                        idle.as_secs()
                    ))
                };
            }
            Ok(None) => {
                break Terminal::Failed("任务流连接已关闭（未收到 task-ended）".to_string());
            }
            Ok(Some(Err(error))) => {
                break Terminal::Failed(format!("任务流传输中断：{error}"));
            }
            Ok(Some(Ok(message))) => {
                let handled = match message {
                    Message::Text(text) => handle_message(&mut translator, text.as_str()),
                    Message::Binary(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        handle_message(&mut translator, text.as_ref())
                    }
                    // 控制帧：ping/pong 由 tokio-tungstenite 处理（pong 的发送
                    // 会随下一次 send 刷出；上游的文本 ping 走上面的分支）
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Handled::default(),
                    Message::Close(_) => Handled {
                        terminal: Some(Terminal::Failed(
                            "任务流连接已关闭（未收到 task-ended）".to_string(),
                        )),
                        ..Handled::default()
                    },
                };
                pending.extend(handled.frames);
                if let Some(reply) = handled.reply {
                    if let Err(error) = stream.socket.send(Message::text(reply)).await {
                        break Terminal::Failed(format!("任务流回复失败：{error}"));
                    }
                }
                if let Some(next) = handled.terminal {
                    break next;
                }
            }
        }
        if !pending.is_empty() && !emit(&mode, &mut pending).await {
            // 客户端断开：它已经不收帧了，继续消费没有意义（参考 abort 分支同）
            let _ = stream.socket.close(None).await;
            return Err(GatewayError::with_status(499, "客户端已断开，任务流转发停止"));
        }
    };
    match terminal {
        Terminal::Ended => {
            if let Some(usage) = translator.reportable_usage() {
                telemetry.report_usage(&usage);
            }
            pending.extend(translator.finish_frames());
            let _ = emit(&mode, &mut pending).await;
            let _ = stream.socket.close(None).await;
            logging::verbose(
                "[MonkeyCode]",
                &format!(
                    "任务 {} 正常结束（{}）",
                    stream.task_id,
                    translator.summary(),
                ),
            );
            Ok(translator)
        }
        Terminal::Failed(reason) => {
            let message = format!("MonkeyCode {reason}");
            telemetry.note_error(&reason);
            logging::log("[MonkeyCode]", &format!("❌ {message}"));
            if mode.is_stream() {
                pending.push(translator.error_frame(&message));
                pending.push(Bytes::from_static(DONE_FRAME));
                let _ = emit(&mode, &mut pending).await;
            }
            let _ = stream.socket.close(None).await;
            Err(GatewayError::with_status(502, message))
        }
    }
}

/// 把待发帧交给下游；返回 false = 客户端已断开（非流式恒 true）。
async fn emit(mode: &Mode<'_>, pending: &mut Vec<Bytes>) -> bool {
    match mode {
        Mode::Collect => {
            pending.clear();
            true
        }
        Mode::Stream(sender) => {
            for frame in pending.drain(..) {
                if sender.send(Ok(frame)).await.is_err() {
                    return false;
                }
            }
            true
        }
    }
}
