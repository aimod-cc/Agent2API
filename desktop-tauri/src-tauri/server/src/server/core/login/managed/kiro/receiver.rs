//! 接收器固定持有原 v4/v6 listener；阶段选择在请求受理时从原 Session 读取。
//! 连接先记录阶段标识，随后原操作锁下再次 claim；推进后的旧连接不能变成新回调。

use std::collections::HashSet;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::super::{Session, WeakTask};
use super::{invalid, live, url, GatewayError, ReceiverOwner};

pub(super) async fn bind(
    ports: &[u16],
) -> Result<(u16, TcpListener, Option<TcpListener>), GatewayError> {
    for port in ports {
        let Ok(v4) = TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).await else {
            continue;
        };
        let actual = v4.local_addr().map_err(|_| invalid())?.port();
        if actual != *port || actual == 0 {
            return Err(invalid());
        }
        // Portal 的 localhost 可能解析 ::1；同族被别的程序占用时放弃整组，不让授权流向它。
        let v6 = match TcpListener::bind((Ipv6Addr::LOCALHOST, actual)).await {
            Ok(v6) => Some(v6),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
                ) =>
            {
                None
            }
            Err(_) => continue,
        };
        return Ok((actual, v4, v6));
    }
    Err(GatewayError::with_status(
        409,
        "Kiro 固定回调端口不可用，未发起授权或改用其他端口",
    ))
}

pub(super) fn spawn(receiver: &Arc<ReceiverOwner>, task: &WeakTask, session: &Arc<Session>) {
    let slots = Arc::new(Semaphore::new(4));
    spawn_one(&receiver.v4, receiver, task, session, &slots);
    if let Some(v6) = &receiver.v6 {
        spawn_one(v6, receiver, task, session, &slots);
    }
}

fn spawn_one(
    listener: &Arc<TcpListener>,
    receiver: &Arc<ReceiverOwner>,
    task: &WeakTask,
    session: &Arc<Session>,
    slots: &Arc<Semaphore>,
) {
    let listener = Arc::downgrade(listener);
    let receiver = Arc::downgrade(receiver);
    let task = task.clone();
    let session = Arc::downgrade(session);
    let slots = slots.clone();
    let Some(owner) = receiver.upgrade() else {
        return;
    };
    let cancel = owner.cancel.clone();
    drop(owner);
    crate::spawn_task(async move {
        loop {
            let Some(listener) = listener.upgrade() else {
                return;
            };
            let accepted = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                accepted = listener.accept() => accepted,
            };
            drop(listener);
            let Ok((stream, peer)) = accepted else {
                return;
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                continue;
            };
            let Some(current_session) = session.upgrade() else {
                return;
            };
            // 接收时固定原阶段，不能读完旧连接后按最新阶段误认。
            let stage = {
                let state = current_session
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if !current_session.valid() || !live(&state) {
                    return;
                }
                state.kiro.current.clone()
            };
            let Some(stage) = stage else {
                continue;
            };
            let receiver = receiver.clone();
            let session = session.clone();
            let task = task.clone();
            let cancel = cancel.clone();
            crate::spawn_task(async move {
                let _permit = permit;
                serve(stream, receiver, session, task, stage, cancel).await;
            });
        }
    });
}

async fn request(stream: &mut TcpStream, port: u16) -> Result<String, GatewayError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let count = stream.read(&mut chunk).await.map_err(|_| invalid())?;
        if count == 0 {
            return Err(invalid());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > 12 * 1024 {
            return Err(invalid());
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(invalid)?;
    if end + 4 != bytes.len() {
        return Err(invalid());
    }
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| invalid())?;
    let mut lines = text.split("\r\n");
    let first: Vec<_> = lines.next().ok_or_else(invalid)?.split(' ').collect();
    if first.len() != 3
        || first[0] != "GET"
        || first[2] != "HTTP/1.1"
        || first[1].len() > 8192
        || !first[1].starts_with('/')
        || first[1].starts_with("//")
    {
        return Err(invalid());
    }
    let (path, _) = first[1].split_once('?').ok_or_else(invalid)?;
    if !["/signin/callback", "/oauth/callback"].contains(&path) {
        return Err(invalid());
    }
    let mut names = HashSet::new();
    let mut host = None;
    for line in lines {
        if names.len() >= 48 || line.starts_with([' ', '\t']) {
            return Err(invalid());
        }
        let (name, value) = line.split_once(':').ok_or_else(invalid)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(invalid());
        }
        let name = name.to_ascii_lowercase();
        if !names.insert(name.clone()) {
            return Err(invalid());
        }
        let value = value.trim();
        if value
            .bytes()
            .any(|byte| byte.is_ascii_control() && byte != b'\t')
        {
            return Err(invalid());
        }
        match name.as_str() {
            "host" => host = Some(value.to_string()),
            "content-length" if value != "0" => return Err(invalid()),
            "transfer-encoding" | "expect" | "upgrade" => return Err(invalid()),
            _ => {}
        }
    }
    let host = host.ok_or_else(invalid)?;
    if host != format!("localhost:{port}") && host != format!("127.0.0.1:{port}") {
        return Err(invalid());
    }
    Ok(format!("http://{host}{}", first[1]))
}

async fn serve(
    mut stream: TcpStream,
    receiver: Weak<ReceiverOwner>,
    session: Weak<Session>,
    task: WeakTask,
    stage: String,
    cancel: CancellationToken,
) {
    let Some(owner) = receiver.upgrade() else {
        return;
    };
    let port = owner.port;
    drop(owner);
    let raw = tokio::select! {
        biased;
        _ = cancel.cancelled() => return,
        result = tokio::time::timeout(Duration::from_secs(5), request(&mut stream, port)) => match result {
            Ok(Ok(raw)) => raw,
            _ => { respond(&mut stream, false).await; return; },
        },
    };
    let Some(current_session) = session.upgrade() else {
        return;
    };
    let accepted = {
        let mut state = current_session
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let source = state.kiro.receiver.as_ref().is_some_and(|owner| {
            !owner.cancel.is_cancelled() && Weak::ptr_eq(&receiver, &Arc::downgrade(owner))
        });
        let valid = current_session.valid()
            && live(&state)
            && source
            && state.kiro.current.as_deref() == Some(stage.as_str())
            && state
                .leases
                .get(&stage)
                .zip(state.step.as_ref())
                .is_some_and(|(lease, step)| {
                    !lease.queued
                        && step.status == "ready"
                        && url::callback(lease, &step.id, &raw).is_ok()
                });
        if valid {
            if let Some(lease) = state.leases.get_mut(&stage) {
                lease.queued = true;
            }
        }
        valid
    };
    if !accepted {
        respond(&mut stream, false).await;
        return;
    }
    let Some(task) = task.upgrade() else {
        return;
    };
    // 原操作锁下再claim，材料只进原Call，一次受理后旧stage不可排队/恢复。
    // 不复制raw到额外worker；此连接已持许可，其未来被取消时一次材料直接丢弃。
    drop(current_session);
    respond(&mut stream, true).await;
    let completion = super::super::receive_callback(task, &stage, &raw);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {},
        _ = completion => {},
    }
}

async fn respond(stream: &mut TcpStream, accepted: bool) {
    let message = if accepted {
        "本次 Kiro 回调已交给登录任务，请返回面板查看当前步骤及最终结果。"
    } else {
        "未接受此 Kiro 回调，请返回面板查看当前登录步骤。"
    };
    let status = if accepted {
        "200 OK"
    } else {
        "400 Bad Request"
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{message}", message.len());
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        stream.write_all(response.as_bytes()),
    )
    .await;
    let _ = stream.shutdown().await;
}
