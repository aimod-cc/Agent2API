//! 桌面后端只有一次启动与一次关闭；公开等待者不拥有后台任务的取消权。
//! 锁只保护内存状态，不跨await、不包文件/端口操作。窗口的exiting标志不参与本门。

mod startup;
mod stopping;

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::FutureExt;
use tauri::{AppHandle, Manager};
use tokio::sync::{oneshot, Notify};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::port_conflict::StartupFailure;
use crate::state::{AppState, BackendHandle};

pub(super) use startup::Startup;

type Backend = Arc<Mutex<BackendHandle>>;
const TIMEOUT: Duration = Duration::from_secs(55);
const CLOSING: &str = "本地网关正在关闭，未重新启动或发布就绪状态";
const LOST: &str = "桌面后端执行任务异常结束，启动或关闭状态未确认";

#[derive(Clone, Copy)]
enum Role {
    Startup = 0,
    Bootstrap = 1,
    Coordinator = 2,
    Cleanup = 3,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Self::Startup => "桌面启动",
            Self::Bootstrap => "阻塞初始化",
            Self::Coordinator => "桌面关闭协调",
            Self::Cleanup => "服务器及插件清理",
        }
    }
}

#[derive(Default)]
struct Job {
    claimed: bool,
    released: bool,
    released_at: Option<Instant>,
    lost: bool,
    joined: bool,
    retained: bool,
    handle: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub(crate) struct Lifecycle {
    changed: Arc<Notify>,
    jobs: [Job; 4],
    startup_result: Option<Result<(), StartupFailure>>,
    bootstrap_result: Option<Result<crate::server::ServerState, StartupFailure>>,
    binding: bool,
    binding_lost: bool,
    ready: bool,
    deadline: Option<Instant>,
    cleanup_result: Option<Result<(), String>>,
    close_result: Option<Result<(), String>>,
    hooks_complete: bool,
    poisoned: bool,
}

fn lock(backend: &Backend) -> MutexGuard<'_, BackendHandle> {
    match backend.lock() {
        Ok(guard) => guard,
        Err(error) => {
            let mut guard = error.into_inner();
            if !guard.lifecycle.poisoned {
                guard.lifecycle.poisoned = true;
                seal_locked(&mut guard);
                guard.lifecycle.close_result.get_or_insert_with(|| Err(
                    "服务器状态锁异常；保留原任务，关闭结果未确认".into()
                ));
                guard.lifecycle.changed.notify_waiters();
            }
            guard
        }
    }
}

fn notification(backend: &Backend) -> Arc<Notify> {
    lock(backend).lifecycle.changed.clone()
}

fn seal_locked(state: &mut BackendHandle) -> Instant {
    state.lifecycle.ready = false;
    *state.lifecycle.deadline.get_or_insert_with(|| Instant::now() + TIMEOUT)
}

fn open(state: &BackendHandle) -> Result<(), StartupFailure> {
    if state.lifecycle.poisoned {
        Err(StartupFailure::other("服务器状态锁异常，未恢复启动资格"))
    } else if state.lifecycle.deadline.is_some() {
        Err(StartupFailure::other(CLOSING))
    } else {
        Ok(())
    }
}

// 不改变窗口退出flag；调用者只能获得关闭结果，不能由本模块触发真正退出。
fn seal(backend: &Backend, stop_startup: bool) -> Instant {
    let (deadline, server, changed) = {
        let mut state = lock(backend);
        let deadline = seal_locked(&mut state);
        (deadline, state.server.clone(), state.lifecycle.changed.clone())
    };
    if stop_startup {
        crate::startup::stop();
    }
    if let Some(server) = server {
        server.request_shutdown_until(deadline);
    }
    changed.notify_waiters();
    deadline
}

