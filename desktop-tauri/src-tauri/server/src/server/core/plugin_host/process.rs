use std::collections::VecDeque;
use std::io;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use crate::server::logging;

const STDERR_LINE_BYTES: usize = 4096;
const STDERR_LINES: usize = 16;
const STDERR_LINE_CHARS: usize = 512;

pub(super) struct ProcessRun {
    pub generation: u64,
    pub pid: Option<u32>,
    child: tokio::sync::Mutex<tokio::process::Child>,
    guard: Mutex<Option<super::process_guard::ProcessGuard>>,
    expected_exit: Mutex<Option<Instant>>,
    pub failed: AtomicBool,
    pub retired: AtomicBool,
    failure: Mutex<Option<String>>,
    exit_report: Mutex<Option<ExitReport>>,
    started: Instant,
    ended: Mutex<Option<Instant>>,
    diagnostic: Mutex<Diagnostic>,
}

impl Drop for ProcessRun {
    fn drop(&mut self) {
        if let Some(guard) = self.guard.get_mut().unwrap_or_else(|error| error.into_inner()).take() {
            guard.kill();
        }
    }
}

#[derive(Default)]
struct Diagnostic {
    stderr: VecDeque<String>,
    discarded: u64,
    private_key: bool,
    stopped: bool,
}

#[derive(Clone, Default)]
pub(super) struct ExitReport {
    pub status: Option<ExitStatus>,
    pub forced: bool,
    pub wait_error: Option<String>,
}

impl ExitReport {
    pub fn description(&self) -> String {
        let mut text = match self.status {
            Some(status) => {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(signal) = status.signal() {
                        return format!("退出信号 {signal}{}", if self.forced { "，由网关终止" } else { "" });
                    }
                }
                match status.code() {
                    Some(code) => {
                        #[cfg(windows)]
                        { format!("退出码 {code}（0x{:08X}）", code as u32) }
                        #[cfg(not(windows))]
                        { format!("退出码 {code}") }
                    }
                    None => "进程已退出，退出码不可用".to_string(),
                }
            }
            None => "尚未取得退出码".to_string(),
        };
        if self.forced { text.push_str("，已请求网关终止"); }
        if let Some(error) = &self.wait_error { text.push_str(&format!("；{error}")); }
        text
    }
}

impl ProcessRun {
    pub fn new(generation: u64, child: tokio::process::Child) -> Self {
        Self {
            generation,
            pid: child.id(),
            child: tokio::sync::Mutex::new(child),
            guard: Mutex::new(None),
            expected_exit: Mutex::new(None),
            failed: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            failure: Mutex::new(None),
            exit_report: Mutex::new(None),
            started: Instant::now(),
            ended: Mutex::new(None),
            diagnostic: Mutex::new(Diagnostic::default()),
        }
    }

    pub fn protect(&self, guard: Option<super::process_guard::ProcessGuard>) {
        *self.guard.lock().unwrap_or_else(|error| error.into_inner()) = guard;
    }

    pub fn expect_exit(&self) {
        if let Ok(mut expected) = self.expected_exit.lock() {
            expected.get_or_insert_with(Instant::now);
        }
    }

    pub fn is_expected_exit(&self) -> bool {
        self.expected_exit.lock().map(|expected| expected.is_some()).unwrap_or(true)
    }

    pub fn expected_exit_grace(&self) -> Duration {
        self.expected_exit.lock().ok().and_then(|expected| *expected)
            .map(|started| Duration::from_secs(20).saturating_sub(started.elapsed()))
            .unwrap_or_else(|| Duration::from_millis(250))
    }

    pub fn note_channel_closed(&self) {
        if let Ok(mut ended) = self.ended.lock() { ended.get_or_insert_with(Instant::now); }
    }

    pub fn record_failure(&self, message: &str) -> bool {
        let Ok(mut failure) = self.failure.lock() else { return false };
        if self.failed.swap(true, Ordering::AcqRel) { return false; }
        *failure = Some(message.to_string());
        true
    }

