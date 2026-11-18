//! Windows 服务模式：把 headless 网关交给 SCM（服务控制管理器）托管。
//!
//! ── 一个 exe，两种起法 ──────────────────────────────────────
//!   · `agent2api-server.exe`            前台跑（开发调试 / 手动试跑）；
//!   · `agent2api-server.exe --service`  由 SCM 拉起（install-service.ps1
//!     注册的 ImagePath 就带这个参数）。
//!
//! 只认 `--service` 这一个开关，其余参数不解析 —— 服务形态的所有配置都走
//! 环境变量（随注册表 ImagePath 一起写进去，见 install-service.ps1）。
//!
//! ── 为什么不自动探测「是不是被 SCM 拉起的」──────────────────
//! 「直接试 StartServiceCtrlDispatcher、失败就退回前台」是个常见做法，但对
//! 排障不友好：用户在资源管理器里双击 exe 时那条路必然失败，程序转成前台
//! 却**没有任何窗口**（headless 本来就没界面），用户看到的是「双击了没
//! 反应」。所以这里要求显式 `--service`：双击 = 前台跑、日志打到 stderr
//! （从 cmd / PowerShell 启动时看得见），注册成服务 = 带参数。两种意图分得开。
//!
//! ── 与前台模式的差异 ────────────────────────────────────────
//!   · 控制台：服务进程没有控制台，`eprintln!` 全部落空 —— 所以调用方在
//!     进 SCM 之前先打开日志文件通道（`logging::init_file_sink`），否则
//!     服务会「启动成功但什么都看不见」；
//!   · 停机：SCM 的停止控制事件与面板重启共用可取消的异步通知；先报
//!     StopPending，再等待同一ServerHandle的真实收尾结果，最后报告Stopped。
//!
//! ── 工作目录 ────────────────────────────────────────────────
//! 服务进程的 CWD 是 `C:\Windows\system32`，不是 exe 所在目录：
//!   · ui/ 靠 `current_exe()` 定位（`resolve_ui_dir` 已如此，不受影响）；
//!   · 数据目录靠 `AGENT2API_PROXY_HOME`（**必须**在注册服务时显式设置，
//!     否则会落到 LocalSystem 的 `USERPROFILE` 下 —— 见 install-service.ps1）。
//!
//! ── 控制流为什么不放在回调里 ────────────────────────────────
//! SCM 的控制回调跑在 SCM 自己的线程上，里面做耗时操作会卡住事件分发
//! （连「停止」都收不到第二次）。所以回调只做两件事：转停机信号、按 SCM
//! 要求上报状态；真正的收尾由网关主线程完成。
//!
//! ── 就绪判定为什么由网关回报 ────────────────────────────────
//! 「进程起来了」不等于「服务可用」：开库、绑端口任一步失败，SCM 都应看到
//! 启动失败而不是先 Running 再闪退（后者会触发恢复策略反复拉起）。所以
//! 网关在**端口绑好、开始接受请求**后报告就绪；上报Running失败也必须
//! 先收尾再退出。最终服务退出码和进程退出码都保留真实失败，不能静默归零。
//!
//! 本模块只在 `cfg(windows)` 下编译，调用点由 `bin/agent2api-server.rs`
//! 用 `#[cfg(windows)]` 圈住。

use std::ffi::OsString;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use agent2api_server::server::logging;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_dispatcher;

use super::ShutdownSignal;

/// 服务名：与 install-service.ps1 里 `sc.exe create` 的名字**必须逐字一致**
/// （SCM 按名字把进程与注册项对上；不一致时服务会以 1053 启动失败）。
pub const SERVICE_NAME: &str = "Agent2API";

/// 启动提示与首次状态通知的等待上限；超时只请求停止，不把线程当作已退出。
const START_HINT: Duration = Duration::from_secs(30);

/// 仅是SCM状态提示，含少量状态传递余量；不是第二个关闭计时器。
/// 真正排空期限只继承首次停止请求，系统关机仍可能提前结束进程。
const STOP_HINT: Duration = Duration::from_secs(60);

/// 命令行走服务模式（`--service`，大小写不敏感，容忍前后空白）
pub fn requested() -> bool {
    std::env::args()
        .skip(1)
        .any(|arg| arg.trim().eq_ignore_ascii_case("--service"))
}

/// SCM同步回调与网关共用watch通知；没有spawn_blocking或永久recv等待。
pub struct StopSignal(ShutdownSignal);

impl StopSignal {
    pub(super) fn signal(&self) -> ShutdownSignal {
        self.0.clone()
    }
}

/// 固定状态槽串行化Running/StopPending/Stopped，迟到就绪不能覆盖停止。
struct StatusState {
    handle: Option<ServiceStatusHandle>,
    current: ServiceState,
    stop_reported: bool,
    error: Option<String>,
}

