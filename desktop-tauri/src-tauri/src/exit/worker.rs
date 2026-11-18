//! 最终动作由独立任务唯一消费；丢失等待者不重发退出、重启或安装。

use std::sync::Arc;

use futures::FutureExt;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::{ui, Action, Allowed, Coordinator, Operation, Phase, ACTION_FAILED, CLEANUP_FAILED, INTERRUPTED};
use crate::{backend, state::AppState};

pub(super) fn spawn(app: AppHandle, shared: Arc<Coordinator>, operation: Arc<Operation>, guard: TaskGuard) {
    let (begin, ready) = oneshot::channel();
    let state = shared.clone();
    let current = operation.clone();
    let app_task = app.clone();
    let work = async move {
        if ready.await.is_err() {
            return;
        }
        let outcome = std::panic::AssertUnwindSafe(run(&app_task, &state, &current)).catch_unwind().await;
        match outcome {
            Ok(Ok(())) => guard.complete(),
            Ok(Err(message)) => {
                state.lock().allowed = None;
                current.fail(message);
                ui::error(&app_task, &state, message);
            }
            Err(_) => {
                state.lock().allowed = None;
                current.fail(INTERRUPTED);
                ui::error(&app_task, &state, INTERRUPTED);
            }
        }
    };
    let spawned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tauri::async_runtime::handle().inner().spawn(work)
    }));
    match spawned {
        Ok(job) => {
            operation.lock().task = Some(job);
            let _ = begin.send(());
        }
        Err(_) => {
            operation.fail(INTERRUPTED);
            ui::error(&app, &shared, INTERRUPTED);
        }
    }
}

async fn run(app: &AppHandle, shared: &Arc<Coordinator>, operation: &Arc<Operation>) -> Result<(), &'static str> {
    let action = operation.action.clone();
    if action.closes_backend() {
        let state = app.state::<AppState>();
        if let Err(detail) = backend::shutdown_and_wait(&state).await {
            // 后端只返回固定状态/计数；用户提示不混入凭据或未知安装路径。
            eprintln!("[Exit] 后端关闭未完成: {detail}");
            return Err(CLEANUP_FAILED);
        }
        shared.lock().cleaned = true;
        operation.lock().cleanup_confirmed = true;
    }
    let running = matches!(operation.lock().phase, Phase::Running);
    if !running || shared.lock().poisoned {
        return Err(INTERRUPTED);
    }
    match action {
        Action::Quit(code) => final_exit(app, shared, operation, Allowed::Quit(code), json!({"requested": true})),
        Action::Restart => final_exit(app, shared, operation, Allowed::Restart, json!({"restarting": true})),
        Action::Install(installer) => {
            if installer.closes_backend() && !operation.lock().cleanup_confirmed {
                return Err(CLEANUP_FAILED);
            }
            let value = launch(operation, installer.clone()).await?;
            if installer.exit_after() {
                final_exit(app, shared, operation, Allowed::Quit(0), value)
            } else {
                complete(operation, value)
            }
        }
    }
}

fn complete(operation: &Operation, value: Value) -> Result<(), &'static str> {
    {
        let mut state = operation.lock();
        if !matches!(state.phase, Phase::Running) {
            return Err(INTERRUPTED);
        }
        state.phase = Phase::Done(value);
    }
    operation.changed.notify_waiters();
    Ok(())
}

fn final_exit(app: &AppHandle, shared: &Coordinator, operation: &Operation, allow: Allowed, value: Value) -> Result<(), &'static str> {
    {
        let mut state = shared.lock();
        let mut current = operation.lock();
        if !state.cleaned || !current.cleanup_confirmed || state.poisoned || !matches!(current.phase, Phase::Running) {
            return Err(INTERRUPTED);
        }
        state.allowed = Some(allow);
        current.phase = Phase::Done(value);
    }
    operation.changed.notify_waiters();
    // 仅此处调用Tauri最终动作；request_restart的特殊退出码不能依赖回调阻止。
    match allow {
        Allowed::Quit(code) => app.exit(code),
        Allowed::Restart => app.request_restart(),
    }
    Ok(())
}

async fn launch(operation: &Arc<Operation>, installer: super::installer::Verified) -> Result<Value, &'static str> {
    let (sender, receiver) = oneshot::channel();
    let guard = LaunchGuard { operation: operation.clone(), completed: false };
    let runner = operation.clone();
    let task = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tauri::async_runtime::handle().inner().spawn_blocking(move || {
            // 已派出的同步系统启动操作不能靠丢等待future来取消；句柄保留至真正返回。
            let may_launch = {
                let state = runner.lock();
                matches!(state.phase, Phase::Running)
                    && (!installer.closes_backend() || state.cleanup_confirmed)
            };
            let result = if may_launch {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| installer.launch()))
                    .unwrap_or_else(|_| Err("安装器启动过程异常中断".into()))
            } else {
                Err("安装执行资格已失效，未运行安装器".into())
            };
            let _ = sender.send(result);
            guard.complete();
            runner.changed.notify_waiters();
        })
    })).map_err(|_| INTERRUPTED)?;
    operation.lock().final_task = Some(task);
    let result = receiver.await.map_err(|_| INTERRUPTED)?;
    let mut waiting = WaitingLaunch { operation: operation.clone(), job: operation.lock().final_task.take() };
    let Some(job) = waiting.job.as_mut() else {
        return Err(INTERRUPTED);
    };
    let joined = job.await;
    waiting.job.take();
    joined.map_err(|_| INTERRUPTED)?;
    result.map_err(|detail| {
        eprintln!("[Exit] 安装器未确认启动: {detail}");
        ACTION_FAILED
    })
}

struct WaitingLaunch {
    operation: Arc<Operation>,
    job: Option<JoinHandle<()>>,
}

impl Drop for WaitingLaunch {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            self.operation.lock().final_task = Some(job);
        }
    }
}

pub(super) struct TaskGuard {
    shared: Arc<Coordinator>,
    operation: Arc<Operation>,
    completed: bool,
}

impl TaskGuard {
    pub(super) fn new(shared: Arc<Coordinator>, operation: Arc<Operation>) -> Self {
        Self { shared, operation, completed: false }
    }

    fn complete(mut self) { self.completed = true; }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        if !self.completed {
            // 析构只登记失败/唤醒；不派新网络或窗口任务，不重试最终动作。
            self.shared.lock().allowed = None;
            self.operation.fail(INTERRUPTED);
        }
    }
}

struct LaunchGuard {
    operation: Arc<Operation>,
    completed: bool,
}

impl LaunchGuard {
    fn complete(mut self) { self.completed = true; }
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.operation.fail(INTERRUPTED);
        }
    }
}
