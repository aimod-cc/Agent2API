use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr};
use tokio::sync::{oneshot, Notify};

use super::{client::CodexClient, process_guard::ProcessGuard, CodingError};

static PROCESSES: OnceLock<Mutex<Vec<Weak<ProcessControl>>>> = OnceLock::new();

pub fn stop_all() {
    if let Some(processes) = PROCESSES.get() {
        for process in processes.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade) { process.kill(); }
    }
}

pub struct ProcessControl {
    child: Mutex<Child>,
    guard: Mutex<Option<ProcessGuard>>,
    pub exited: AtomicBool,
    pub notify: Notify,
}

impl ProcessControl {
    pub fn new(child: Child) -> Result<Arc<Self>, CodingError> {
        let guard = ProcessGuard::new(&child)?;
        let process = Arc::new(Self { child: Mutex::new(child), guard: Mutex::new(Some(guard)), exited: AtomicBool::new(false), notify: Notify::new() });
        let mut processes = PROCESSES.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap_or_else(|e| e.into_inner());
        processes.retain(|p| p.strong_count() > 0);
        processes.push(Arc::downgrade(&process));
        Ok(process)
    }

    pub fn kill(&self) {
        if let Some(guard) = self.guard.lock().unwrap_or_else(|e| e.into_inner()).take() { guard.kill(); }
        if let Ok(mut child) = self.child.lock() { let _ = child.start_kill(); }
        self.notify.notify_waiters();
    }

    pub async fn wait(&self) -> bool {
        for _ in 0..30 {
            if self.exited.load(Ordering::Acquire) { return true; }
            let exited = self.child.lock().map(|mut child| child.try_wait().ok().flatten().is_some()).unwrap_or(true);
            if exited { self.exited.store(true, Ordering::Release); return true; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }
}

pub fn read_endpoint(stderr: ChildStderr) -> oneshot::Receiver<Result<String, CodingError>> {
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut ready = Some(tx);
        while let Ok(Some(line)) = bounded_line(&mut reader, 256 * 1024).await {
            if ready.is_none() { continue; }
            let text = String::from_utf8_lossy(&line);
            if let Some((_, raw)) = text.split_once("listening on:") {
                let raw = raw.trim();
                if let Ok(url) = url::Url::parse(raw) {
                    if url.scheme() == "ws" && url.host_str() == Some("127.0.0.1") && url.port().is_some_and(|p| p > 0) {
                        if let Some(ready) = ready.take() { let _ = ready.send(Ok(raw.to_string())); }
                    }
                }
            }
        }
        if let Some(ready) = ready { let _ = ready.send(Err(CodingError::new(503, "Codex 启动中断，未取得本机共享地址"))); }
    });
    rx
}

pub fn supervise(client: Arc<CodexClient>) {
    tokio::spawn(async move {
        loop {
            let exited = client.process.child.lock().map(|mut child| child.try_wait().ok().flatten().is_some()).unwrap_or(true);
            if exited { break; }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        client.process.kill();
        client.process.exited.store(true, Ordering::Release);
        client.disconnected("本机 Codex 后台已退出；历史记录仍保留，正在执行的任务不会自动重发");
        client.process.notify.notify_waiters();
    });
}

async fn bounded_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R, maximum: usize) -> Result<Option<Vec<u8>>, ()> {
    let mut out = Vec::new();
    loop {
        let available = reader.fill_buf().await.map_err(|_| ())?;
        if available.is_empty() { return if out.is_empty() { Ok(None) } else { Ok(Some(out)) }; }
        let count = available.iter().position(|b| *b == b'\n').map(|i| i + 1).unwrap_or(available.len());
        if out.len() + count > maximum { return Err(()); }
        let ended = available[count - 1] == b'\n';
        out.extend_from_slice(&available[..count]);
        reader.consume(count);
        if ended { return Ok(Some(out)); }
    }
}