#[derive(Clone)]
pub struct Progress {
    state: Arc<Mutex<StatusState>>,
    stop: ShutdownSignal,
    settled: mpsc::SyncSender<()>,
}

impl Progress {
    fn install(&self, handle: ServiceStatusHandle) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.handle = Some(handle);
        let stopping = self.stop.current().is_some();
        let next = if stopping {
            ServiceState::StopPending
        } else {
            ServiceState::StartPending
        };
        let hint = if stopping { STOP_HINT } else { START_HINT };
        let result = Self::report(&mut state, status(next, 1, hint));
        if stopping && result.is_ok() {
            state.stop_reported = true;
        }
        result
    }

    pub(super) fn ready(&self) -> Result<(), String> {
        let result = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if self.stop.current().is_some() || state.current != ServiceState::StartPending {
                Err("就绪前已开始停止，不再报告Running".into())
            } else {
                Self::report(&mut state, status(ServiceState::Running, 0, Duration::ZERO))
            }
        };
        let _ = self.settled.try_send(());
        if result.is_ok() && self.stop.current().is_none() {
            agent2api_server::server::core::update::windows_service::notify_ready();
        }
        result
    }

    pub(super) fn stopping(&self) -> Result<(), String> {
        self.stop.request("Windows服务开始关闭");
        let result = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.current == ServiceState::Stopped || state.stop_reported {
                Ok(())
            } else if state.handle.is_none() {
                // 注册回调与保存状态句柄之间收到STOP；install随后补报，不能漏信号。
                state.current = ServiceState::StopPending;
                Ok(())
            } else {
                let result =
                    Self::report(&mut state, status(ServiceState::StopPending, 1, STOP_HINT));
                state.stop_reported = result.is_ok();
                result
            }
        };
        let _ = self.settled.try_send(());
        result
    }

    fn stopped(&self, outcome: Result<(), String>) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let status_error = state.error.clone().map_or(Ok(()), Err);
        let outcome = super::combine_results(outcome, status_error);
        let code = if outcome.is_ok() {
            ServiceExitCode::NO_ERROR
        } else {
            ServiceExitCode::ServiceSpecific(1)
        };
        // SCM收到Stopped后可能立即结束进程：结果和退出码证据必须在此前保存。
        store_service_result(&outcome);
        logging::console_line(
            "[Service]",
            &format!("即将报告Stopped，服务退出码={code:?}"),
        );
        let reported = Self::report(
            &mut state,
            status_with_exit(ServiceState::Stopped, 0, Duration::ZERO, code),
        );
        let report_failed = reported.is_err();
        let outcome = super::combine_results(outcome, reported);
        if report_failed {
            store_service_result(&outcome);
        }
        outcome
    }

    fn report(state: &mut StatusState, status: ServiceStatus) -> Result<(), String> {
        let next = status.current_state;
        let result = state
            .handle
            .ok_or_else(|| "服务状态句柄尚未就绪".to_string())
            .and_then(|handle| {
                handle
                    .set_service_status(status)
                    .map_err(|error| format!("上报 {next:?} 失败：{error}"))
            });
        // 即便上报失败，本地也不能倒退回Running；首个错误保留到最终服务结果。
        state.current = next;
        if let Err(error) = &result {
            state.error.get_or_insert_with(|| error.clone());
            logging::console_line("[Service]", error);
        } else if next != ServiceState::Stopped {
            logging::console_line("[Service]", &format!("已报告 {next:?}"));
        }
        result
    }
}

/// 工作线程拥有运行时，从启动直到取得真实清理结果后才返回。
pub type StartFn = Box<dyn FnOnce(StopSignal, Progress) -> Result<(), String> + Send>;

/// 进程级启动槽位：SCM 的 service_main 是 `extern "system" fn`，不能捕获
/// 环境，只能走静态槽位取值（进程内只会被调用一次）。
static START_FN: Mutex<Option<StartFn>> = Mutex::new(None);
static SERVICE_RESULT: Mutex<Option<Result<(), String>>> = Mutex::new(None);

fn store_service_result(outcome: &Result<(), String>) {
    *SERVICE_RESULT
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(outcome.clone());
    match outcome {
        Ok(()) => logging::console_line(
            "[Service]",
            "服务收尾结果已确认；预定进程退出码=0，远端回收未验证",
        ),
        Err(reason) => logging::console_line(
            "[Service]",
            &format!("服务收尾失败；预定进程退出码=1：{reason}"),
        ),
    }
}

