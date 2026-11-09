use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use agent2api_provider_protocol as proto;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{
    handshake::{client::generate_key, derive_accept_key},
    protocol::{CloseFrame, Role, WebSocketConfig, frame::coding::CloseCode},
    Message,
};
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;

use crate::server::core::egress;

const MAX_MESSAGE: usize = 2 * 1024 * 1024;

type Socket = WebSocketStream<reqwest::Upgraded>;

struct Command {
    message: Message,
    reply: oneshot::Sender<Result<(), String>>,
}

struct Connection {
    commands: mpsc::Sender<Command>,
    events: tokio::sync::Mutex<mpsc::Receiver<Value>>,
    cancel: CancellationToken,
    touched: Mutex<Instant>,
}

fn connections() -> &'static Mutex<HashMap<String, Arc<Connection>>> {
    static ITEMS: OnceLock<Mutex<HashMap<String, Arc<Connection>>>> = OnceLock::new();
    ITEMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn error(message: impl Into<String>) -> proto::WireError {
    proto::WireError::with_code(message, proto::WireError::INTERNAL)
}

fn find(params: &Value) -> Result<Arc<Connection>, proto::WireError> {
    let connection = connections().lock().map_err(|_| error("脚本 WebSocket 会话不可用"))?
        .get(params["id"].as_str().unwrap_or("")).cloned()
        .ok_or_else(|| error("脚本 WebSocket 会话已结束"))?;
    if let Ok(mut time) = connection.touched.lock() { *time = Instant::now(); }
    Ok(connection)
}

struct Opening(Option<String>);

impl Drop for Opening {
    fn drop(&mut self) {
        if let Some(id) = &self.0 { abort(&json!({ "id": id })); }
    }
}

