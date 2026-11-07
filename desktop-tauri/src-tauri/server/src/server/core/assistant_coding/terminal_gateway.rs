use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, OwnedMutexGuard};
use tokio_tungstenite::{accept_hdr_async_with_config, tungstenite::{handshake::server::{Request, Response}, Message}, WebSocketStream};

use super::{client::CodexClient, terminal_policy::{self, TerminalSession}, transport, CodingError};

pub struct TerminalGrant {
    thread_id: Mutex<String>,
    pub attached: AtomicBool,
    created: Instant,
}

pub struct TerminalTicket {
    pub endpoint: String,
    pub token: String,
    pub grant: Arc<TerminalGrant>,
}

pub struct TerminalGateway {
    endpoint: String,
    grants: Arc<Mutex<HashMap<String, Arc<TerminalGrant>>>>,
    stop: watch::Sender<bool>,
}

impl TerminalGateway {
    pub async fn start(client: Weak<CodexClient>) -> Result<Self, CodingError> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|_| CodingError::new(500, "无法建立本机终端连接"))?;
        let address = listener.local_addr().map_err(|_| CodingError::new(500, "无法读取本机终端地址"))?;
        let grants = Arc::new(Mutex::new(HashMap::<String, Arc<TerminalGrant>>::new()));
        let (stop, mut stopping) = watch::channel(false);
        let accept_grants = grants.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stopping.changed() => break,
                    accepted = listener.accept() => {
                        let Ok((socket, peer)) = accepted else { break; };
                        if !peer.ip().is_loopback() { continue; }
                        let Some(client) = client.upgrade().filter(|c| c.running.load(Ordering::Acquire)) else { break; };
                        let grants = accept_grants.clone();
                        let stop = stopping.clone();
                        tokio::spawn(async move { let _ = connection(client, socket, grants, stop).await; });
                    }
                }
            }
        });
        Ok(Self { endpoint: format!("ws://{address}"), grants, stop })
    }

    pub fn ticket(&self, thread_id: &str) -> Result<TerminalTicket, CodingError> {
        if *self.stop.borrow() { return Err(CodingError::new(503, "本机共享终端入口已关闭")); }
        let token = crate::server::access::random_hex(32);
        let grant = Arc::new(TerminalGrant { thread_id: Mutex::new(thread_id.into()), attached: AtomicBool::new(false), created: Instant::now() });
        let mut grants = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        grants.retain(|_, g| g.created.elapsed() < Duration::from_secs(24 * 60 * 60)
            && (g.attached.load(Ordering::Acquire) || g.created.elapsed() < Duration::from_secs(120)));
        if grants.len() >= 128 { return Err(CodingError::new(409, "本次运行的终端连接过多，请关闭不用的终端后重新启动 Codex 后台")); }
        grants.insert(digest(&token), grant.clone());
        Ok(TerminalTicket { endpoint: self.endpoint.clone(), token, grant })
    }

    pub fn revoke(&self, token: &str) { self.grants.lock().unwrap_or_else(|e| e.into_inner()).remove(&digest(token)); }
    pub fn close(&self) { self.stop.send_replace(true); self.grants.lock().unwrap_or_else(|e| e.into_inner()).clear(); }
}

fn digest(token: &str) -> String { format!("{:x}", Sha256::digest(token.as_bytes())) }

struct Pending {
    request: Value,
    _operation: Option<OwnedMutexGuard<()>>,
    started: Instant,
}

async fn deliver(socket: &mut WebSocketStream<TcpStream>, value: Value, detached: &mut Option<Instant>) {
    if detached.is_some() { return; }
    if !matches!(tokio::time::timeout(Duration::from_secs(8), socket.send(Message::Text(value.to_string().into()))).await, Ok(Ok(()))) {
        *detached = Some(Instant::now());
    }
}

