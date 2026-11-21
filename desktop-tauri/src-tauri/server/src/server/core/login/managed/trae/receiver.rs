//! Trae 专用IPv4接收器；真实listener对象只属于原Session，输入不经过手工回调入口。
//! 每组4个有界连接、单候选queue/claim；收到响应仅表示交付，不代表兑换或保存成功。

use std::collections::HashSet;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::super::{Session, WeakTask};
use super::{callback, invalid, queue, QueuedCallback, ReceiverOwner};
use crate::server::errors::GatewayError;

pub(super) fn spawn(receiver: &Arc<ReceiverOwner>, task: &WeakTask, session: &Arc<Session>) {
    let receiver = Arc::downgrade(receiver);
    let task = task.clone();
    let provider = session.provider.clone();
    let session = Arc::downgrade(session);
    let slots = Arc::new(Semaphore::new(4));
    let Some(owner) = receiver.upgrade() else {
        return;
    };
    let cancel = owner.stop_accept.clone();
    let listener = Arc::downgrade(&owner.listener);
    drop(owner);
    crate::spawn_task(async move {
        loop {
            let Some(listener) = listener.upgrade() else {
                return;
            };
            let accepted = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                result = listener.accept() => result,
            };
            drop(listener);
            let Ok((stream, peer)) = accepted else {
                return;
            };
            if !matches!(peer.ip(), std::net::IpAddr::V4(address) if address.is_loopback()) {
                continue;
            }
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                continue;
            };
            let receiver = receiver.clone();
            let provider = provider.clone();
            let session = session.clone();
            let task = task.clone();
            let cancel = cancel.clone();
            crate::spawn_task(async move {
                let _permit = permit;
                serve(stream, receiver, session, task, &provider, cancel).await;
            });
        }
    });
}

async fn request(stream: &mut TcpStream, port: u16) -> Result<callback::Input, GatewayError> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(1024));
    let mut chunk = Zeroizing::new([0u8; 1024]);
    loop {
        let count = stream.read(&mut *chunk).await.map_err(|_| invalid())?;
        if count == 0 {
            return Err(invalid());
        }
        if bytes.len().saturating_add(count) > 64 * 1024 {
            return Err(invalid());
        }
        bytes.extend_from_slice(&chunk[..count]);
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
    let raw = std::str::from_utf8(&bytes[..end]).map_err(|_| invalid())?;
    let mut lines = raw.split("\r\n");
    let request: Vec<_> = lines.next().ok_or_else(invalid)?.split(' ').collect();
    if request.len() != 3
        || request[0] != "GET"
        || !matches!(request[2], "HTTP/1.1" | "HTTP/1.0")
        || request[1].len() > 32 * 1024
        || request[1].contains('#')
    {
        return Err(invalid());
    }
    let (path, query) = request[1].split_once('?').ok_or_else(invalid)?;
    if path != "/authorize" {
        return Err(invalid());
    }
    let mut host = None;
    let mut header_count = 0usize;
    let mut singleton_headers = HashSet::new();
    for line in lines {
        header_count += 1;
        if header_count > 64 || line.starts_with([' ', '\t']) {
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
        if matches!(
            name.as_str(),
            "host" | "content-length" | "transfer-encoding" | "expect" | "upgrade"
        ) && !singleton_headers.insert(name.clone())
        {
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
    if host.as_deref() != Some(format!("127.0.0.1:{port}").as_str()) {
        return Err(invalid());
    }
    callback::parse(query)
}

async fn serve(
    mut stream: TcpStream,
    receiver: Weak<ReceiverOwner>,
    session: Weak<Session>,
    task: WeakTask,
    provider: &str,
    cancel: CancellationToken,
) {
    let Some(owner) = receiver.upgrade() else {
        return;
    };
    let port = owner.port;
    drop(owner);
    let input = tokio::select! {
        biased;
        _ = cancel.cancelled() => return,
        result = tokio::time::timeout(Duration::from_secs(5), request(&mut stream, port)) => match result {
            Ok(Ok(input)) => input,
            _ => { respond(&mut stream, false, provider).await; return; },
        },
    };
    let Some(current) = session.upgrade() else {
        return;
    };
    let queued = match queue(&current, &receiver, input) {
        Ok(queued) => queued,
        Err(_) => {
            respond(&mut stream, false, provider).await;
            return;
        }
    };
    let Some(task) = task.upgrade() else {
        return;
    };
    drop(current);
    // 完整对象移到单一交付任务；不排raw URL、不按序列化码重新构造，也不重复回调。
    let completion = crate::spawn_task(deliver(task, queued));
    respond(&mut stream, true, provider).await;
    let _ = completion.await;
}

struct DeliveryGuard {
    task: super::super::LoginTaskHandle,
    complete: bool,
}
impl Drop for DeliveryGuard {
    fn drop(&mut self) {
        if !self.complete {
            super::super::fail(
                &self.task,
                "Trae 本次真实回调处理已中断，未重放授权码；请重新发起",
            );
        }
    }
}

fn deliver(
    task: super::super::LoginTaskHandle,
    queued: QueuedCallback,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    // 在future首次poll前建立兜底；取消等待不会把已queue的输入重新放回。
    let mut guard = DeliveryGuard {
        task,
        complete: false,
    };
    async move {
        let task = guard.task.clone();
        let result = super::receive(task.clone(), queued).await;
        if let Err(error) = result {
            super::super::fail(&task, &error.message);
        }
        guard.complete = true;
    }
}

async fn respond(stream: &mut TcpStream, accepted: bool, provider: &str) {
    let response = crate::server::core::login::callback_page::raw_receipt_response(
        accepted,
        provider,
        "未受理此 Trae 回调，请回到面板查看当前步骤",
    );
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        stream.write_all(response.as_bytes()),
    )
    .await;
    let _ = stream.shutdown().await;
}