/// dispatcher返回成功只说明SCM连接结束，必须再取service_main的真实运行结果。
pub fn dispatch(start: StartFn) -> Result<(), String> {
    *START_FN.lock().map_err(|_| "启动槽位被污染".to_string())? = Some(start);
    *SERVICE_RESULT
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    let dispatched = service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|error| format!("连接服务控制管理器失败：{error}"));
    // dispatch未进入service_main时也释放启动闭包，不遗留跨轮状态。
    START_FN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    match SERVICE_RESULT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    {
        Some(outcome) => super::combine_results(outcome, dispatched),
        None => dispatched.and(Err("服务入口未返回可确认的运行结果".into())),
    }
}

windows_service::define_windows_service!(ffi_service_main, service_main);

/// SCM 调用的真正入口：注册控制回调、上报状态、跑网关、收尾上报。
fn service_main(_arguments: Vec<OsString>) {
    let outcome = std::panic::catch_unwind(run_service)
        .unwrap_or_else(|_| Err("Windows服务入口异常，关闭状态未确认".into()));
    // 正常路径在Stopped之前已保存；这里只兜住尚无结果的入口/注册异常。
    let missing = SERVICE_RESULT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_none();
    if missing {
        store_service_result(&outcome);
    }
}

fn run_service() -> Result<(), String> {
    let stop = ShutdownSignal::new();
    let (settled_tx, settled_rx) = mpsc::sync_channel(1);
    let progress = Progress {
        state: Arc::new(Mutex::new(StatusState {
            handle: None,
            current: ServiceState::StartPending,
            stop_reported: false,
            error: None,
        })),
        stop: stop.clone(),
        settled: settled_tx,
    };
    let control_progress = progress.clone();
    let event_handler = move |control| -> ServiceControlHandlerResult {
        let source = match control {
            ServiceControl::Interrogate => return ServiceControlHandlerResult::NoError,
            ServiceControl::Stop => "Windows服务STOP",
            ServiceControl::Shutdown => "Windows系统关机",
            ServiceControl::Preshutdown => "Windows系统预关机",
            _ => return ServiceControlHandlerResult::NotImplemented,
        };
        // 回调只登记首关请求和pending，不等待任何HTTP/插件清理。
        control_progress.stop.request(source);
        let _ = control_progress.stopping();
        ServiceControlHandlerResult::NoError
    };
    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .map_err(|error| format!("注册服务控制回调失败：{error}"))?;
    let outcome = (|| {
        progress.install(status_handle)?;
        let start = START_FN
            .lock()
            .map_err(|_| "启动槽位被污染".to_string())?
            .take()
            .ok_or_else(|| "未找到网关启动逻辑".to_string())?;
        let worker_progress = progress.clone();
        let completed = progress.settled.clone();
        let worker = std::thread::Builder::new()
            .name("agent2api-gateway".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    start(StopSignal(stop), worker_progress)
                }))
                .unwrap_or_else(|_| Err("网关线程异常退出，关闭状态未确认".into()));
                // 参数/运行时创建失败也唤醒启动等待，不能丢失真实错误后再等30秒。
                let _ = completed.try_send(());
                result
            })
            .map_err(|error| format!("创建网关线程失败：{error}"))?;

        // 只在SCM入口线程做一次有界启动等待，不占Tokio阻塞池。
        // 就绪、开始清理、启动提前失败都解除这个等待，清理中不能被误判为启动超时。
        let startup = match settled_rx.recv_timeout(START_HINT) {
            Ok(()) => Ok(()),
            Err(error) => {
                let reason = match error {
                    mpsc::RecvTimeoutError::Timeout => "网关启动未在宽限期内就绪",
                    mpsc::RecvTimeoutError::Disconnected => "网关启动通知意外中断",
                }
                .to_string();
                progress
                    .stop
                    .request_with_error("Windows服务启动失败", Some(reason.clone()));
                let _ = progress.stopping();
                Err(reason)
            }
        };
        // 所有启动/状态上报失败都等同一worker清理结束，绝不丢掉线程就报Stopped。
        let worker_result = worker
            .join()
            .unwrap_or_else(|_| Err("网关线程异常退出，关闭状态未确认".into()));
        super::combine_results(worker_result, startup)
    })();
    progress.stopped(outcome)
}

/// 上报状态。`checkpoint` 只在 pending 态非零。
fn status(state: ServiceState, checkpoint: u32, wait_hint: Duration) -> ServiceStatus {
    status_with_exit(state, checkpoint, wait_hint, ServiceExitCode::NO_ERROR)
}

fn status_with_exit(
    state: ServiceState,
    checkpoint: u32,
    wait_hint: Duration,
    exit_code: ServiceExitCode,
) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        // pending/Stopped都不再接新控制；停止回调和迟到就绪由同一状态锁排顺序。
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code,
        checkpoint,
        wait_hint,
        process_id: None,
    }
}