pub async fn open(params: &Value) -> Result<Value, proto::WireError> {
    let mut url = url::Url::parse(params["url"].as_str().unwrap_or(""))
        .map_err(|_| error("WebSocket 地址无效"))?;
    if !matches!(url.scheme(), "ws" | "wss") || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some()
    {
        return Err(error("WebSocket 只允许不含账号信息的 ws / wss 地址"));
    }
    let secure = url.scheme() == "wss";
    url.set_scheme(if secure { "https" } else { "http" }).map_err(|_| error("WebSocket 地址无效"))?;
    let proxy = super::transport::proxy_for(&url, params)?;
    let client = egress::websocket_client(proxy.as_ref()).map_err(error)?;
    let key = generate_key();
    let mut request = client.get(url).version(reqwest::Version::HTTP_11)
        .header("connection", "Upgrade").header("upgrade", "websocket")
        .header("sec-websocket-version", "13").header("sec-websocket-key", &key);
    if let Some(headers) = params["headers"].as_object() {
        if headers.len() > 128 { return Err(error("WebSocket 请求头过多")); }
        for (name, value) in headers {
            if matches!(name.to_ascii_lowercase().as_str(), "host" | "connection" | "upgrade"
                | "content-length" | "transfer-encoding" | "proxy-authorization"
                | "sec-websocket-key" | "sec-websocket-version" | "sec-websocket-protocol"
                | "sec-websocket-extensions") { continue; }
            if let Some(value) = value.as_str() { request = request.header(name, value); }
        }
    }
    let protocols: Vec<String> = match params.get("protocols") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) if items.len() <= 32 => items.iter().map(|item| {
            item.as_str().filter(|text| !text.is_empty() && text.len() <= 256
                && text.bytes().all(|ch| ch.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&ch)))
                .map(str::to_string).ok_or_else(|| error("WebSocket 子协议无效"))
        }).collect::<Result<_, _>>()?,
        _ => return Err(error("WebSocket 子协议无效")),
    };
    if protocols.iter().enumerate().any(|(index, item)| protocols[..index].contains(item)) {
        return Err(error("WebSocket 子协议重复"));
    }
    if !protocols.is_empty() { request = request.header("sec-websocket-protocol", protocols.join(", ")); }
    let id = params["id"].as_str().filter(|id| !id.is_empty() && id.len() <= 80)
        .ok_or_else(|| error("缺少 WebSocket 会话标识"))?.to_string();
    let (commands, incoming) = mpsc::channel(8);
    let (events, outgoing) = mpsc::channel(2);
    let connection = Arc::new(Connection {
        commands, events: tokio::sync::Mutex::new(outgoing),
        cancel: CancellationToken::new(), touched: Mutex::new(Instant::now()),
    });
    {
        let mut items = connections().lock().map_err(|_| error("脚本 WebSocket 会话不可用"))?;
        items.retain(|_, item| {
            let keep = item.touched.lock().is_ok_and(|time| time.elapsed() < Duration::from_secs(1800));
            if !keep { item.cancel.cancel(); }
            keep
        });
        if items.len() >= 64 || items.contains_key(&id) { return Err(error("WebSocket 连接已达上限或标识重复")); }
        items.insert(id.clone(), connection.clone());
    }
    let mut opening = Opening(Some(id.clone()));
    let wait = Duration::from_millis(crate::server::config::timeout_settings().headers_ms().clamp(1000, 600_000));
    let connected = async {
        let response = request.send().await.map_err(|failure| error(egress::describe_error_detail(&failure)))?;
        if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
            return Err(error(format!("WebSocket 握手失败：HTTP {}", response.status().as_u16())));
        }
        let header = |name: &str| response.headers().get(name).and_then(|value| value.to_str().ok()).unwrap_or("");
        if !header("upgrade").eq_ignore_ascii_case("websocket")
            || !header("connection").split(',').any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
            || header("sec-websocket-accept") != derive_accept_key(key.as_bytes())
            || !header("sec-websocket-extensions").is_empty()
        { return Err(error("WebSocket 握手校验失败")); }
        let protocol = header("sec-websocket-protocol").to_string();
        if !protocol.is_empty() && !protocols.contains(&protocol) { return Err(error("WebSocket 返回未请求的子协议")); }
        let upgraded = response.upgrade().await.map_err(|_| error("WebSocket 连接升级失败"))?;
        let config = WebSocketConfig::default().read_buffer_size(16 * 1024)
            .write_buffer_size(16 * 1024).max_write_buffer_size(MAX_MESSAGE + 64 * 1024)
            .max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE));
        Ok((WebSocketStream::from_raw_socket(upgraded, Role::Client, Some(config)).await, protocol))
    };
    let (socket, protocol) = tokio::select! {
        biased;
        _ = connection.cancel.cancelled() => return Err(error("WebSocket 连接已取消")),
        result = tokio::time::timeout(wait, connected) => result.map_err(|_| error("WebSocket 握手等待超时"))??,
    };
    crate::spawn_task(run(socket, incoming, events, connection.cancel.clone()));
    opening.0 = None;
    Ok(json!({ "id": id, "protocol": protocol }))
}

async fn event(events: &mpsc::Sender<Value>, cancel: &CancellationToken, value: Value) -> bool {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        result = events.send(value) => result.is_ok(),
    }
}

