//! 唯一启动任务及阻塞初始化。取消等待者不丢任务，关门后只能交回原句柄供清理。

use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;

use futures::FutureExt;
use tauri::AppHandle;
use tokio::sync::Notify;

use crate::port_conflict::StartupFailure;
use crate::server::{self, ServerState};

use super::{lock, notification, open, runtime, seal, spawn_async, Backend, JobGuard, Role, CLOSING};

pub(in crate::backend) struct Startup {
    backend: Backend,
}

pub(super) fn spawn(app: AppHandle, backend: Backend, guard: JobGuard) {
    let runner = backend.clone();
    spawn_async(&backend, Role::Startup, async move {
        let start = Startup { backend: runner.clone() };
        let result = std::panic::AssertUnwindSafe(crate::backend::start_once(&app, &start))
            .catch_unwind().await;
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                lock(&runner).lifecycle.jobs[Role::Startup as usize].lost = true;
                Err(StartupFailure::other("桌面网关启动异常中断，启动状态未确认"))
            }
        };
        finish(&runner, result);
        guard.complete();
    });
}

fn finish(backend: &Backend, result: Result<(), StartupFailure>) {
    let server = lock(backend).server.clone();
    let stopping = server.as_ref().is_some_and(|server| server.wait_shutdown_requested().now_or_never().is_some());
    let (published, changed) = {
        let mut state = lock(backend);
        let result = result.and_then(|()| {
            open(&state)?;
            if stopping {
                return Err(StartupFailure::other(CLOSING));
            }
            if state.server.is_none() {
                return Err(StartupFailure::other("启动未交付服务器句柄，不能发布就绪"));
            }
            Ok(())
        });
        state.lifecycle.ready = result.is_ok();
        state.failure = result.as_ref().err().cloned();
        state.lifecycle.startup_result = Some(result.clone());
        (result, state.lifecycle.changed.clone())
    };
    if published.is_err() {
        // 保留原启动失败分类供UI读取；收尾失败另放关闭结果，不覆盖端口冲突信息。
        seal(backend, false);
    }
    // 启动状态不依赖外层等待者还活着；与同步close共用同一门，不发布迟到Ready。
    {
        let state = lock(backend);
        if published.is_ok() && state.lifecycle.deadline.is_some() {
            crate::startup::stop();
        } else {
            crate::startup::complete(&published);
        }
    }
    changed.notify_waiters();
}

impl Startup {
    pub(in crate::backend) fn check_open(&self) -> Result<(), StartupFailure> {
        let server = {
            let state = lock(&self.backend);
            open(&state)?;
            state.server.clone()
        };
        if server.as_ref().is_some_and(|server| server.wait_shutdown_requested().now_or_never().is_some()) {
            return Err(StartupFailure::other(CLOSING));
        }
        Ok(())
    }

    pub(in crate::backend) async fn until_closing<T>(
        &self,
        work: impl Future<Output = T>,
    ) -> Result<T, StartupFailure> {
        self.check_open()?;
        tokio::select! {
            biased;
            _ = closing(&self.backend) => Err(StartupFailure::other(CLOSING)),
            result = work => {
                self.check_open()?;
                Ok(result)
            }
        }
    }

