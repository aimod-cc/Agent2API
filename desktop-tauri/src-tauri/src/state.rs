//! 应用共享状态。
//!
//! 只放可变、跨命令共享的部分：进程内服务器句柄、登录会话、窗口显示偏好
//! 与独立的最终退出协调状态。
//! 窗口句柄不在这里 —— 由 Tauri 的 `AppHandle::get_webview_window` 按标签查找，
//! 避免自己维护一份可能失同步的副本。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::port_conflict::StartupFailure;

/// 本进程服务器句柄及桌面启动/关闭协调状态；关闭结果不能由句柄是否为空推断。
#[derive(Default)]
pub struct BackendHandle {
    /// 即使关闭失败仍保留；只用真实关闭结果决定能否进入最终退出。
    pub server: Option<crate::server::ServerHandle>,
    pub(crate) lifecycle: crate::backend::lifecycle::Lifecycle,
    pub port: u16,
    /// 最近一次启动失败（成功启动后清空）。
    ///
    /// 为什么要存下来：`backend:error` 是一次性事件，而界面可能在事件发出**之后**
    /// 才订阅（窗口还没加载完、用户刷新了 WebView）。只靠事件的话，用户重启界面
    /// 就再也看不到「为什么起不来」——只能看到一个永远灰着的状态灯。
    /// 存一份让界面随时能查，事件只负责「提醒你现在就去查」。
    pub failure: Option<StartupFailure>,
}

/// 一次进行中的登录：state 用于轮询后端，edition/mode 供界面展示与取消时判断。
#[derive(Clone)]
pub struct ActiveLogin {
    pub state: String,
    pub edition: String,
    pub mode: String,
    /// 这次登录属于哪一家（provider id）。
    ///
    /// 壳侧为什么也要记：登录窗口的导航拦截要判断「这个 URL 是不是本家的回调」，
    /// 而各家的回调协议不同（小浣熊是 `office-raccoon://auth/callback`，
    /// workbuddy 没有回调 —— 它的凭证由后端轮询上游拿）。把 provider 记在
    /// 活动登录上，拦截逻辑就不必猜、也不必让前端多传一个参数。
    pub provider: String,
}

/// 与窗口显示有关的开关；用户退出意图和最终放行由 exit 协调器分别持有。
/// 同步窗口回调只读取缓存，不读磁盘。
#[derive(Default)]
pub struct WindowState {
    pub close_to_tray: AtomicBool,
    /// 轻量模式：关窗时销毁窗口（释放 WebView2 内存）而不是隐藏
    pub lightweight: AtomicBool,
    /// 窗口重建期间保留托盘请求，页面就绪后只领取一次。
    pending_update_check: AtomicBool,
}

impl WindowState {
    pub fn set_close_to_tray(&self, value: bool) {
        self.close_to_tray.store(value, Ordering::SeqCst);
    }

    pub fn close_to_tray(&self) -> bool {
        self.close_to_tray.load(Ordering::SeqCst)
    }

    pub fn set_lightweight(&self, value: bool) {
        self.lightweight.store(value, Ordering::SeqCst);
    }

    pub fn lightweight(&self) -> bool {
        self.lightweight.load(Ordering::SeqCst)
    }

    pub fn request_update_check(&self) {
        self.pending_update_check.store(true, Ordering::SeqCst);
    }

    pub fn take_update_check_request(&self) -> bool {
        self.pending_update_check.swap(false, Ordering::SeqCst)
    }
}

#[derive(Default)]
pub struct AppState {
    pub backend: Arc<Mutex<BackendHandle>>,
    pub login: Mutex<Option<ActiveLogin>>,
    pub window: WindowState,
    pub(crate) exit: Arc<crate::exit::Coordinator>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
}
