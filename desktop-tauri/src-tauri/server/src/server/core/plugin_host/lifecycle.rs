use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::json;
use tokio::sync::Mutex;

use super::process::{self, ExitReport, ProcessRun};
use super::{Host, Inner, StreamEvent};

impl Host {
    fn scoped_runtime_package(&self) -> bool {
        self.providers.lock().ok().is_some_and(|providers| providers.iter()
            .any(|provider| crate::server::core::providers::is_scoped_runtime_provider(provider.id.as_str())))
    }

    pub async fn ensure(self: &Arc<Self>) -> Result<Vec<proto::ProviderInfo>, String> {
        if self.closing.load(Ordering::Acquire) { return Err("插件宿主已关闭，不能重新启动已回收的宿主".to_string()); }
        if self.disabled.load(Ordering::Acquire) { return Err(self.failure_message()); }
        if self.ready() {
            return Ok(self.providers.lock().map(|providers| providers.clone()).unwrap_or_default());
        }
        self.start().await
    }

    pub async fn shutdown(&self) {
        self.activity.retire();
        let _serial = self.starting.lock().await;
        let scoped = self.scoped_runtime_package();
        let run = {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            self.closing.store(true, Ordering::Release);
            self.ready.store(false, Ordering::Release);
            let run = self.current_run();
            if let Some(run) = &run { run.expect_exit(); }
            run
        };
        let Some(run) = run else {
            self.running.store(false, Ordering::Release);
            return;
        };
        let clean = if self.running.load(Ordering::Acquire) {
            let timeout = if scoped { Duration::from_secs(15) } else { Duration::from_secs(2) };
            let result = tokio::time::timeout(timeout,
                self.call_coded(proto::methods::gateway::SHUTDOWN, json!({}), timeout)).await;
            matches!(result, Ok(Ok(value)) if value.is_null())
        } else { !scoped };
        let report = run.reap(Duration::from_secs(2)).await;
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            self.running.store(false, Ordering::Release);
            self.fail_pending("插件宿主已关闭");
        }
        if report.status.is_some() {
            *self.inner.lock().await = None;
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            run.retired.store(true, Ordering::Release);
            *self.active.lock().unwrap_or_else(|error| error.into_inner()) = None;
        }
        if (scoped && !clean) || report.status.is_none() {
            let message = if report.status.is_some() { "插件辅助程序已退出，但资源清理未确认完成" }
                else { "插件未确认退出；未把发送终止信号视为关闭" };
            if let Ok(mut last) = self.last_error.lock() { *last = Some(message.to_string()); }
            process::event("error", message, run.event_data("plugin.shutdown", self.executable(), Some(&report)));
        }
    }

    async fn start(self: &Arc<Self>) -> Result<Vec<proto::ProviderInfo>, String> {
        let _serial = self.starting.lock().await;
        if self.closing.load(Ordering::Acquire) { return Err("插件宿主正在关闭".to_string()); }
        if self.ready() {
            return Ok(self.providers.lock().map(|providers| providers.clone()).unwrap_or_default());
        }
        if self.disabled.load(Ordering::Acquire) { return Err(self.failure_message()); }
        if let Ok(retry) = self.retry_after.lock() {
            if let Some(deadline) = *retry {
                if std::time::Instant::now() < deadline {
                    return Err(format!("插件暂不可用，{:.1} 秒后重试", deadline.saturating_duration_since(std::time::Instant::now()).as_secs_f32()));
                }
            }
        }

        if let Some(old) = self.current_run() {
            {
                let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
                old.expect_exit();
                old.retired.store(true, Ordering::Release);
                self.running.store(false, Ordering::Release);
                self.ready.store(false, Ordering::Release);
                self.fail_pending("插件宿主正在重新启动");
            }
            let report = old.reap(Duration::from_millis(100)).await;
            if report.status.is_none() {
                let message = "旧插件进程尚未确认退出，未启动新进程";
                self.note_failure(message);
                return Err(message.to_string());
            }
        }
        *self.inner.lock().await = None;

        let mut command = tokio::process::Command::new(&self.options.exe);
        command.args(&self.options.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        if let Some(dir) = &self.options.cwd { command.current_dir(dir); }
        let mut child = command.spawn().map_err(|error| {
            let message = process::redact(&format!("启动插件进程失败（{}）：{error}", self.executable()));
            self.note_failure(&message);
            message
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            let _ = process::reap(&mut child, Duration::ZERO).await;
            let message = "插件进程没有完整的 stdin/stdout 管道";
            self.note_failure(message);
            return Err(message.to_string());
        };
        let stderr = child.stderr.take();
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let run = Arc::new(ProcessRun::new(generation, child));
        *self.inner.lock().await = Some(Inner {
            stdin: Arc::new(Mutex::new(stdin)), run: run.clone(),
        });
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            *self.active.lock().unwrap_or_else(|error| error.into_inner()) = Some(run.clone());
            self.running.store(true, Ordering::Release);
        }
        let stderr_task = process::drain_stderr(run.clone(), stderr);
        let host = self.clone();
        let reader_run = run.clone();
        tokio::spawn(async move {
            host.read_loop(proto::frame::buffered(stdout), reader_run, stderr_task).await;
        });

        let env = proto::HostEnv {
            data_dir: self.options.data_dir.clone(), host_version: self.options.host_version.clone(),
            target: self.options.target.clone(), proxy: self.options.proxy.clone(),
            debug_traffic: self.options.debug_traffic, loopback_port: self.options.loopback_port,
        };
        let result = tokio::time::timeout(self.options.init_timeout,
            self.call(proto::methods::gateway::INIT, json!({ "env": env }), self.options.init_timeout)).await
            .map_err(|_| "插件握手超时".to_string())
            .and_then(|result| result.map_err(|error| format!("插件握手失败：{error}")))
            .and_then(|value| serde_json::from_value::<Vec<proto::ProviderInfo>>(value)
                .map_err(|error| format!("插件返回的 provider 列表无法解析：{error}")));
        match result {
            Ok(providers) => {
                let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
                if !self.is_current_run(&run) || run.failed.load(Ordering::Acquire)
                    || !self.running.load(Ordering::Acquire) || self.closing.load(Ordering::Acquire) {
                    return Err(self.failure_message());
                }
                if let Ok(mut current) = self.providers.lock() { *current = providers.clone(); }
                if let Ok(mut crashes) = self.crashes.lock() { *crashes = 0; }
                if let Ok(mut retry) = self.retry_after.lock() { *retry = None; }
                let previous = self.last_error.lock().ok().and_then(|mut last| last.take());
                self.ready.store(true, Ordering::Release);
                if previous.is_some() {
                    let mut data = run.event_data("plugin.recovered", self.executable(), None);
                    data["previousError"] = json!(previous);
                    data["detail"] = json!("插件重新启动并完成握手，当前故障提示已清除；历史故障仍保留在日志中。");
                    process::event("info", &format!("插件「{}」已恢复运行", self.executable()), data);
                }
                Ok(providers)
            }
            Err(message) => {
                self.mark_run_failure(&run, &message);
                let _ = run.reap(Duration::ZERO).await;
                Err(run.failure().unwrap_or_else(|| self.failure_message()))
            }
        }
    }

    async fn read_loop(
        self: Arc<Self>,
        mut reader: tokio::io::BufReader<tokio::process::ChildStdout>,
        run: Arc<ProcessRun>,
        mut stderr_task: tokio::task::JoinHandle<()>,
    ) {
        let reason = loop {
            if run.retired.load(Ordering::Acquire) { break "旧插件进程已回收".to_string(); }
            match proto::read_frame(&mut reader).await {
                Ok(Some(frame)) => {
                    let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
                    if !self.is_current_run(&run) { break "旧插件进程已回收".to_string(); }
                    match frame.kind() {
                        proto::FrameKind::Reply | proto::FrameKind::Error => self.settle(frame),
                        proto::FrameKind::Request => {
                            let host = self.clone();
                            let callback_run = run.clone();
                            tokio::spawn(async move { host.serve_callback(frame, callback_run).await; });
                        }
                        proto::FrameKind::Event => self.handle_event(frame),
                        proto::FrameKind::Unknown => {}
                    }
                }
                Ok(None) => break "插件通信通道已关闭（stdout EOF）".to_string(),
                Err(error) => break process::redact(&format!("读取插件通信帧失败：{error}")),
            }
        };
        run.note_channel_closed();
        self.mark_run_failure(&run, &reason);
        let report = run.reap(run.expected_exit_grace()).await;
        process::finish_stderr(&run, &mut stderr_task).await;
        self.record_exit(&run, &reason, &report);
    }

    pub(super) fn current_run(&self) -> Option<Arc<ProcessRun>> {
        self.active.lock().ok().and_then(|run| run.clone())
    }

    pub(super) fn is_current_run(&self, run: &ProcessRun) -> bool {
        self.generation.load(Ordering::Acquire) == run.generation && !run.retired.load(Ordering::Acquire)
    }

    fn executable(&self) -> &str {
        self.options.exe.file_name().and_then(|name| name.to_str()).unwrap_or("plugin")
    }

    fn failure_message(&self) -> String {
        self.last_error.lock().ok().and_then(|last| last.clone())
            .unwrap_or_else(|| "插件进程不可用".to_string())
    }

    fn note_failure(&self, message: &str) {
        let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
        let message = process::redact(message);
        let crashes = self.note_failure_state(&message);
        process::event("error", &format!("插件「{}」不可用（连续失败 {crashes} 次）：{message}", self.executable()), json!({
            "source": "plugin", "component": self.executable(), "action": "plugin.failure",
            "consecutiveFailures": crashes, "disabled": crashes >= 5,
        }));
    }

    fn note_failure_state(&self, message: &str) -> u32 {
        if let Ok(mut last) = self.last_error.lock() { *last = Some(message.to_string()); }
        let crashes = self.crashes.lock().map(|mut count| { *count = count.saturating_add(1); *count }).unwrap_or(1);
        let seconds = (1u64 << crashes.saturating_sub(1).min(6)).min(60);
        if let Ok(mut retry) = self.retry_after.lock() {
            *retry = Some(std::time::Instant::now() + Duration::from_secs(seconds));
        }
        if crashes >= 5 { self.disabled.store(true, Ordering::Release); }
        crashes
    }

    fn mark_run_failure(&self, run: &ProcessRun, reason: &str) {
        let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
        if !self.is_current_run(run) { return; }
        self.running.store(false, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        if run.is_expected_exit() { self.fail_pending("插件宿主已关闭"); return; }
        let message = process::redact(reason);
        if !run.record_failure(&message) { return; }
        self.fail_pending(&message);
        let crashes = self.note_failure_state(&message);
        let mut data = run.event_data("plugin.failure", self.executable(), None);
        data["consecutiveFailures"] = json!(crashes);
        data["disabled"] = json!(crashes >= 5);
        process::event("error", &format!("插件「{}」不可用（连续失败 {crashes} 次）：{message}", self.executable()), data);
    }

    fn record_exit(&self, run: &ProcessRun, reason: &str, report: &ExitReport) {
        let failure = run.failure();
        if failure.is_none() && run.is_expected_exit() { return; }
        let message = format!("{}；{}", failure.as_deref().unwrap_or(reason), report.description());
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            if self.is_current_run(run) && run.failed.load(Ordering::Acquire) {
                if let Ok(mut last) = self.last_error.lock() { *last = Some(message.clone()); }
                run.replace_failure(&message);
            }
        }
        process::event("error", &format!("插件「{}」退出诊断：{message}", self.executable()),
            run.event_data("plugin.exit", self.executable(), Some(report)));
    }

    fn fail_pending(&self, message: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            for (_, call) in pending.drain() {
                let _ = call.tx.send(Err(proto::WireError::with_code(message, proto::WireError::INTERNAL)));
            }
        }
        if let Ok(mut streams) = self.streams.lock() {
            for (_, stream) in streams.drain() {
                let _ = stream.tx.send(StreamEvent::Failed(message.to_string()));
            }
        }
    }
}
