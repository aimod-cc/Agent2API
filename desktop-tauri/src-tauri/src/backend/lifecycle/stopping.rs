//! 独立关闭协调器：先关启动门，HTTP存在时复用其结果，无HTTP才关闭插件核心。
//! 慢初始化到期只报告未确认并继续持有；不abort阻塞闭包、不在超时后重置期限。

use futures::FutureExt;
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, Instant};

use super::{
    lock, notification, pending_close, publish_close, seal, spawn_async, Backend,
    JobGuard, Role,
};

pub(super) async fn coordinate(backend: Backend, guard: JobGuard) {
    let deadline = wait_request(&backend).await;
    // 锁外停止入口，不检查WindowState.exiting；已有begin_exit不能跳过真实收尾。
    crate::startup::stop();
    maybe_spawn_cleanup(&backend, deadline);
    let changed = notification(&backend);
    let mut deadline_observed = false;
    loop {
        let notified = changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        maybe_spawn_cleanup(&backend, deadline);
        let (startup_done, bootstrap_done, cleanup_done, cleanup_claimed) = {
            let state = lock(&backend);
            let done = |role: Role| {
                let job = &state.lifecycle.jobs[role as usize];
                !job.claimed || job.released
            };
            (done(Role::Startup), done(Role::Bootstrap), done(Role::Cleanup),
                state.lifecycle.jobs[Role::Cleanup as usize].claimed)
        };
        let hooks_pending = !lock(&backend).lifecycle.hooks_complete;
        if startup_done && bootstrap_done && hooks_pending {
            // bootstrap能在关闭之后才安装定时器；等原闭包退场后只在这里补一次同步收尾。
            crate::server::core::auto_checkin::stop_global();
            crate::server::core::providers::claude_subscription::shutdown();
            lock(&backend).lifecycle.hooks_complete = true;
        }
        if startup_done && bootstrap_done && cleanup_claimed && cleanup_done {
            let startup = join_job(&backend, Role::Startup, deadline).await;
            let bootstrap = join_job(&backend, Role::Bootstrap, deadline).await;
            let cleanup = join_job(&backend, Role::Cleanup, deadline).await;
            if startup && bootstrap && cleanup {
                let result = result(&backend);
                let may_clear = result.is_ok() && lock(&backend).lifecycle.close_result.is_none();
                if may_clear {
                    crate::panel_session::clear();
                }
                publish_close(&backend, result);
                guard.complete();
                return;
            }
        }
        if Instant::now() >= deadline && !deadline_observed {
            // plugin/HTTP清理自身必须走到真实Result；有未退出的启动闭包则只交付未确认。
            let untracked = {
                let state = lock(&backend);
                [Role::Startup, Role::Bootstrap, Role::Cleanup].iter().any(|role| {
                    let job = &state.lifecycle.jobs[*role as usize];
                    job.claimed && !job.retained
                })
            };
            if !startup_done || !bootstrap_done || untracked {
                pending_close(&backend, "启动、阻塞初始化或任务句柄登记");
            }
            deadline_observed = true;
        }
        if deadline_observed {
            notified.await;
        } else {
            tokio::select! {
                _ = notified => {},
                _ = sleep_until(deadline) => {},
            }
        }
    }
}

async fn wait_request(backend: &Backend) -> Instant {
    let changed = notification(backend);
    loop {
        let notified = changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let (deadline, server) = {
            let state = lock(backend);
            (state.lifecycle.deadline, state.server.clone())
        };
        if let Some(deadline) = deadline {
            return deadline;
        }
        if let Some(server) = server {
            tokio::select! {
                _ = notified => {},
                _ = server.wait_shutdown_requested() => return seal(backend, true),
            }
        } else {
            notified.await;
        }
    }
}