pub(super) async fn ensure_ready(app: &AppHandle) -> Result<(), StartupFailure> {
    let backend = app.state::<AppState>().backend.clone();
    let observed = lock(&backend).server.clone();
    if observed.as_ref().is_some_and(|server| server.wait_shutdown_requested().now_or_never().is_some()) {
        seal(&backend, true);
    }
    // 第一次await前占唯一启动资格；调用取消不会撤销已派发的独立任务。
    let start = {
        let mut state = lock(&backend);
        if let Some(Err(failure)) = &state.lifecycle.startup_result {
            return Err(failure.clone());
        }
        open(&state)?;
        if state.lifecycle.jobs[Role::Startup as usize].claimed {
            None
        } else {
            state.lifecycle.jobs[Role::Startup as usize].claimed = true;
            Some(JobGuard::new(backend.clone(), Role::Startup))
        }
    };
    ensure_coordinator(&backend);
    if let Some(guard) = start {
        startup::spawn(app.clone(), backend.clone(), guard);
    }
    let changed = notification(&backend);
    loop {
        let notified = changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let ready_server = {
            let state = lock(&backend);
            if let Some(Err(failure)) = &state.lifecycle.startup_result {
                return Err(failure.clone());
            }
            open(&state)?;
            state.lifecycle.ready.then(|| state.server.clone()).flatten()
        };
        if let Some(server) = ready_server {
            if server.wait_shutdown_requested().now_or_never().is_some() {
                seal(&backend, true);
                return Err(StartupFailure::other(CLOSING));
            }
            open(&lock(&backend))?;
            return Ok(());
        }
        notified.await;
    }
}

pub(super) fn request_shutdown(backend: &Backend) {
    seal(backend, true);
    ensure_coordinator(backend);
}

pub(super) async fn shutdown_and_wait(backend: &Backend) -> Result<(), String> {
    request_shutdown(backend);
    let changed = notification(backend);
    let deadline = match lock(backend).lifecycle.deadline {
        Some(deadline) => deadline,
        None => return Err("关闭申请未登记，未建立新的等待期限".into()),
    };
    let mut startup_deadline_checked = false;
    loop {
        let notified = changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if let Some(result) = lock(backend).lifecycle.close_result.clone() {
            // 未poll就被丢弃的协调器也有明确Err；尚在运行的初始化始终仍由原状态持有。
            return result;
        }
        if !startup_deadline_checked {
            if Instant::now() >= deadline {
                let pending = {
                    let state = lock(backend);
                    [Role::Startup, Role::Bootstrap].iter().any(|role| {
                        let job = &state.lifecycle.jobs[*role as usize];
                        job.claimed && (!job.released || !job.retained)
                    })
                };
                if pending {
                    pending_close(backend, "启动或阻塞初始化");
                    continue;
                }
                startup_deadline_checked = true;
            } else {
                tokio::select! {
                    _ = notified => {},
                    _ = tokio::time::sleep_until(deadline) => {},
                }
                continue;
            }
        }
        notified.await;
    }
}

pub(super) fn running_port(backend: &Backend) -> Option<u16> {
    let (port, server) = {
        let state = lock(backend);
        if state.lifecycle.poisoned || !state.lifecycle.ready || state.lifecycle.deadline.is_some() {
            return None;
        }
        (state.port, state.server.clone()?)
    };
    // 只查本地共享停止状态，不发健康请求；监听已开始关闭时不能沿用上次ready。
    if server.wait_shutdown_requested().now_or_never().is_some() {
        return None;
    }
    let state = lock(backend);
    (state.lifecycle.ready && state.lifecycle.deadline.is_none() && !state.lifecycle.poisoned)
        .then_some(port)
}

fn ensure_coordinator(backend: &Backend) {
    let guard = {
        let mut state = lock(backend);
        let job = &mut state.lifecycle.jobs[Role::Coordinator as usize];
        if job.claimed {
            return;
        }
        job.claimed = true;
        JobGuard::new(backend.clone(), Role::Coordinator)
    };
    let runner = backend.clone();
    spawn_async(backend, Role::Coordinator, async move {
        stopping::coordinate(runner, guard).await;
    });
}

