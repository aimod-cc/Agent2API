//! 只接CatPaw批准来源的POST表单；Google GET监听路径不经过此解析器。
//! 每个flow四条连接，头12KiB/表单32KiB/整次读取5秒，拒绝重复与走私字段。
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use super::super::{catpaw, Session, WeakTask};
use crate::server::core::plugin_host::wasm::managed_policy::CATPAW_ORIGIN;

const HEAD_LIMIT: usize = 12 * 1024;
const BODY_LIMIT: usize = 32 * 1024;

enum Request {
    Form(catpaw::PostToken),
    Preflight { private_network: bool },
}

pub(super) fn spawn(listener: TcpListener, task: &WeakTask, session: &Arc<Session>, id: &str, uri: &str,
    cancel: &CancellationToken, slots: &Arc<Semaphore>) {
    let task = task.clone();
    let session = Arc::downgrade(session);
    let id = id.to_string();
    let uri = uri.to_string();
    let cancel = cancel.clone();
    let slots = slots.clone();
    crate::spawn_task(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                accepted = listener.accept() => accepted,
            };
            let Ok((stream, peer)) = accepted else { return; };
            if !peer.ip().is_loopback() { continue; }
            // 尚未发布browser时接受到的连接不保留，不能等到后续步骤再占机会。
            if !session.upgrade().is_some_and(|session| catpaw::preflight_live(&session, &id)) { continue; }
            let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
            let task = task.clone();
            let session = session.clone();
            let id = id.clone();
            let uri = uri.clone();
            let cancel = cancel.clone();
            crate::spawn_task(async move { serve(stream, task, session, id, uri, cancel, permit).await; });
        }
    });
}

async fn serve(mut stream: TcpStream, task: WeakTask, session: Weak<Session>, id: String, uri: String,
    cancel: CancellationToken, _permit: OwnedSemaphorePermit) {
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => return,
        result = tokio::time::timeout(Duration::from_secs(5), request(&mut stream, &uri)) => result,
    };
    let parsed = match result {
        Ok(Ok(request)) => request,
        _ => { respond(&mut stream, Reply::Rejected, false).await; return; }
    };
    let Some(session) = session.upgrade() else { return; };
    match parsed {
        Request::Preflight { private_network } => {
            let reply = if catpaw::preflight_live(&session, &id) { Reply::Preflight(private_network) } else { Reply::Rejected };
            respond(&mut stream, reply, true).await;
        }
        Request::Form(input) => {
            let Some(task) = task.upgrade() else { return; };
            let expected = match catpaw::queue_post(&session, &id, &input) {
                Ok(expected) => expected,
                Err(_) => { respond(&mut stream, Reply::Rejected, true).await; return; }
            };
            // 在任何await前将唯一候选交宿主任务；from-never-polled的guard也会终结本flow。
            // 原连接许可持有到投递处理完成，不产生无界候选或无限等待队列。
            let completion = crate::spawn_task(catpaw::deliver_post(task, id, expected, input));
            respond(&mut stream, Reply::Accepted, true).await;
            let _ = completion.await;
        }
    }
}

struct Head { method: String, headers: BTreeMap<String, String>, length: usize }

fn head(bytes: &[u8], uri: &str) -> Result<Head, ()> {
    let raw = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut lines = raw.split("\r\n");
    let parts: Vec<_> = lines.next().ok_or(())?.split(' ').collect();
    let url = url::Url::parse(uri).map_err(|_| ())?;
    if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") || url.path() != "/catpaw/callback"
        || url.port().is_none_or(|port| port == 0) { return Err(()); }
    if parts.len() != 3 || !matches!(parts[0], "POST" | "OPTIONS") || parts[1] != url.path() || parts[2] != "HTTP/1.1" {
        return Err(());
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        if headers.len() >= 48 || line.starts_with([' ', '\t']) { return Err(()); }
        let (name, value) = line.split_once(':').ok_or(())?;
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') { return Err(()); }
        let name = name.to_ascii_lowercase();
        let value = value.trim_matches([' ', '\t']);
        if value.bytes().any(|byte| byte.is_ascii_control() && byte != b'\t') || value.len() > 8192 {
            return Err(());
        }
        if matches!(name.as_str(), "transfer-encoding" | "content-encoding" | "expect" | "upgrade" | "trailer" | "proxy-connection" | "proxy-authorization")
            || headers.insert(name, value.to_string()).is_some() { return Err(()); }
    }
    let authority = format!("127.0.0.1:{}", url.port().ok_or(())?);
    if headers.get("host").map(String::as_str) != Some(authority.as_str())
        || headers.get("origin").map(String::as_str) != Some(CATPAW_ORIGIN) { return Err(()); }
    if let Some(connection) = headers.get("connection") {
        if connection.split(',').any(|value| !matches!(value.trim().to_ascii_lowercase().as_str(), "close" | "keep-alive")) {
            return Err(());
        }
    }
    let length = match headers.get("content-length") {
        Some(value) if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => value.parse::<usize>().map_err(|_| ())?,
        None if parts[0] == "OPTIONS" => 0,
        _ => return Err(()),
    };
    if length > BODY_LIMIT { return Err(()); }
    Ok(Head { method: parts[0].to_string(), headers, length })
}

