//! 仅监听 loopback 的 OAuth 回调端口；响应不含账号、code、state 或 token。

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use super::login::{self, Flow, MAX_CALLBACK_BYTES};

pub(super) fn spawn(flow: Arc<Flow>, listener: TcpListener) {
    crate::spawn_task(async move {
        let slots = Arc::new(Semaphore::new(8));
        loop {
            let accepted = tokio::select! {
                biased;
                _ = flow.stop.cancelled() => break,
                accepted = listener.accept() => accepted,
            };
            let Ok((socket, peer)) = accepted else {
                break;
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                continue;
            };
            let flow = flow.clone();
            crate::spawn_task(async move {
                let _permit = permit;
                tokio::select! {
                    biased;
                    _ = flow.stop.cancelled() => {},
                    _ = tokio::time::timeout(Duration::from_secs(10), handle(socket, flow.clone())) => {},
                }
            });
        }
    });
}

async fn handle(mut socket: TcpStream, flow: Arc<Flow>) {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let Ok(read) = socket.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > MAX_CALLBACK_BYTES {
            reply(&mut socket, false).await;
            return;
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let Ok(request) = std::str::from_utf8(&bytes) else {
        return;
    };
    let mut lines = request.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, target, version) = (first.next(), first.next(), first.next());
    if method != Some("GET")
        || first.next().is_some()
        || !matches!(version, Some("HTTP/1.1" | "HTTP/1.0"))
    {
        reply(&mut socket, false).await;
        return;
    }
    let Some(target) = target.filter(|target| target.starts_with("/auth/callback?")) else {
        reply(&mut socket, false).await;
        return;
    };
    let Ok(redirect) = url::Url::parse(&flow.redirect) else {
        return;
    };
    let authority = format!("127.0.0.1:{}", redirect.port().unwrap_or(1455));
    let mut host = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            reply(&mut socket, false).await;
            return;
        };
        if name.eq_ignore_ascii_case("host") {
            if host.replace(value.trim()).is_some() {
                reply(&mut socket, false).await;
                return;
            }
        }
    }
    if host != Some(authority.as_str()) {
        reply(&mut socket, false).await;
        return;
    }
    let full = format!("http://{authority}{target}");
    let accepted = login::receive(&flow, &full)
        .await
        .is_ok_and(|result| !result.retryable);
    reply(&mut socket, accepted).await;
}

async fn reply(socket: &mut TcpStream, accepted: bool) {
    let response = crate::server::core::login::callback_page::raw_receipt_response(
        accepted,
        super::ID,
        "回调地址无效或不属于本次 ChatGPT 登录",
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}