async fn connection(client: Arc<CodexClient>, socket: TcpStream, grants: Arc<Mutex<HashMap<String, Arc<TerminalGrant>>>>, mut stop: watch::Receiver<bool>) -> Result<(), CodingError> {
    let mut grant = None;
    let accepted = tokio::time::timeout(Duration::from_secs(8), accept_hdr_async_with_config(socket, |request: &Request, response: Response| {
        let token = request.headers().get("Authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
        if !request.headers().contains_key("Origin") {
            grant = token.and_then(|token| grants.lock().unwrap_or_else(|e| e.into_inner()).get(&digest(token)).cloned())
                .filter(|g| g.created.elapsed() < Duration::from_secs(24 * 60 * 60)
                    && (g.attached.load(Ordering::Acquire) || g.created.elapsed() < Duration::from_secs(120)));
        }
        if grant.is_some() { Ok(response) }
        else {
            let mut denied = tokio_tungstenite::tungstenite::http::Response::new(Some("Local terminal authorization required".into()));
            *denied.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
            Err(denied)
        }
    }, Some(transport::socket_config()))).await;
    let mut downstream = accepted.map_err(|_| CodingError::new(408, "本机终端连接超时"))?
        .map_err(|_| CodingError::new(401, "本机终端连接未授权"))?;
    let grant = grant.ok_or_else(|| CodingError::new(401, "本机终端连接未授权"))?;
    client.check_current().await?;
    let thread_id = grant.thread_id.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(&thread_id).is_none_or(|s| s.ending || !s.attached) {
        return Err(CodingError::new(409, "这条共享会话已结束，请从界面重新打开终端"));
    }
    let mut session = TerminalSession { thread_id, initialized: false, attached: false };
    let mut upstream = transport::open(&client.endpoint, &client.connection_token).await?;
    let mut pending = HashMap::<String, Pending>::new();
    let mut detached = None;
    let mut check = tokio::time::interval(Duration::from_secs(1));
    let result = async {
        loop {
            if detached.is_some() && pending.is_empty() { break; }
            tokio::select! {
                _ = stop.changed() => break,
                _ = check.tick() => {
                    if !client.running.load(Ordering::Acquire) || detached.is_some_and(|at: Instant| at.elapsed() > Duration::from_secs(50)) { break; }
                    if pending.values().any(|p| p.started.elapsed() > Duration::from_secs(50)) {
                        return Err(CodingError::uncertain("终端请求尚未确认，连接已关闭；不会自动重发"));
                    }
                }
                incoming = downstream.next(), if detached.is_none() => {
                    let Some(Ok(incoming)) = incoming else { detached = Some(Instant::now()); continue; };
                    match incoming {
                        Message::Text(text) => {
                            let mut request: Value = serde_json::from_str(&text).map_err(|_| CodingError::new(400, "终端消息格式无效"))?;
                            if request.get("method").is_some() {
                                let operation = if terminal_policy::serialized(&request) {
                                    match client.operations.clone().try_lock_owned() {
                                        Ok(guard) => Some(guard),
                                        Err(_) => {
                                            deliver(&mut downstream, json!({ "id": request.get("id"), "error": { "code": -32001, "message": "Another session operation is pending; wait for its result before trying again" } }), &mut detached).await;
                                            continue;
                                        }
                                    }
                                } else { None };
                                if let Err(error) = terminal_policy::before(&client, &session, &mut request).await {
                                    if let Some(id) = request.get("id") { deliver(&mut downstream, json!({ "id": id, "error": { "code": -32600, "message": error.message } }), &mut detached).await; }
                                    continue;
                                }
                                if let Some(id) = request.get("id") {
                                    if pending.len() >= 128 || pending.contains_key(&id.to_string()) { return Err(CodingError::new(429, "终端请求过多或标识重复")); }
                                    pending.insert(id.to_string(), Pending { request: request.clone(), _operation: operation, started: Instant::now() });
                                }
                                send_upstream(&mut upstream, request).await?;
                            } else { send_upstream(&mut upstream, request).await?; }
                        }
                        Message::Close(_) => { detached = Some(Instant::now()); }
                        Message::Ping(value) => { let _ = tokio::time::timeout(Duration::from_secs(5), downstream.send(Message::Pong(value))).await; }
                        _ => {}
                    }
                }
                incoming = upstream.next() => {
                    let Some(Ok(incoming)) = incoming else { break; };
                    match incoming {
                        Message::Text(text) => {
                            let message: Value = serde_json::from_str(&text).map_err(|_| CodingError::new(502, "Codex 终端响应无效"))?;
                            let mut replay = Vec::new();
                            if message.get("method").is_none() {
                                if let Some(entry) = message.get("id").and_then(|id| pending.remove(&id.to_string())) {
                                    match terminal_policy::after(&client, &mut session, &entry.request, &message).await {
                                        Ok(notifications) => replay = notifications,
                                        Err(error) => {
                                            deliver(&mut downstream, json!({ "id": message.get("id"), "error": { "code": -32603, "message": error.message } }), &mut detached).await;
                                            continue;
                                        }
                                    }
                                    if session.attached {
                                        grant.attached.store(true, Ordering::Release);
                                        *grant.thread_id.lock().unwrap_or_else(|e| e.into_inner()) = session.thread_id.clone();
                                    }
                                }
                            }
                            deliver(&mut downstream, terminal_policy::filter(&client, message), &mut detached).await;
                            for notification in replay { deliver(&mut downstream, notification, &mut detached).await; }
                        }
                        Message::Close(_) => break,
                        Message::Ping(value) => { let _ = tokio::time::timeout(Duration::from_secs(5), upstream.send(Message::Pong(value))).await; }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }.await;
    terminal_policy::detached(&client, &session);
    let _ = tokio::time::timeout(Duration::from_secs(2), upstream.close(None)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), downstream.close(None)).await;
    result
}

async fn send_upstream(socket: &mut transport::Socket, value: Value) -> Result<(), CodingError> {
    tokio::time::timeout(Duration::from_secs(10), socket.send(Message::Text(value.to_string().into()))).await
        .map_err(|_| CodingError::uncertain("终端请求发送超时，未重发"))?
        .map_err(|_| CodingError::uncertain("终端请求发送中断，未重发"))
}