    pub fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|failure| failure.clone())
    }

    pub fn replace_failure(&self, message: &str) {
        if let Ok(mut failure) = self.failure.lock() { *failure = Some(message.to_string()); }
    }

    pub async fn reap(&self, grace: Duration) -> ExitReport {
        let mut child = self.child.lock().await;
        if let Some(report) = self.exit_report.lock().ok().and_then(|report| report.clone()) {
            if report.status.is_some() { return report; }
        }
        let report = reap(&mut child, grace).await;
        if let Some(guard) = self.guard.lock().unwrap_or_else(|error| error.into_inner()).take() {
            guard.kill();
        }
        if let Ok(mut saved) = self.exit_report.lock() { *saved = Some(report.clone()); }
        report
    }

    fn record_stderr(&self, bytes: &[u8], oversized: bool) {
        let Ok(mut diagnostic) = self.diagnostic.lock() else { return };
        if diagnostic.stopped { return; }
        let raw = String::from_utf8_lossy(bytes);
        let private_start = raw.contains("-----BEGIN") && raw.contains("PRIVATE KEY-----");
        let private_end = raw.contains("-----END") && raw.contains("PRIVATE KEY-----");
        let mut safe = redact(&raw);
        if let Some(index) = safe.find('`') {
            safe.truncate(index);
            safe.push_str("[运行时附带值已隐藏]");
        }
        let line = if private_start || diagnostic.private_key {
            diagnostic.private_key = !private_end;
            "[私钥内容已隐藏]".to_string()
        } else if oversized {
            "[stderr 超长行已省略]".to_string()
        } else {
            safe.chars().take(STDERR_LINE_CHARS).collect::<String>()
        };
        if line.is_empty() { return; }
        if diagnostic.stderr.len() == STDERR_LINES {
            diagnostic.stderr.pop_front();
            diagnostic.discarded = diagnostic.discarded.saturating_add(1);
        }
        diagnostic.stderr.push_back(line);
    }

    pub fn finish_stderr(&self) {
        if let Ok(mut diagnostic) = self.diagnostic.lock() { diagnostic.stopped = true; }
    }

    pub fn event_data(&self, action: &str, executable: &str, report: Option<&ExitReport>) -> Value {
        let (stderr, discarded) = self.diagnostic.lock().map(|diagnostic| {
            (diagnostic.stderr.iter().cloned().collect::<Vec<_>>(), diagnostic.discarded)
        }).unwrap_or_default();
        let detail = if stderr.is_empty() {
            "插件未留下 stderr 诊断输出，不能仅凭管道关闭判断退出原因。".to_string()
        } else {
            format!("退出前的 stderr 摘要（已脱敏）：\n{}", stderr.join("\n"))
                .chars().take(950).collect()
        };
        let elapsed = self.ended.lock().ok().and_then(|ended| *ended)
            .map(|ended| ended.saturating_duration_since(self.started))
            .unwrap_or_else(|| self.started.elapsed());
        let mut data = json!({
            "source": "plugin", "component": executable, "action": action,
            "pid": self.pid, "generation": self.generation,
            "uptimeMs": elapsed.as_millis().min(u64::MAX as u128) as u64,
            "stderr": stderr, "stderrDiscardedLines": discarded, "detail": detail,
        });
        if let Some(report) = report {
            data["exitCode"] = json!(report.status.and_then(|status| status.code()));
            data["exitStatus"] = json!(report.description());
            data["forced"] = json!(report.forced);
            data["waitError"] = json!(report.wait_error);
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                data["exitSignal"] = json!(report.status.and_then(|status| status.signal()));
            }
        }
        data
    }
}

