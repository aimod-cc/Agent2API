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
//!   · 停机：SCM 发 STOP / SHUTDOWN 控制事件，转成与 Ctrl+C / SIGTERM 同一条
//!     优雅停机路径（网关主循环收到信号 → `shutdown_tx` → 在途请求收尾）。
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
//! 网关在**端口绑好、开始接受请求**那一刻调 `Ready::signal()`，本模块据此
//! 上报 Running；网关在 signal 之前返回错误，就上报带退出码的 Stopped。
//!
//! 本模块只在 `cfg(windows)` 下编译，调用点由 `bin/agent2api-server.rs`
//! 用 `#[cfg(windows)]` 圈住。

use std::ffi::OsString;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

/// 服务名：与 install-service.ps1 里 `sc.exe create` 的名字**必须逐字一致**
/// （SCM 按名字把进程与注册项对上；不一致时服务会以 1053 启动失败）。
pub const SERVICE_NAME: &str = "Agent2API";

/// 启动宽限期：开库 + 读配置 + 绑端口，给足 SCM 的判活窗口。
/// 只用于 StartPending 的 `wait_hint`，不代表真的会等这么久。
const START_HINT: Duration = Duration::from_secs(30);

/// 停机宽限期：在途请求收尾（SSE 长连接可能还没断）。
const STOP_HINT: Duration = Duration::from_secs(20);

/// 命令行走服务模式（`--service`，大小写不敏感，容忍前后空白）
pub fn requested() -> bool {
    std::env::args()
        .skip(1)
        .any(|arg| arg.trim().eq_ignore_ascii_case("--service"))
}

/// 停机信号：SCM 的控制事件经回调转到这里。
/// 网关主循环拿它和 Ctrl+C / SIGTERM 一起 select。
pub struct StopSignal(mpsc::Receiver<()>);

impl StopSignal {
    /// 阻塞等待 SCM 要求停止。
    ///
    /// 通道断开（回调已释放）也当停机处理：宁可退出让 SCM 按恢复策略拉起，
    /// 也不要卡住进程变成「僵而不死」。
    pub fn wait(self) {
        let _ = self.0.recv();
    }

    /// 转成能和非阻塞 select 一起用的接收端。
    /// 网关主循环里通常是 `tokio::select!`，用 `recv()` 的 future 形式更自然：
    /// 这个方法把同步通道包成 async 等待点。
    pub fn into_async(self) -> StopSignalAsync {
        StopSignalAsync(self.0)
    }
}

/// `StopSignal` 的 async 形态：`wait()` 在阻塞线程上轮询通道。
/// 之所以不用 tokio::sync::mpsc：SCM 回调线程是**同步**线程，不能在里面
/// await，而 tokio 的 `blocking_send` 又要求运行时上下文 —— 用标准库通道
/// 是两边都能安全调用的最小公约数。
pub struct StopSignalAsync(mpsc::Receiver<()>);

impl StopSignalAsync {
    /// 等到停机信号（在 tokio 的阻塞线程池上轮询，不占 worker）。
    pub async fn wait(self) {
        let receiver = self.0;
        let _ = tokio::task::spawn_blocking(move || receiver.recv()).await;
    }
}

/// 就绪回报：网关在「端口已绑、开始接受请求」那一刻调 `signal()`。
pub struct Ready(mpsc::Sender<()>);

impl Ready {
    pub fn signal(self) {
        let _ = self.0.send(());
    }
}

/// 网关启动逻辑：拿到停机信号与就绪回报，跑起来并阻塞到停机。
pub type StartFn = Box<dyn FnOnce(StopSignal, Ready) -> Result<(), String> + Send>;

/// 进程级启动槽位：SCM 的 service_main 是 `extern "system" fn`，不能捕获
/// 环境，只能走静态槽位取值（进程内只会被调用一次）。
static START_FN: Mutex<Option<StartFn>> = Mutex::new(None);

/// 连接到 SCM 并开始服务生命周期。会阻塞到服务停止。
pub fn dispatch(start: StartFn) -> Result<(), String> {
    *START_FN.lock().map_err(|_| "启动槽位被污染".to_string())? = Some(start);
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|error| format!("连接服务控制管理器失败：{error}"))
}

windows_service::define_windows_service!(ffi_service_main, service_main);

/// SCM 调用的真正入口：注册控制回调、上报状态、跑网关、收尾上报。
fn service_main(_arguments: Vec<OsString>) {
    if let Err(reason) = run_service() {
        // 服务进程没有控制台，这条只会进日志文件（调用方已开文件通道）
        eprintln!("❌ Windows 服务运行失败：{reason}");
    }
}

fn run_service() -> Result<(), String> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();

    let event_handler = move |control| -> ServiceControlHandlerResult {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
                // send 失败 = 网关已经退出（重复 STOP 或停机竞态）。照样回
                // NoError，让 SCM 认定事件已受理，不然它会重试到判活超时。
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            // 只声明接受 STOP / SHUTDOWN（见下面 controls_accepted），其余按
            // 未实现回给 SCM。本服务没有暂停 / 继续 / 参数变更的语义。
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .map_err(|error| format!("注册服务控制回调失败：{error}"))?;

    // 先报 StartPending：开库 + 绑端口可能超过 SCM 的判活窗口，
    // 不报 pending 会被当成「启动超时」直接杀进程（1053）。
    status_handle
        .set_service_status(status(ServiceState::StartPending, 1, START_HINT))
        .map_err(|error| format!("上报 StartPending 失败：{error}"))?;

    let start = START_FN
        .lock()
        .map_err(|_| "启动槽位被污染".to_string())?
        .take()
        .ok_or_else(|| "未找到网关启动逻辑（dispatch 未被调用？）".to_string())?;

    // 就绪通道 + 网关线程：SCM 要求 service_main 尽快返回控制权，
    // 而网关的 block_on 会一直占着线程直到停机。
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let worker = std::thread::Builder::new()
        .name("agent2api-gateway".to_string())
        .spawn(move || start(StopSignal(stop_rx), Ready(ready_tx)))
        .map_err(|error| format!("创建网关线程失败：{error}"))?;

    // 等就绪信号。超时或线程提前结束都算启动失败 —— 不能让 SCM 停在
    // StartPending 里一直转（它会按 wait_hint 判活并杀掉进程）。
    let ready = ready_rx.recv_timeout(START_HINT).is_ok();

    if !ready {
        // 网关可能在报就绪前就失败了：等它收尾拿到真实原因
        let reason = worker
            .join()
            .err()
            .map(|_| "网关线程异常退出".to_string())
            .unwrap_or_else(|| "网关启动未在宽限期内就绪".to_string());
        let _ = status_handle.set_service_status(status_with_exit(
            ServiceState::Stopped,
            0,
            Duration::default(),
            ServiceExitCode::ServiceSpecific(1),
        ));
        return Err(reason);
    }

    status_handle
        .set_service_status(status(ServiceState::Running, 0, Duration::default()))
        .map_err(|error| format!("上报 Running 失败：{error}"))?;

    // 等网关收尾（收到 STOP 后走优雅停机）
    let outcome = worker
        .join()
        .map_err(|_| "网关线程异常退出".to_string())?;

    let _ = status_handle.set_service_status(status(ServiceState::Stopped, 0, STOP_HINT));
    outcome
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
        // 只接受停机类事件：本服务没有暂停 / 继续的语义
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code,
        checkpoint,
        wait_hint,
        process_id: None,
    }
}