fn content_type(raw: &str) -> bool {
    let mut parts = raw.split(';');
    if !parts.next().is_some_and(|value| value.trim().eq_ignore_ascii_case("application/x-www-form-urlencoded")) { return false; }
    if let Some(charset) = parts.next() {
        let Some((name, value)) = charset.trim().split_once('=') else { return false; };
        if !name.trim().eq_ignore_ascii_case("charset") || !value.trim().eq_ignore_ascii_case("utf-8") { return false; }
    }
    parts.next().is_none()
}

async fn request(stream: &mut TcpStream, uri: &str) -> Result<Request, ()> {
    let mut buffer = Vec::with_capacity(2048);
    let mut chunk = [0; 1024];
    let end = loop {
        let size = stream.read(&mut chunk).await.map_err(|_| ())?;
        if size == 0 { return Err(()); }
        buffer.extend_from_slice(&chunk[..size]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            if end + 4 > HEAD_LIMIT { return Err(()); }
            break end;
        }
        if buffer.len() >= HEAD_LIMIT { return Err(()); }
    };
    let head = head(&buffer[..end], uri)?;
    let total = end + 4 + head.length;
    if buffer.len() > total { return Err(()); }
    if head.method == "OPTIONS" {
        if head.length != 0 || head.headers.get("access-control-request-method").map(String::as_str) != Some("POST")
            || head.headers.contains_key("content-type") { return Err(()); }
        if let Some(headers) = head.headers.get("access-control-request-headers") {
            // 可选的Content-Type是唯一获准额外头；不放行Authorization/Cookie或任意头列表。
            if !headers.eq_ignore_ascii_case("content-type") { return Err(()); }
        }
        let private_network = match head.headers.get("access-control-request-private-network").map(String::as_str) {
            Some("true") => true,
            None => false,
            _ => return Err(()),
        };
        return Ok(Request::Preflight { private_network });
    }
    if head.length == 0 || !head.headers.get("content-type").is_some_and(|value| content_type(value))
        || head.headers.keys().any(|key| key.starts_with("access-control-request-")) { return Err(()); }
    while buffer.len() < total {
        let size = stream.read(&mut chunk).await.map_err(|_| ())?;
        if size == 0 { return Err(()); }
        if buffer.len().saturating_add(size) > total { return Err(()); }
        buffer.extend_from_slice(&chunk[..size]);
    }
    // 连接只处理一帧并关闭；已随同到达的流水线/尾随内容不是第二次登录机会。
    match stream.try_read(&mut chunk) {
        Ok(size) if size != 0 => return Err(()),
        Err(error) if error.kind() != std::io::ErrorKind::WouldBlock => return Err(()),
        _ => {}
    }
    Ok(Request::Form(form(&buffer[end + 4..])?))
}

fn form(bytes: &[u8]) -> Result<catpaw::PostToken, ()> {
    let raw = std::str::from_utf8(bytes).map_err(|_| ())?;
    if raw.is_empty() || raw.chars().any(char::is_control) { return Err(()); }
    let mut fields = BTreeMap::new();
    for pair in raw.split('&') {
        let (key, value) = pair.split_once('=').ok_or(())?;
        let key = decode(key)?;
        let value = decode(value)?;
        if !matches!(key.as_str(), "token" | "state") || fields.insert(key, value).is_some() { return Err(()); }
    }
    if fields.len() != 2 { return Err(()); }
    catpaw::PostToken::parsed(fields.remove("token").ok_or(())?, fields.remove("state").ok_or(())?).map_err(|_| ())
}

fn decode(raw: &str) -> Result<String, ()> {
    let bytes = raw.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        match bytes[offset] {
            b'+' => result.push(b' '),
            b'%' => {
                let hex = bytes.get(offset + 1..offset + 3).ok_or(())?;
                let high = (hex[0] as char).to_digit(16).ok_or(())?;
                let low = (hex[1] as char).to_digit(16).ok_or(())?;
                result.push((high * 16 + low) as u8);
                offset += 2;
            }
            byte => result.push(byte),
        }
        offset += 1;
    }
    String::from_utf8(result).map_err(|_| ())
}

#[derive(Clone, Copy)]
enum Reply { Accepted, Rejected, Preflight(bool) }

async fn respond(stream: &mut TcpStream, reply: Reply, approved_origin: bool) {
    let (status, message) = match reply {
        Reply::Accepted => ("200 OK", "回调已交给登录任务处理，请返回面板查看最终结果。"),
        Reply::Rejected => ("400 Bad Request", "未接受此回调，请返回面板检查当前登录步骤。"),
        Reply::Preflight(_) => ("204 No Content", ""),
    };
    let mut cors = String::new();
    if approved_origin {
        cors.push_str(&format!("Access-Control-Allow-Origin: {CATPAW_ORIGIN}\r\nVary: Origin, Access-Control-Request-Method, Access-Control-Request-Headers, Access-Control-Request-Private-Network\r\n"));
        if let Reply::Preflight(private_network) = reply {
            cors.push_str("Access-Control-Allow-Methods: POST\r\nAccess-Control-Allow-Headers: content-type\r\nAccess-Control-Max-Age: 0\r\n");
            if private_network { cors.push_str("Access-Control-Allow-Private-Network: true\r\n"); }
        }
    }
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\n{cors}Connection: close\r\n\r\n{message}", message.len());
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    }).await;
}
