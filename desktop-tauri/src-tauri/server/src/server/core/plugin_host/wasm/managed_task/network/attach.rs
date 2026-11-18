//! 单次固定attach；socket不离开原worker，任何结束路径直接drop，不等关闭握手。

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use reqwest::{
    header::{HeaderMap, HeaderValue, ORIGIN, USER_AGENT},
    Method, Request, Version,
};
use tokio::time::Instant;
use tokio_tungstenite::{
    tungstenite::{
        handshake::{client::generate_key, derive_accept_key},
        protocol::{Role, WebSocketConfig},
        Message,
    },
    WebSocketStream,
};

use super::super::{
    observe::{self, MessageEvent, StreamFailure},
    policy, registry,
};
use super::{
    observation::{ObservationEnd, ObservationFailure, Observer, TextChunk, HEARTBEAT_TIMEOUT},
    wire::CountedUpgrade,
    Session, WitnessedCreation,
};

type Socket = WebSocketStream<CountedUpgrade>;

pub(super) async fn run(
    session: &Session,
    observer: &mut Observer<'_>,
) -> Result<ObservationEnd, ObservationFailure> {
    // 就绪等待只用原headers窗口；模型idle从这唯一一次就绪转换开始，不按read重置。
    let ready_at = Instant::now();
    let mut idle = observer.window.total.min(ready_at + observer.window.idle);
    let handshake = observer
        .window
        .total
        .min(ready_at + observer.window.connect)
        .min(idle);
    let handshake_timeout = if handshake == idle {
        ObservationFailure::IdleTimeout
    } else {
        ObservationFailure::HandshakeTimeout
    };
    let (mut socket, _read) = observer
        .wait(handshake, handshake_timeout, async {
            let ((request, key), lease) =
                registry::observation_read(observer.record, observer.permit, true, |creation| {
                    request(session, creation)
                })?;
            // 只有此次registry确认后的固定请求可发送；Session完整冻结租约仍在外层栈。
            let client = session
                .frozen
                .client()
                .map_err(|_| ObservationFailure::Unavailable)?;
            let response = observer
                .wait(handshake, handshake_timeout, async {
                    client
                        .execute(request)
                        .await
                        .map_err(|_| ObservationFailure::Transport)
                })
                .await?;
            observer.check(handshake, handshake_timeout)?;
            validate_upgrade(&response, &key)?;
            let upgraded = observer
                .wait(handshake, handshake_timeout, async {
                    response
                        .upgrade()
                        .await
                        .map_err(|_| ObservationFailure::Handshake)
                })
                .await?;
            let config = WebSocketConfig::default()
                .read_buffer_size(16 * 1024)
                .write_buffer_size(1024)
                .max_write_buffer_size(4096)
                .max_message_size(Some(observe::MAX_MESSAGE_BYTES))
                .max_frame_size(Some(observe::MAX_MESSAGE_BYTES));
            observer.check(handshake, handshake_timeout)?;
            let socket = WebSocketStream::from_raw_socket(
                CountedUpgrade::new(upgraded, observer.record, observer.output),
                Role::Client,
                Some(config),
            )
            .await;
            Ok((socket, lease))
        })
        .await?;
    let deadline = idle.min(Instant::now() + HEARTBEAT_TIMEOUT);
    observer.wait(deadline, ObservationFailure::HeartbeatTimeout, async {
        socket.send(Message::text(r#"{"type":"auto-approve"}"#)).await
            .map_err(|_| ObservationFailure::Transport)
    }).await?;
    let mut interactions = 0usize;
    loop {
        observer.messages_available()?;
        // 一次next及其碎片重组也有固定绝对截止；底层分段读取会主动让出poll机会。
        socket.get_mut().set_deadline(idle);
        let message = observer
            .wait(idle, ObservationFailure::IdleTimeout, async {
                match socket.next().await {
                    Some(Ok(message)) => Ok(message),
                    Some(Err(error)) => Err(socket_failure(&socket, error)),
                    None => Err(ObservationFailure::StreamClosed),
                }
            })
            .await?;
        // 先记账再分类；ping/pong、未知载荷、close也不返还消息机会。
        observer.count_message()?;
        match message {
            Message::Text(text) => {
                let observed =
                    observe::parse_message(text.as_bytes()).map_err(ObservationFailure::Invalid)?;
                observer.check(idle, ObservationFailure::IdleTimeout)?;
                let permission_request = matches!(&observed.event, MessageEvent::PermissionRequest(_));
                match observed.event {
                    MessageEvent::Text { kind, text } => {
                        if !text.is_empty() {
                            idle = observer
                                .window
                                .total
                                .min(Instant::now() + observer.window.idle);
                            observer.emit(TextChunk::Text { kind, text }, idle).await?;
                        }
                    }
                    MessageEvent::Tool(tool) => {
                        let failed = tool.failed();
                        // 只读工具事件仍计消息/投影字节并走原背压；不把工具活动当模型文本进展。
                        observer.emit(TextChunk::Tool(tool), idle).await?;
                        if failed {
                            // 先保留真实失败观察值，随后终结；不能让后面的task-ended洗成正常完成。
                            return Err(ObservationFailure::StreamFailed(
                                StreamFailure::ToolFailed,
                            ));
                        }
                    }
                    MessageEvent::Ping => {
                        let deadline = idle.min(Instant::now() + HEARTBEAT_TIMEOUT);
                        let timeout = if deadline == idle {
                            ObservationFailure::IdleTimeout
                        } else {
                            ObservationFailure::HeartbeatTimeout
                        };
                        socket.get_mut().set_deadline(deadline);
                        // 唯一应用层上行是宿主固定心跳，不接受任意字符串/ACP/批准/回答。
                        observer
                            .wait(deadline, timeout, async {
                                socket
                                    .send(Message::text(r#"{"type":"ping"}"#))
                                    .await
                                    .map_err(|_| ObservationFailure::Transport)
                            })
                            .await?;
                    }
                    MessageEvent::RoundEnded => return Ok(ObservationEnd::RoundEnded),
                    MessageEvent::Failed(failure) => {
                        return Err(ObservationFailure::StreamFailed(failure))
                    }
                    MessageEvent::PermissionRequest(id) | MessageEvent::QuestionRequest(id) => {
                        interactions += 1;
                        if interactions > 256 { return Err(ObservationFailure::UnexpectedFrame); }
                        use base64::Engine;
                        let permission = permission_request;
                        let payload = if permission {
                            serde_json::json!({"id": id, "approved": true, "remember": false, "persist": false})
                        } else {
                            serde_json::json!({"request_id": id, "answers_json": "", "cancelled": false})
                        };
                        let frame = serde_json::json!({
                            "type": if permission { "permission-resp" } else { "reply-question" },
                            "data": base64::engine::general_purpose::STANDARD.encode(payload.to_string()),
                            "timestamp": crate::server::logging::now_ms(),
                        });
                        let deadline = idle.min(Instant::now() + HEARTBEAT_TIMEOUT);
                        observer.wait(deadline, ObservationFailure::HeartbeatTimeout, async {
                            socket.send(Message::text(frame.to_string())).await.map_err(|_| ObservationFailure::Transport)
                        }).await?;
                    }
                    MessageEvent::NeedsInteraction(interaction) => {
                        return Ok(ObservationEnd::NeedsInteraction(interaction))
                    }
                    MessageEvent::Started
                    | MessageEvent::Cursor { .. }
                    | MessageEvent::Unknown(_) => {}
                }
            }
            Message::Ping(_) => {
                let deadline = idle.min(Instant::now() + HEARTBEAT_TIMEOUT);
                let timeout = if deadline == idle {
                    ObservationFailure::IdleTimeout
                } else {
                    ObservationFailure::HeartbeatTimeout
                };
                socket.get_mut().set_deadline(deadline);
                // tungstenite只排队对应Pong；本层立刻有界flush，不让它在下一次next中无限等写。
                observer
                    .wait(deadline, timeout, async {
                        socket
                            .flush()
                            .await
                            .map_err(|_| ObservationFailure::Transport)
                    })
                    .await?;
            }
            Message::Pong(_) => {}
            Message::Close(_) => return Err(ObservationFailure::StreamClosed),
            Message::Binary(_) | Message::Frame(_) => {
                return Err(ObservationFailure::UnexpectedFrame)
            }
        }
        observer
            .wait(idle, ObservationFailure::IdleTimeout, async {
                tokio::task::yield_now().await;
                Ok(())
            })
            .await?;
    }
}

fn request(
    session: &Session,
    creation: &WitnessedCreation,
) -> Result<(Request, String), ObservationFailure> {
    if creation.binding() != session.binding.as_ref()
        || !Arc::ptr_eq(&creation.session, &session.identity)
    {
        return Err(ObservationFailure::Unavailable);
    }
    let route = creation.parsed.stream_route(session.binding.provider()).map_err(|_| ObservationFailure::Unavailable)?;
    let mut url = url::Url::parse(route.url()).map_err(|_| ObservationFailure::Unavailable)?;
    if url.scheme() != "wss" || url.host_str() != policy::hostname(session.binding.provider()) {
        return Err(ObservationFailure::Unavailable);
    }
    url.set_scheme("https")
        .map_err(|_| ObservationFailure::Unavailable)?;
    let key = generate_key();
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, session.user_agent.header());
    headers.insert(ORIGIN, HeaderValue::from_static(policy::origin(session.binding.provider()).ok_or(ObservationFailure::Unavailable)?));
    headers.insert(
        "accept-language",
        HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.8"),
    );
    headers.insert("cache-control", HeaderValue::from_static("no-cache"));
    headers.insert("pragma", HeaderValue::from_static("no-cache"));
    headers.insert("connection", HeaderValue::from_static("Upgrade"));
    headers.insert("upgrade", HeaderValue::from_static("websocket"));
    headers.insert("sec-websocket-version", HeaderValue::from_static("13"));
    headers.insert(
        "sec-websocket-key",
        HeaderValue::from_str(&key).map_err(|_| ObservationFailure::Unavailable)?,
    );
    session.cookie.insert_into(&mut headers);
    let mut request = Request::new(Method::GET, url);
    *request.version_mut() = Version::HTTP_11;
    *request.headers_mut() = headers;
    Ok((request, key))
}

fn validate_upgrade(response: &reqwest::Response, key: &str) -> Result<(), ObservationFailure> {
    if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS
        || response.version() != Version::HTTP_11
    {
        return Err(ObservationFailure::Handshake);
    }
    let headers = response.headers();
    // 合并前的重复头也拒绝；不接受重复单值头、歧义Connection或任何扩展/子协议。
    for name in headers.keys() {
        if headers.get_all(name).iter().count() != 1 {
            return Err(ObservationFailure::Handshake);
        }
    }
    if headers.contains_key("sec-websocket-extensions")
        || headers.contains_key("sec-websocket-protocol")
        || headers.contains_key("transfer-encoding")
        || headers.contains_key("content-length")
        || headers.contains_key("content-encoding")
    {
        return Err(ObservationFailure::Handshake);
    }
    let field = |name: &'static str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .ok_or(ObservationFailure::Handshake)
    };
    if !field("upgrade")?.eq_ignore_ascii_case("websocket")
        || field("sec-websocket-accept")? != derive_accept_key(key.as_bytes())
    {
        return Err(ObservationFailure::Handshake);
    }
    let mut upgrade = false;
    let mut keep_alive = false;
    for token in field("connection")?.split(',').map(str::trim) {
        if token.eq_ignore_ascii_case("upgrade") && !upgrade {
            upgrade = true;
        } else if token.eq_ignore_ascii_case("keep-alive") && !keep_alive {
            keep_alive = true;
        } else {
            return Err(ObservationFailure::Handshake);
        }
    }
    if !upgrade {
        return Err(ObservationFailure::Handshake);
    }
    Ok(())
}

fn socket_failure(
    socket: &Socket,
    error: tokio_tungstenite::tungstenite::Error,
) -> ObservationFailure {
    if socket.get_ref().exhausted() {
        ObservationFailure::UpgradedBytes
    } else if matches!(error, tokio_tungstenite::tungstenite::Error::Capacity(_)) {
        ObservationFailure::MessageBytes
    } else {
        ObservationFailure::Transport
    }
}