async fn run(mut socket: Socket, mut commands: mpsc::Receiver<Command>, events: mpsc::Sender<Value>, cancel: CancellationToken) {
    let mut closing = None;
    let mut idle = Instant::now();
    let idle_limit = Duration::from_millis(crate::server::config::timeout_settings().stream_idle_ms().clamp(1000, 1_800_000));
    loop {
        let deadline = closing.unwrap_or(idle + idle_limit);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep_until(deadline.into()) => {
                event(&events, &cancel, json!({ "type": "close", "code": 1006, "reason": "", "clean": false })).await;
                break;
            }
            command = commands.recv() => {
                let Some(command) = command else { break; };
                let close = command.message.is_close();
                let sent = tokio::select! {
                    _ = cancel.cancelled() => break,
                    result = tokio::time::timeout(Duration::from_secs(30), socket.send(command.message)) => result,
                };
                if !matches!(sent, Ok(Ok(()))) {
                    let _ = command.reply.send(Err("WebSocket 发送失败".into()));
                    event(&events, &cancel, json!({ "type": "error" })).await;
                    break;
                }
                let _ = command.reply.send(Ok(()));
                idle = Instant::now();
                if close { closing = Some(Instant::now() + Duration::from_secs(5)); }
            }
            message = socket.next() => {
                idle = Instant::now();
                let value = match message {
                    Some(Ok(Message::Text(text))) => json!({ "type": "text", "data": text.as_str() }),
                    Some(Ok(Message::Binary(bytes))) => json!({ "type": "binary", "data": base64::engine::general_purpose::STANDARD.encode(bytes) }),
                    Some(Ok(Message::Ping(_))) => {
                        let flushed = tokio::select! {
                            _ = cancel.cancelled() => break,
                            result = tokio::time::timeout(Duration::from_secs(5), socket.flush()) => result,
                        };
                        if !matches!(flushed, Ok(Ok(()))) {
                            event(&events, &cancel, json!({ "type": "error" })).await;
                            break;
                        }
                        continue;
                    }
                    Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(Message::Close(frame))) => {
                        let (code, reason) = frame.map(|frame| (u16::from(frame.code), frame.reason.to_string())).unwrap_or((1005, String::new()));
                        let flushed = tokio::time::timeout(Duration::from_secs(5), socket.flush()).await;
                        event(&events, &cancel, json!({ "type": "close", "code": code, "reason": reason, "clean": matches!(flushed, Ok(Ok(()))) })).await;
                        break;
                    }
                    Some(Ok(Message::Frame(_))) => continue,
                    None => { event(&events, &cancel, json!({ "type": "close", "code": 1006, "reason": "", "clean": false })).await; break; }
                    Some(Err(_)) => { event(&events, &cancel, json!({ "type": "error" })).await; break; }
                };
                if !event(&events, &cancel, value).await { break; }
            }
        }
    }
}

pub async fn read(params: &Value) -> Result<Value, proto::WireError> {
    let connection = find(params)?;
    let mut events = connection.events.lock().await;
    tokio::select! {
        biased;
        _ = connection.cancel.cancelled() => Err(error("WebSocket 已取消")),
        event = tokio::time::timeout(Duration::from_secs(20), events.recv()) => Ok(match event {
            Ok(Some(value)) => value,
            Ok(None) => json!({ "type": "close", "code": 1006, "reason": "", "clean": false }),
            Err(_) => json!({ "type": "pending" }),
        }),
    }
}

pub async fn send(params: &Value) -> Result<Value, proto::WireError> {
    let data = params["data"].as_str().unwrap_or("");
    let message = match params["type"].as_str() {
        Some("text") if data.len() <= MAX_MESSAGE => Message::Text(data.into()),
        Some("binary") if data.len() <= MAX_MESSAGE.div_ceil(3) * 4 => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| error("WebSocket 数据编码无效"))?;
            if bytes.len() > MAX_MESSAGE { return Err(error("WebSocket 消息过大")); }
            Message::Binary(bytes.into())
        }
        Some("close") => {
            let reason = params["reason"].as_str().unwrap_or("");
            if reason.len() > 123 { return Err(error("WebSocket 关闭原因过长")); }
            let code = params["code"].as_u64().unwrap_or(1000);
            if code != 1000 && !(3000..=4999).contains(&code) { return Err(error("WebSocket 关闭代码无效")); }
            Message::Close(Some(CloseFrame { code: CloseCode::from(code as u16), reason: reason.into() }))
        }
        _ => return Err(error("WebSocket 消息类型或大小无效")),
    };
    let connection = find(params)?;
    let (reply, result) = oneshot::channel();
    let work = async {
        connection.commands.send(Command { message, reply }).await.map_err(|_| error("WebSocket 已关闭"))?;
        result.await.map_err(|_| error("WebSocket 已关闭"))?.map_err(error)?;
        Ok(Value::Null)
    };
    tokio::select! {
        biased;
        _ = connection.cancel.cancelled() => Err(error("WebSocket 已取消")),
        result = tokio::time::timeout(Duration::from_secs(35), work) => result.map_err(|_| error("WebSocket 发送等待超时"))?,
    }
}

pub fn abort(params: &Value) {
    if let Some(id) = params["id"].as_str() {
        if let Ok(mut items) = connections().lock() {
            if let Some(connection) = items.remove(id) { connection.cancel.cancel(); }
        }
    }
}

pub fn clear() {
    if let Ok(mut items) = connections().lock() {
        items.retain(|id, connection| {
            if id.starts_with("wasm-") { return true; }
            connection.cancel.cancel();
            false
        });
    }
}