fn maybe_spawn_cleanup(backend: &Backend, deadline: Instant) {
    let guard = {
        let mut state = lock(backend);
        if state.lifecycle.jobs[Role::Cleanup as usize].claimed {
            return;
        }
        if state.server.is_none() && state.lifecycle.binding {
            // binding已领取但还未归还句柄；不能猜成“没有HTTP”并重复启动插件关闭。
            return;
        }
        if state.lifecycle.deadline.is_none() {
            return;
        }
        state.lifecycle.jobs[Role::Cleanup as usize].claimed = true;
        JobGuard::new(backend.clone(), Role::Cleanup)
    };
    let target = lock(backend).server.clone();
    let runner = backend.clone();
    // 先同步通知已存在的监听，再调度等待；后续等待者不拥有此任务的取消句柄。
    if let Some(server) = &target {
        server.request_shutdown_until(deadline);
    }
    spawn_async(backend, Role::Cleanup, async move {
        let work = async {
            match target {
                Some(server) => server.wait_shutdown().await,
                None => crate::server::core::plugin_host::shutdown_all_until(deadline).await,
            }
        };
        let result = std::panic::AssertUnwindSafe(work).catch_unwind().await
            .unwrap_or_else(|_| Err("服务器或插件清理异常中断，关闭状态未确认".into()));
        {
            let mut state = lock(&runner);
            state.lifecycle.cleanup_result = Some(result);
        }
        notification(&runner).notify_waiters();
        guard.complete();
    });
}

fn result(backend: &Backend) -> Result<(), String> {
    let state = lock(backend);
    let mut errors = Vec::new();
    if state.lifecycle.poisoned {
        errors.push("服务器状态锁异常，未确认全部清理".to_string());
    }
    if state.lifecycle.binding_lost {
        errors.push("监听装配异常中断，可能存在尚未交回的任务".to_string());
    }
    for role in [Role::Startup, Role::Bootstrap, Role::Cleanup] {
        let job = &state.lifecycle.jobs[role as usize];
        if job.claimed && (!job.released || !job.retained || !job.joined || job.lost) {
            errors.push(format!("{}未确认正常退场", role.label()));
        }
        if !matches!(role, Role::Cleanup) && job.released_at.zip(state.lifecycle.deadline)
            .is_some_and(|(finished, deadline)| finished > deadline) {
            errors.push(format!("{}未在首次关闭截止前退场，迟到完成不改写超时结果", role.label()));
        }
    }
    if !state.lifecycle.hooks_complete {
        errors.push("桌面同步收尾尚未完成".to_string());
    }
    match &state.lifecycle.cleanup_result {
        Some(Ok(())) => {},
        Some(Err(error)) => errors.push(error.clone()),
        None => errors.push("尚未取得服务器或插件的真实关闭结果".to_string()),
    }
    if errors.is_empty() { Ok(()) } else { Err(errors.join("；")) }
}

// 只从原状态取走已有JoinHandle；若协调器在await中消失，Drop把未确认任务放回原槽。
struct WaitingJob {
    backend: Backend,
    role: Role,
    handle: Option<JoinHandle<()>>,
}

impl Drop for WaitingJob {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            lock(&self.backend).lifecycle.jobs[self.role as usize].handle = Some(handle);
        }
    }
}

async fn join_job(backend: &Backend, role: Role, deadline: Instant) -> bool {
    let handle = {
        let mut state = lock(backend);
        let job = &mut state.lifecycle.jobs[role as usize];
        if !job.claimed || job.joined {
            return true;
        }
        if !job.retained {
            return false;
        }
        match job.handle.take() {
            Some(handle) => handle,
            None if job.lost => {
                job.joined = true;
                return true;
            }
            None => return false,
        }
    };
    let mut waiting = WaitingJob { backend: backend.clone(), role, handle: Some(handle) };
    let Some(handle) = waiting.handle.as_mut() else {
        pending_close(backend, role.label());
        return false;
    };
    let outcome = if Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, &mut *handle).await {
            Ok(outcome) => outcome,
            Err(_) => {
                pending_close(backend, role.label());
                handle.await
            }
        }
    } else {
        if !handle.is_finished() {
            pending_close(backend, role.label());
        }
        handle.await
    };
    waiting.handle.take();
    let mut state = lock(backend);
    let job = &mut state.lifecycle.jobs[role as usize];
    job.joined = true;
    job.lost |= outcome.is_err();
    true
}
