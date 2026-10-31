use std::sync::{Arc, Mutex};

use crate::server::errors::GatewayError;

#[derive(Default)]
struct State {
    users: usize,
    paused: bool,
    retired: bool,
}

#[derive(Default)]
pub(super) struct Activity {
    state: Mutex<State>,
}

impl Activity {
    pub fn acquire(self: &Arc<Self>) -> Option<Arc<UsageLease>> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.paused || state.retired { return None; }
        state.users += 1;
        Some(Arc::new(UsageLease { activity: self.clone() }))
    }

    pub fn pause(self: &Arc<Self>) -> Option<PauseGuard> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.users != 0 || state.paused { return None; }
        state.paused = true;
        Some(PauseGuard { activity: self.clone() })
    }

    pub fn retire(&self) {
        self.state.lock().unwrap_or_else(|error| error.into_inner()).retired = true;
    }

    pub fn retired(&self) -> bool {
        self.state.lock().unwrap_or_else(|error| error.into_inner()).retired
    }
}

pub(super) struct UsageLease {
    activity: Arc<Activity>,
}

impl Drop for UsageLease {
    fn drop(&mut self) {
        let mut state = self.activity.state.lock().unwrap_or_else(|error| error.into_inner());
        state.users = state.users.saturating_sub(1);
    }
}

pub(super) struct PauseGuard {
    activity: Arc<Activity>,
}

impl Drop for PauseGuard {
    fn drop(&mut self) {
        self.activity.state.lock().unwrap_or_else(|error| error.into_inner()).paused = false;
    }
}

pub(super) struct OpeningStream {
    pub host: Arc<super::Host>,
    pub id: Option<i64>,
    pub written: Option<tokio::sync::oneshot::Receiver<bool>>,
}

impl Drop for OpeningStream {
    fn drop(&mut self) {
        let Some(id) = self.id else { return };
        let host = self.host.clone();
        let written = self.written.take();
        host.close_stream(id);
        crate::spawn_task(async move {
            // 取消帧必须排在请求帧之后；尚未写出的调用不能永远占住在途凭证。
            if let Some(written) = written {
                if !written.await.unwrap_or(false) {
                    if let Ok(mut streams) = host.streams.lock() { streams.remove(&id); }
                    return;
                }
            }
            host.abort(id).await;
        });
    }
}

pub(super) enum AvailabilityError {
    Updating,
    Unavailable,
}

impl AvailabilityError {
    pub fn gateway(self) -> GatewayError {
        match self {
            Self::Updating => GatewayError::with_status(503, "插件正在更新，等待切换超时，请稍后重试")
                .with_code("plugin_updating"),
            Self::Unavailable => GatewayError::with_status(503, "提供商插件当前不可用，请检查插件状态")
                .with_code("plugin_unavailable"),
        }
    }
}
