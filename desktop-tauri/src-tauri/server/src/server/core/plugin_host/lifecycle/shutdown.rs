//! Host关闭协调：同步关准入与通知先完成；等待启动锁、普通WASM及受控worker共用最早截止。
//! 只保留有限状态，不创建清理监听任务；超时后的宿主/Runtime仍由原登记持有。

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::{
    sync::Notify,
    time::{sleep_until, Instant},
};

use crate::server::core::plugin_host::{wasm::managed_task::shutdown as managed, Host};

pub(in crate::server::core::plugin_host) const TIMEOUT: Duration = Duration::from_secs(55);

#[derive(Clone, Copy)]
pub(in crate::server::core::plugin_host) struct Window {
    pub(in crate::server::core::plugin_host) deadline: Instant,
    pub(in crate::server::core::plugin_host) reason: managed::ShutdownReason,
}

#[derive(Default)]
pub(in crate::server::core::plugin_host) struct State {
    window: Mutex<Option<Window>>,
    batch: OnceLock<Arc<managed::DrainBatch>>,
    changed: Notify,
    deadline_reached: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
pub(in crate::server::core::plugin_host) struct Report {
    pub(in crate::server::core::plugin_host) local_exited: bool,
    pub(in crate::server::core::plugin_host) deadline_reached: bool,
    pub(in crate::server::core::plugin_host) ordinary_active: usize,
    pub(in crate::server::core::plugin_host) managed: managed::DrainReport,
}

impl Host {
    /// 调用者不能持运行时全局表锁；该锁会被CallGate在Host锁之前读取。
    pub(in crate::server::core::plugin_host) fn seal_shutdown(
        &self,
        reason: managed::ShutdownReason,
        deadline: Instant,
    ) -> Window {
        self.activity.retire();
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut window = self
            .embedded
            .shutdown
            .window
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let deadline = deadline.min(Instant::now() + TIMEOUT);
        let value = match *window {
            Some(old) => Window {
                deadline: old.deadline.min(deadline),
                reason: old.reason,
            },
            None => Window { deadline, reason },
        };
        *window = Some(value);
        self.closing.store(true, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        drop(window);
        // 与CallGate共用Host生命周期锁，先写真实原因再释放门，避免迟到revoke抢成另一原因。
        managed::signal_host(self, value.reason, value.deadline);
        self.embedded.shutdown.changed.notify_waiters();
        value
    }

    pub(in crate::server::core::plugin_host) fn notify_shutdown(&self) {
        let Some(window) = self.shutdown_window() else {
            return;
        };
        let batch = self
            .embedded
            .shutdown
            .batch
            .get_or_init(|| managed::host_batch(self, window.reason, window.deadline));
        // OnceLock等待期间可能又有更早截止的关闭；必须重新读取，而非沿用首次局部副本。
        let Some(window) = self.shutdown_window() else {
            return;
        };
        batch.tighten(window.reason, window.deadline);
        self.notify_wasm_shutdown(window.reason, window.deadline);
    }

    pub(in crate::server::core::plugin_host) fn shutdown_window(&self) -> Option<Window> {
        *self
            .embedded
            .shutdown
            .window
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(in crate::server::core::plugin_host) async fn closing_deadline(&self) {
        loop {
            let notified = self.embedded.shutdown.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let Some(window) = self.shutdown_window() else {
                return;
            };
            if Instant::now() >= window.deadline {
                self.embedded
                    .shutdown
                    .deadline_reached
                    .store(true, Ordering::Release);
                return;
            }
            tokio::select! {
                biased;
                _ = &mut notified => {},
                _ = sleep_until(window.deadline) => {
                    self.embedded.shutdown.deadline_reached.store(true, Ordering::Release);
                    return;
                },
            }
        }
    }

    /// 仅查看真实本地执行，不以WASM没有ProcessRun当作已经关闭。
    pub(in crate::server::core::plugin_host) fn local_shutdown_complete(&self) -> bool {
        if !self.closing.load(Ordering::Acquire) {
            return false;
        }
        let Ok(_starting) = self.starting.try_lock() else {
            return false;
        };
        if self.current_run().is_some() || self.embedded.active.load(Ordering::Acquire) != 0 {
            return false;
        }
        if self.options.wasm.is_some() {
            let Ok(runtime) = self.embedded.runtime.try_read() else {
                return false;
            };
            if runtime.is_some() {
                return false;
            }
        }
        self.embedded
            .shutdown
            .batch
            .get()
            .is_some_and(|batch| batch.report().reached_terminal())
    }

    pub(in crate::server::core::plugin_host) fn has_unsettled_instance(&self) -> bool {
        if self.closing.load(Ordering::Acquire) {
            return !self.local_shutdown_complete();
        }
        if self.current_run().is_some() || self.embedded.active.load(Ordering::Acquire) != 0 {
            return true;
        }
        if self.starting.try_lock().is_err() {
            return true;
        }
        self.embedded
            .runtime
            .try_read()
            .map_or(true, |runtime| runtime.is_some())
    }

    pub(in crate::server::core::plugin_host) async fn finish_shutdown(&self) -> Report {
        self.notify_shutdown();
        let Some(window) = self.shutdown_window() else {
            return Report::default();
        };
        let Some(batch) = self.embedded.shutdown.batch.get() else {
            return Report::default();
        };
        let ordinary = async {
            if self.local_shutdown_complete() {
                return true;
            }
            // starting锁等待算在首个关闭的绝对期限内；受控排空在旁路同时推进。
            let serial = tokio::select! {
                biased;
                _ = self.closing_deadline() => return false,
                guard = self.starting.lock() => guard,
            };
            self.notify_wasm_shutdown(window.reason, window.deadline);
            let stopped = tokio::select! {
                biased;
                _ = self.closing_deadline() => false,
                result = async {
                    if self.options.wasm.is_some() { self.wait_wasm_idle().await; }
                    else { self.shutdown_native().await; }
                } => { result; true },
            };
            drop(serial);
            stopped
        };
        let (_ordinary_done, managed) = tokio::join!(ordinary, batch.wait());
        // 无await的退出确认与移走Runtime；starting锁/本地活动未退出就保留全部登记。
        let mut local_exited = false;
        if managed.reached_terminal() {
            if let Ok(_serial) = self.starting.try_lock() {
                if self.current_run().is_none() && self.embedded.active.load(Ordering::Acquire) == 0
                {
                    if self.options.wasm.is_none() || self.release_wasm_runtime() {
                        self.running.store(false, Ordering::Release);
                        local_exited = true;
                    }
                }
            }
        }
        Report {
            local_exited,
            deadline_reached: managed.deadline_reached
                || self
                    .embedded
                    .shutdown
                    .deadline_reached
                    .load(Ordering::Acquire)
                || !local_exited
                    && self
                        .shutdown_window()
                        .is_some_and(|window| Instant::now() >= window.deadline),
            ordinary_active: self.embedded.active.load(Ordering::Acquire),
            managed,
        }
    }
}