    pub(in crate::backend) async fn bootstrap(
        &self,
        port: u16,
        host: IpAddr,
    ) -> Result<ServerState, StartupFailure> {
        self.check_open()?;
        let runtime = runtime();
        let context = runtime.clone();
        let guard = {
            let mut state = lock(&self.backend);
            open(&state)?;
            let job = &mut state.lifecycle.jobs[Role::Bootstrap as usize];
            if job.claimed {
                return Err(StartupFailure::other("初始化资格已经使用，未重复初始化服务器"));
            }
            job.claimed = true;
            JobGuard::new(self.backend.clone(), Role::Bootstrap)
        };
        let backend = self.backend.clone();
        let spawned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            runtime.spawn_blocking(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // 显式继承Tauri选择的上下文，bootstrap内部spawn_task不会落到另一运行时。
                    let _entered = context.enter();
                    open(&lock(&backend))?;
                    server::ServerState::bootstrap(port, host).map_err(StartupFailure::other)
                }));
                let result = match result {
                    Ok(result) => result,
                    Err(_) => {
                        lock(&backend).lifecycle.jobs[Role::Bootstrap as usize].lost = true;
                        Err(StartupFailure::other("网关阻塞初始化异常中断，已停止后续启动"))
                    }
                };
                {
                    let mut state = lock(&backend);
                    state.lifecycle.bootstrap_result = Some(result);
                }
                notification(&backend).notify_waiters();
                guard.complete();
            })
        }));
        if let Ok(job) = spawned {
            let mut state = lock(&self.backend);
            state.lifecycle.jobs[Role::Bootstrap as usize].handle = Some(job);
            state.lifecycle.jobs[Role::Bootstrap as usize].retained = true;
        } else {
            lock(&self.backend).lifecycle.jobs[Role::Bootstrap as usize].retained = true;
        }
        notification(&self.backend).notify_waiters();
        let changed = notification(&self.backend);
        loop {
            let notified = changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = lock(&self.backend);
                open(&state)?;
                if state.lifecycle.jobs[Role::Bootstrap as usize].released {
                    return state.lifecycle.bootstrap_result.as_ref().cloned().unwrap_or_else(|| {
                        Err(StartupFailure::other("阻塞初始化在交付结果前消失，未继续启动"))
                    });
                }
            }
            notified.await;
        }
    }

    pub(in crate::backend) fn bind(&self, server_state: &ServerState, port: u16) -> Result<(), StartupFailure> {
        {
            let mut state = lock(&self.backend);
            open(&state)?;
            if state.server.is_some() || state.lifecycle.binding
                || state.lifecycle.jobs[Role::Cleanup as usize].claimed {
                return Err(StartupFailure::other("服务器监听已经装配，未重复绑定端口"));
            }
            // 与关闭目标选择共用门；登记后若关闭到达，协调器等本次绑定交回结果。
            state.lifecycle.binding = true;
        }
        let mut binding = BindingGuard { backend: self.backend.clone(), completed: false };
        let server = server::start_managed(server_state).map_err(|conflict| {
            StartupFailure::port(conflict.with_occupant(crate::backend::occupant_of(port)))
        })?;
        let server_wait = server.clone();
        let deadline = {
            let mut state = lock(&self.backend);
            // 从start_managed返回至此没有await，所得句柄永远交给原协调状态，不丢迟到句柄。
            state.server = Some(server);
            state.port = port;
            state.lifecycle.deadline
        };
        binding.complete();
        if let Some(deadline) = deadline {
            server_wait.request_shutdown_until(deadline);
            return Err(StartupFailure::other(CLOSING));
        }
        self.check_open()
    }
}

async fn closing(backend: &Backend) {
    let changed: Arc<Notify> = notification(backend);
    loop {
        let notified = changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let (stopping, server) = {
            let state = lock(backend);
            (state.lifecycle.deadline.is_some(), state.server.clone())
        };
        if stopping {
            return;
        }
        if let Some(server) = server {
            tokio::select! {
                _ = notified => {},
                _ = server.wait_shutdown_requested() => return,
            }
        } else {
            notified.await;
        }
    }
}

struct BindingGuard {
    backend: Backend,
    completed: bool,
}

impl BindingGuard {
    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for BindingGuard {
    fn drop(&mut self) {
        let changed = {
            let mut state = lock(&self.backend);
            state.lifecycle.binding = false;
            if !self.completed && std::thread::panicking() {
                state.lifecycle.binding_lost = true;
                super::seal_locked(&mut state);
            }
            state.lifecycle.changed.clone()
        };
        changed.notify_waiters();
    }
}