pub(super) fn drain_stderr(
    run: Arc<ProcessRun>,
    stderr: Option<tokio::process::ChildStderr>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(mut stderr) = stderr else { return };
        let mut chunk = [0u8; 2048];
        let mut line = Vec::with_capacity(STDERR_LINE_BYTES);
        let mut oversized = false;
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) => break,
                Ok(count) => {
                    for &byte in &chunk[..count] {
                        if byte == b'\n' {
                            run.record_stderr(&line, oversized);
                            line.clear();
                            oversized = false;
                        } else if line.len() < STDERR_LINE_BYTES {
                            line.push(byte);
                        } else {
                            oversized = true;
                        }
                    }
                }
                Err(error) => {
                    run.record_stderr(format!("读取 stderr 失败：{error}").as_bytes(), false);
                    break;
                }
            }
        }
        if !line.is_empty() || oversized { run.record_stderr(&line, oversized); }
    })
}

pub(super) async fn finish_stderr(run: &ProcessRun, task: &mut tokio::task::JoinHandle<()>) {
    if tokio::time::timeout(Duration::from_millis(250), &mut *task).await.is_err() {
        task.abort();
        let _ = task.await;
    }
    run.finish_stderr();
}

pub(super) async fn reap(child: &mut tokio::process::Child, grace: Duration) -> ExitReport {
    let mut report = ExitReport::default();
    match child.try_wait() {
        Ok(Some(status)) => { report.status = Some(status); return report; }
        Ok(None) => {}
        Err(error) => report.wait_error = Some(redact(&format!("读取退出状态失败：{error}"))),
    }
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(Ok(status)) => { report.status = Some(status); return report; }
        Ok(Err(error)) => report.wait_error = Some(redact(&format!("读取退出状态失败：{error}"))),
        Err(_) => {}
    }
    report.forced = true;
    if let Err(error) = child.start_kill() {
        report.wait_error = Some(redact(&format!("终止插件失败：{error}")));
    }
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) => report.status = Some(status),
        Ok(Err(error)) => report.wait_error = Some(redact(&format!("等待插件退出失败：{error}"))),
        Err(_) => report.wait_error = Some("等待插件退出超时；未把发送终止信号视为已退出".to_string()),
    }
    report
}

pub(super) fn event(level: &str, message: &str, data: Value) {
    logging::console_line("Plugin", message);
    logging::log_event(level, "plugin", &format!("Plugin {message}"), Some(data));
}

pub(super) fn redact(text: &str) -> String {
    static FILTERS: OnceLock<Result<Vec<Regex>, regex::Error>> = OnceLock::new();
    let filters = FILTERS.get_or_init(|| [
        r#"(?i)(?:https?|wss?|socks5h?)://[^\s<>\"']+"#,
        r#"(?i)\b(?:bearer|basic)\s+[A-Za-z0-9_./+=-]+"#,
        r#"(?i)\b[\w-]*(?:authorization|cookie|token|secret|password|passwd|credential|api[_-]?key|session[_-]?id|signature|body|payload|prompt|content|messages?|request|response)[\w-]*\s*[\"']?\s*[:=].*"#,
        r#"(?i)\b[\w-]*(?:authorization|cookie|token|secret|password|passwd|credential|api[_-]?key)[\w-]*\s+[\"']?\s*[A-Za-z0-9_./+=-]+.*"#,
        r#"(?i)\b(?:sk-|gh[pousr]_)[A-Za-z0-9_-]+"#,
        r#"\b[A-Za-z0-9_+=/-]{24,}(?:\.[A-Za-z0-9_+=/-]+)*\b"#,
        r#"(?i)(?:[a-z]:[\\/]+users[\\/]+|/(?:home|users)/)[^\s\\/\"']+"#,
        r#"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"#,
    ].into_iter().map(Regex::new).collect());
    let Ok(filters) = filters else { return "[诊断内容已隐藏]".to_string() };
    let mut safe = text.chars().filter(|ch| !ch.is_control() || ch.is_whitespace()).collect::<String>();
    for filter in filters { safe = filter.replace_all(&safe, "[redacted]").into_owned(); }
    safe.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn disconnected() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "插件进程已退出或正在关闭")
}