// Tauri 2.11.5公开的inner()就是应用所选Tokio句柄，不使用服务器库的惰性运行时。
fn runtime() -> tokio::runtime::Handle {
    tauri::async_runtime::handle().inner().clone()
}

fn spawn_async(backend: &Backend, role: Role, future: impl Future<Output = ()> + Send + 'static) {
    let (begin, ready) = oneshot::channel();
    // 先登记JoinHandle再开闩；未首次poll或装配中断时future里预先建立的守卫仍有效。
    let watched = backend.clone();
    let task = async move {
        if ready.await.is_ok() {
            let result = std::panic::AssertUnwindSafe(future).catch_unwind().await;
            if result.is_err() {
                record_join_failure(&watched, role);
            }
        }
    };
    let spawned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime().spawn(task)));
    if let Ok(job) = spawned {
        {
            let mut state = lock(backend);
            state.lifecycle.jobs[role as usize].handle = Some(job);
            state.lifecycle.jobs[role as usize].retained = true;
        }
        notification(backend).notify_waiters();
        let _ = begin.send(());
    } else {
        lock(backend).lifecycle.jobs[role as usize].retained = true;
        record_join_failure(backend, role);
    }
}

fn record_join_failure(backend: &Backend, role: Role) {
    let changed = {
        let mut state = lock(backend);
        state.lifecycle.jobs[role as usize].lost = true;
        seal_locked(&mut state);
        let message = format!("{}异常退出，关闭状态未确认", role.label());
        if matches!(role, Role::Coordinator | Role::Startup | Role::Bootstrap) {
            state.lifecycle.close_result.get_or_insert_with(|| Err(message));
        }
        state.lifecycle.changed.clone()
    };
    changed.notify_waiters();
}

fn publish_close(backend: &Backend, result: Result<(), String>) {
    let changed = {
        let mut state = lock(backend);
        if state.lifecycle.close_result.is_none() {
            state.lifecycle.close_result = Some(result);
        }
        state.lifecycle.changed.clone()
    };
    changed.notify_waiters();
}

fn pending_close(backend: &Backend, stage: &str) {
    publish_close(backend, Err(format!(
        "关闭等待已到首次55秒截止，{stage}仍未确认退出；保留原任务和运行时，不代表清理完成"
    )));
}

// 析构只写状态并唤醒；不联网、不spawn、不清会话。guard在future/阻塞闭包外先建立。
struct JobGuard {
    backend: Backend,
    role: Role,
    completed: bool,
}

impl JobGuard {
    fn new(backend: Backend, role: Role) -> Self {
        Self { backend, role, completed: false }
    }

    fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        let changed = {
            let mut state = lock(&self.backend);
            let job = &mut state.lifecycle.jobs[self.role as usize];
            job.released = true;
            job.released_at = Some(Instant::now());
            if !self.completed {
                job.lost = true;
                seal_locked(&mut state);
                if matches!(self.role, Role::Startup | Role::Bootstrap) {
                    state.lifecycle.close_result.get_or_insert_with(|| Err(format!(
                        "{}在确认完成前消失；仍保留其他运行任务，不能据此退出或重启", self.role.label()
                    )));
                }
                match self.role {
                    Role::Startup => {
                        let failure = StartupFailure::other(LOST);
                        state.lifecycle.startup_result = Some(Err(failure.clone()));
                        state.failure = Some(failure);
                    }
                    Role::Bootstrap => {}
                    Role::Coordinator => {
                        state.lifecycle.close_result.get_or_insert_with(|| Err(
                            "桌面关闭协调在确认完成前消失；保留原任务，不能据此退出或重启".into()
                        ));
                    }
                    Role::Cleanup => {
                        state.lifecycle.cleanup_result = Some(Err(
                            "服务器及插件清理任务在取得真实结果前消失，关闭状态未确认".into()
                        ));
                    }
                }
            }
            state.lifecycle.changed.clone()
        };
        changed.notify_waiters();
    }
}
