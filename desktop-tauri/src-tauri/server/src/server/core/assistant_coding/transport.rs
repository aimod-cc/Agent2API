use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message, protocol::WebSocketConfig};

use super::{client::CodexClient, permissions, projection, sessions, CodingError};

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
pub struct WriteRequest {
    pub value: Value,
    pub result: oneshot::Sender<Result<(), CodingError>>,
}

pub fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(64 * 1024 * 1024)).max_frame_size(Some(64 * 1024 * 1024))
}

pub async fn open(endpoint: &str, token: &str) -> Result<Socket, CodingError> {
    let mut request = endpoint.into_client_request().map_err(|_| CodingError::new(500, "Codex 本机连接地址无效"))?;
    request.headers_mut().insert("Authorization", format!("Bearer {token}").parse().map_err(|_| CodingError::new(500, "Codex 本机连接凭证无效"))?);
    tokio::time::timeout(Duration::from_secs(8), connect_async_with_config(request, Some(socket_config()), true)).await
        .map_err(|_| CodingError::new(504, "连接 Codex 本机后台超时"))?
        .map(|(socket, _)| socket).map_err(|_| CodingError::new(503, "无法连接 Codex 本机后台"))
}

pub async fn run(client: Arc<CodexClient>, ready: oneshot::Sender<Result<(), CodingError>>) {
    let mut ready = Some(ready);
    while client.running.load(Ordering::Acquire) {
        let result = match open(&client.endpoint, &client.connection_token).await {
            Ok(socket) => serve(client.clone(), socket, &mut ready).await,
            Err(error) => Err(error),
        };
        *client.writer.lock().await = None;
        client.transport_lost();
        if let Some(ready) = ready.take() {
            let _ = ready.send(result.and_then(|_| Err(CodingError::new(503, "Codex 连接在初始化时中断"))));
            break;
        }
        if !client.running.load(Ordering::Acquire) { break; }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn serve(client: Arc<CodexClient>, socket: Socket, ready: &mut Option<oneshot::Sender<Result<(), CodingError>>>) -> Result<(), CodingError> {
    let epoch = client.connection_epoch.fetch_add(1, Ordering::AcqRel) + 1;
    let (mut sink, mut source) = socket.split();
    let (sender, mut receiver) = mpsc::channel::<WriteRequest>(64);
    *client.writer.lock().await = Some(sender);
    let writer = async {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        loop {
            tokio::select! {
                request = receiver.recv() => {
                    let Some(request) = request else { return Err(CodingError::new(503, "Codex 连接已关闭")); };
                    if request.result.is_closed() { continue; }
                    let result = tokio::time::timeout(Duration::from_secs(10), sink.send(Message::Text(request.value.to_string().into()))).await;
                    let ok = matches!(result, Ok(Ok(())));
                    let _ = request.result.send(if ok { Ok(()) } else { Err(CodingError::uncertain("Codex 写入连接中断，未自动重发")) });
                    if !ok { return Err(CodingError::uncertain("Codex 写入连接中断，未自动重发")); }
                }
                _ = heartbeat.tick() => {
                    if !client.running.load(Ordering::Acquire) || client.connection_epoch.load(Ordering::Acquire) != epoch { return Ok(()); }
                    tokio::time::timeout(Duration::from_secs(5), sink.send(Message::Ping(Vec::new().into()))).await
                        .map_err(|_| CodingError::new(503, "Codex 连接无响应"))?.map_err(|_| CodingError::new(503, "Codex 连接已关闭"))?;
                }
            }
        }
    };
    let reader = async {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(50), source.next()).await
                .map_err(|_| CodingError::new(503, "Codex 连接无响应"))?
                .ok_or_else(|| CodingError::new(503, "Codex 连接已关闭"))?
                .map_err(|_| CodingError::new(503, "Codex 连接已中断"))?;
            match message {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).map_err(|_| CodingError::new(502, "Codex 返回了无效数据"))?;
                    if value.get("method").is_some() {
                        if value.get("id").is_some() { permissions::register(&client, value).await; }
                        else { projection::notification(&client, value); }
                    } else { client.resolve(value); }
                }
                Message::Close(_) => return Ok(()),
                _ => {}
            }
        }
    };
    let initialize = async {
        client.rpc("initialize", json!({
            "clientInfo": { "name": "agent2api", "title": "Agent2API", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "experimentalApi": true },
        })).await?;
        client.write(json!({ "method": "initialized" })).await?;
        sessions::observe_loaded(&client).await?;
        client.connected.store(true, Ordering::Release);
        client.state.changed_all();
        Ok::<_, CodingError>(())
    };
    tokio::pin!(writer, reader, initialize);
    let mut initialized = false;
    loop {
        tokio::select! {
            result = &mut writer => return result,
            result = &mut reader => return result,
            result = &mut initialize, if !initialized => {
                result?;
                initialized = true;
                if let Some(ready) = ready.take() { let _ = ready.send(Ok(())); }
            }
            _ = client.process.notify.notified() => {
                if !client.running.load(Ordering::Acquire) || client.connection_epoch.load(Ordering::Acquire) != epoch { return Ok(()); }
            }
        }
    }
}
