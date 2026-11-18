//! headless 服务器入口：Agent2API 网关的**无图形**运行形态。
//!
//! ── 与桌面端的关系 ──────────────────────────────────────────
//! 两者链接同一个网关 lib（`agent2api_server`），HTTP 契约、数据目录、
//! 迁移逻辑完全一致。差异只有三件：
//!   · 监听地址：本二进制按 `AGENT2API_HOST` 解析（默认 0.0.0.0，
//!     供容器端口映射）；桌面壳固定 127.0.0.1。
//!   · 界面：桌面由 Tauri 壳的 custom-protocol 出面板；本形态由网关
//!     自己托管 `ui/` 静态目录（`set_ui_dir`），并注入网页端 bridge
//!     （`web_shim`，UI 代码不感知差异）。
//!   · 安全闸门：本形态面向网络部署，两层的最低保护 ——
//!     管理面要「管理员账号或 API Key」（都没有则拒绝启动）；
//!     转发面（/v1/*）没配 Key 时 fail-closed（登录面板自建第一把即恢复；
//!     桌面默认只听回环，同一风险不存在；确要裸跑见 `AGENT2API_ALLOW_NO_KEY`）。
//!
//! ── 两种起法（同一个可执行文件）──────────────────────────────
//!   · `agent2api-server`             前台跑：日志打 stderr，Ctrl+C 停；
//!   · `agent2api-server --service`   Windows 服务（仅 Windows）：交给 SCM
//!                                    托管，日志落文件，`sc.exe stop` 停。
//! Linux 上对应的形态是 systemd 单元（前台跑即可，由 systemd 管进程），
//! 不需要这个参数 —— 发行包里带的 `agent2api.service` 就是干这个的。
//!
//! ── 环境变量 ────────────────────────────────────────────────
//!   AGENT2API_PROXY_HOME        配置/数据目录（旧名 WORKBUDDY_PROXY_HOME 兼容读）
//!   AGENT2API_HOST              监听地址，默认 0.0.0.0
//!   AGENT2API_PROXY_PORT        监听端口（旧名 WORKBUDDY_PROXY_PORT），默认 3065
//!   AGENT2API_PANEL_PORT        可选：面板分端口 —— 设置后管理面（界面 + /api/*）
//!                               单独监听该端口，主端口只保留 /v1/* 网关；
//!                               公网部署只映射主端口，即可把面板留在内网
//!   AGENT2API_UI_DIR            管理界面静态目录，默认 `ui/`（相对可执行文件）
//!   AGENT2API_LOG_FILE          日志文件路径（可选）。服务形态下不设也能跑：
//!                               按 `{PROXY_HOME}/logs/server.log` 落盘
//!   AGENT2API_ADMIN_USER        面板管理员账号（与下面的密码变量之一同时设置）
//!   AGENT2API_ADMIN_PASSWORD    面板管理员密码（明文，启动时自动转 bcrypt 哈希）
//!   AGENT2API_ADMIN_PASSWORD_HASH 面板管理员密码的 bcrypt 哈希（优先于明文；
//!                               htpasswd -nBC 10 user 的输出整行可粘）
//!   AGENT2API_ALLOW_NO_KEY      置 `1` 关闭全部闸门（未配 Key 也放行，纯内网用）
//!   AGENT2API_CAPTCHA_ENABLED   登录页人机验证组件环境变量，默认为1开启，0为关闭
//!   AGENT2API_SELF_UPDATE       置 `1` 允许面板在容器内原地更新（Dockerfile 默认设好；
//!                               换文件后进程退出、由容器 restart 策略拉起）
//!   AGENT2API_VERBOSE           置 `1` 打开 debug 级日志（与桌面一致）
//!   AGENT2API_IMAGE_CACHE_MB    生成图片的内存下载缓存上限（MiB，默认 128，
//!                               范围 8~4096；非法/越界回落默认）。写坏只回落、
//!                               不报错，与其它配置项同一口径
//!   AGENT2API_IMAGE_COMPRESS_CACHE_MB  会话图片压缩结果的内存缓存上限
//!                               （MiB，默认 128，范围 8~4096，口径同上）

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use agent2api_server::server::{config, logging, start_managed, ServerHandle, ServerState};
use futures::FutureExt;
use tokio::sync::watch;
use tokio::time::Instant;

// ── 全局内存分配器（仅 Linux）──────────────────────────────────
// 长稳、多账号并发转发的部署形态（Docker 容器 / Linux 服务端）改用 jemalloc，
// 抑制 glibc malloc 的碎片与 RSS 缓慢增长（Issue #420）。为什么只在 Linux、
// 为什么是 jemalloc、为什么不做成 feature，见 server/Cargo.toml 对应条目的
// 注释 —— 这里只放"生效"那一行。
//
// 位置要求：全局分配器在整个进程里只能有一个，所以它只出现在本 headless
// 入口，不写进 `agent2api_server` 库 —— 桌面端链接的是那个库，库一旦也定义，
// 两边就会撞成「duplicate global allocator」。放在这里，桌面端进程里没有任何
// 定义，回落到 Rust 默认分配器，行为逐字不变。
#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// 解析后的启动参数。前台与服务形态共用同一份，避免两边口径漂移。
struct Launch {
    port: u16,
    host: IpAddr,
    panel_port: Option<u16>,
    ui_dir: PathBuf,
}

/// 启动到清理一直由同一份自有运行时承接；句柄取得后不再直接关闭插件。
#[derive(Default)]
struct Running {
    bootstrap_started: bool,
    server: Option<ServerHandle>,
}

#[derive(Clone)]
struct ShutdownRequest {
    source: &'static str,
    error: Option<String>,
    deadline: Instant,
}

/// 同步回调也能发送；首次请求固定原因和截止，取消等待者不会留下阻塞线程。
#[derive(Clone)]
struct ShutdownSignal(watch::Sender<Option<ShutdownRequest>>);

impl ShutdownSignal {
    fn new() -> Self {
        Self(watch::channel(None).0)
    }

    fn request(&self, source: &'static str) -> bool {
        self.request_with_error(source, None)
    }

    fn request_with_error(&self, source: &'static str, error: Option<String>) -> bool {
        self.0.send_if_modified(move |request| {
            if request.is_some() {
                return false;
            }
            *request = Some(ShutdownRequest {
                source,
                error,
                // 尚未拿到ServerHandle时也记录首关期限，稍后只能继承，不能重新起算。
                deadline: Instant::now() + Duration::from_secs(55),
            });
            true
        })
    }

    fn current(&self) -> Option<ShutdownRequest> {
        self.0.borrow().clone()
    }

    async fn wait(&self) -> ShutdownRequest {
        let mut receiver = self.0.subscribe();
        loop {
            if let Some(request) = receiver.borrow_and_update().clone() {
                return request;
            }
            // self仍保留发送端，正常情况下不会断开；断开也不能变成永远等待。
            if receiver.changed().await.is_err() {
                self.request_with_error("停止通知中断", Some("停止通知通道意外关闭".into()));
            }
        }
    }
}

#[derive(Clone, Copy)]
enum GatewayEvent {
    Ready,
    Stopping,
}

fn main() -> ExitCode {
    if let Some(code) = agent2api_server::server::core::providers::subscriptions::dispatch_helper()
    {
        return ExitCode::from(code);
    }
    if std::env::args().nth(1).as_deref() == Some("--systemd-updater") {
        return ExitCode::from(agent2api_server::server::core::update::systemd::run_helper() as u8);
    }
    if std::env::args().nth(1).as_deref() == Some("--windows-updater") {
        let id = std::env::args().nth(2).unwrap_or_default();
        return ExitCode::from(
            agent2api_server::server::core::update::windows_service::run_helper(&id) as u8,
        );
    }
    // Windows 服务形态判定放在**参数解析之前**：服务进程没有控制台，
    // 配置写错（端口非法之类）时 eprintln 会全部丢掉，而「配置错导致起不来」
    // 恰恰是最需要看日志的情形。所以先把文件通道打开，再做任何可能报错的事。
    #[cfg(windows)]
    if service::requested() {
        // 运行形态标志：设置页「版本信息」要能区分「Windows 服务」与
        // 「前台跑的 headless」。`--service` 只在这里出现一次，置位后
        // 运行期再没有别的可观察痕迹（SCM 拉起与双击启动的进程一模一样）。
        agent2api_server::server::core::system_info::set_service_mode(true);
        logging::init_file_sink();
        return run_as_windows_service();
    }

    let launch = match parse_launch() {
        Ok(launch) => launch,
        Err(reason) => return exit_code(Err(reason)),
    };

    run_foreground(launch)
}

/// 前台形态：信号先于bootstrap监听，启动期间收到停止也继承同一请求。
fn run_foreground(launch: Launch) -> ExitCode {
    exit_code(run_gateway(launch, ShutdownSignal::new(), true, |_| Ok(())))
}

fn exit_code(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => {
            logging::console_line(
                "[Server]",
                "进程退出码=0；本轮关闭结果已确认，未验证远端环境回收",
            );
            ExitCode::SUCCESS
        }
        Err(reason) => {
            logging::console_line("[Server]", &format!("进程退出码=1：{reason}"));
            ExitCode::FAILURE
        }
    }
}

/// 运行时在任何bootstrap之前建立。先等库侧真实Result，再关闭运行时，绝不倒序。
fn run_gateway(
    launch: Launch,
    signal: ShutdownSignal,
    console_signals: bool,
    mut report: impl FnMut(GatewayEvent) -> Result<(), String>,
) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("创建 Tokio 运行时失败: {error}"))?;
    let result = runtime.block_on(async {
        let signal_job = if console_signals {
            Some(install_shutdown_signals(signal.clone())?)
        } else {
            None
        };
        let mut running = Running::default();
        // 保留取得的句柄跨越启动异常：有句柄就等它，没有才使用初始化回退清理。
        let work = std::panic::AssertUnwindSafe(async {
            start_gateway(&launch, &signal, &mut running)?;
            let server = running.server.as_ref().ok_or("服务器停止句柄未登记")?;
            if server.wait_shutdown_requested().now_or_never().is_some() {
                signal.request("服务器内部开始关闭");
            } else if signal.current().is_none() {
                report(GatewayEvent::Ready)?;
            }
            tokio::select! {
                _ = signal.wait() => {},
                _ = server.wait_shutdown_requested() => {
                    signal.request("服务器内部开始关闭");
                },
            }
            Ok::<(), String>(())
        })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err("网关启动或运行异常，开始核对清理结果".into()));

        if let Err(reason) = &work {
            signal.request_with_error("启动或运行失败", Some(reason.clone()));
        }
        let request = signal.wait().await;
        // 先通知监听和插件协调器，再上报状态；慢日志/服务状态上报不能推迟首关。
        if let Some(server) = &running.server {
            server.request_shutdown_until(request.deadline);
        }
        let notice = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            report(GatewayEvent::Stopping)
        }))
        .unwrap_or_else(|_| Err("停止状态上报异常；仍继续等待实际清理结果".into()));
        logging::console_line(
            "[Server]",
            &format!("开始停止：{}；重复请求不会延长首个关闭期限", request.source),
        );
        if let Some(reason) = &request.error {
            logging::console_line("[Server]", reason);
        }
        // 信号任务只做异步等待，可取消并确认结束，不留下阻塞recv工作。
        if let Some(job) = signal_job {
            job.abort();
            let _ = job.await;
        }
        let cleanup = std::panic::AssertUnwindSafe(finish(&running, request.deadline))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err("退出清理异常，完成状态未确认".into()));
        let original = match work {
            Err(reason) => Err(reason),
            Ok(()) => request.error.map_or(Ok(()), Err),
        };
        let result = combine_results(combine_results(original, notice), cleanup);
        match &result {
            Ok(()) => logging::console_line("[Server]", "本轮关闭结果已收到，即将结束服务器运行时"),
            Err(reason) => {
                logging::console_line("[Server]", &format!("本轮启动/关闭未完整确认：{reason}"))
            }
        }
        result
    });
    // 这里只终止已取得结果之后的进程运行时，不再加一个55秒窗口。
    // 库侧Result不承诺所有进程后台工作结束；不能让未跟踪的阻塞工作无限卡住退出，
    // 也不能把不等待它们冒充已全部析构。进程返回后仍由系统结束残留工作。
    runtime.shutdown_background();
    result
}

fn combine_results(first: Result<(), String>, second: Result<(), String>) -> Result<(), String> {
    match (first, second) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(first), Err(second)) => Err(format!("{first}；{second}")),
    }
}

/// 把网关跑起来：开库、装闸门、绑端口、起 HTTP 服务。
///
/// 前台与服务形态共用 —— 两者的差异只在「怎么等停机」，不在这里。
fn start_gateway(
    launch: &Launch,
    signal: &ShutdownSignal,
    running: &mut Running,
) -> Result<(), String> {
    if signal.current().is_some() {
        return Err("开始初始化前已收到停止请求，本次不启动网关".into());
    }
    // bootstrap中途失败也按已尝试初始化处理，不遗漏先前已启动的插件/调度。
    running.bootstrap_started = true;
    let mut state = ServerState::bootstrap(launch.port, launch.host)
        .map_err(|reason| format!("网关初始化失败: {reason}"))?;
    if signal.current().is_some() {
        return Err("初始化期间收到停止请求，不再启动HTTP监听".into());
    }

    state.panel_port = launch.panel_port;
    // headless 面板闸门：未注册时 /api/* 只放行注册相关端点与 API Key（强制
    // 先注册，见 access::panel_gate 与 http::require_api_key 的注释）。
    // 唯一的关闭口在下面的安全闸门里：AGENT2API_ALLOW_NO_KEY=1 是「我自己
    // 要全放行」的显式声明，闸门不得拦它。
    agent2api_server::server::access::set_panel_gate(true);
    if let Some(p) = launch.panel_port {
        logging::log(
            "[Server]",
            &format!(
                "面板分端口已启用：管理面（界面 + /api/*）监听 :{p}，主端口只保留 /v1/* 网关 —— 公网部署建议防火墙/compose 只映射主端口"
            ),
        );
    }

    // 面板访问控制的库句柄与刷新令牌载入（注册 / 双令牌落盘都走它）
    agent2api_server::server::access::attach_db(state.db().cloned());
    agent2api_server::server::access::load_refresh_tokens();
    agent2api_server::server::access::load_access_tokens();
    // env 预置的管理员同步进库（明文在此前已转哈希，库里只存哈希）
    agent2api_server::server::access::sync_env_admin_to_store();

    apply_security_gates();

    state.set_ui_dir(launch.ui_dir.clone());

    // 更新钩子只登记请求，不直接退出进程；清理失败仍必须保留失败退出码。
    let restart_signal = signal.clone();
    agent2api_server::server::core::update::selfupdate::set_restart_hook(Box::new(move || {
        if restart_signal.request("面板安装并重启") {
            logging::console_line(
                "[Server]",
                "已收到面板重启请求；先等待真实收尾结果，再由外部服务管理器拉起",
            );
        }
    }));

    if signal.current().is_some() {
        return Err("启动监听前收到停止请求，本次不再接受连接".into());
    }
    let server =
        start_managed(&state).map_err(|conflict| format!("网关启动失败：{}", conflict.message))?;
    // 先保存再做任何就绪通知；后续失败仍有同一关闭句柄可等待。
    running.server = Some(server);
    if let Some(request) = signal.current() {
        if let Some(server) = &running.server {
            server.request_shutdown_until(request.deadline);
        }
        return Ok(());
    }
    agent2api_server::server::core::update::systemd::notify_ready();

    logging::log(
        "[Server]",
        &format!(
            "headless 运行中：面板 http://{}:{}/ ｜ API http://{}:{}/v1",
            display_host(launch.host),
            launch.port,
            display_host(launch.host),
            launch.port
        ),
    );

    Ok(())
}

/// 安全闸门：/v1/* 的 fail-closed 与注册提示。
///
/// 桌面形态的安全边界是「只监听 127.0.0.1」，免鉴权语义（一把 Key 都
/// 没配 → 全放行）在此之上成立；本形态绑 0.0.0.0、对外可见：
///   · 转发面（/v1/*）：只认 API Key。没有任何 Key 时 fail-closed
///     （拒绝服务并引导去面板建第一把），额度绝不对全网开放；
///   · 管理面（/api/*）：管理员注册后需要登录。**未注册的窗口期**放行
///     （否则连「创建管理员」都进不去）—— 这是刻意的取舍：抢注只可能
///     发生在「部署完到用户注册」之间，所以启动日志会强烈提醒立即注册；
///     注册一完成，管理面立即要求登录。介意这个窗口的，用环境变量
///     预置管理员（AGENT2API_ADMIN_USER / …_PASSWORD_HASH，跳过注册）。
/// 纯内网确要无 Key 裸跑的，显式设 AGENT2API_ALLOW_NO_KEY=1 自己负责。
fn apply_security_gates() {
    if !config::current().active_api_keys().is_empty() {
        logging::log("[Security]", "API Key 认证已启用");
    } else if std::env::var("AGENT2API_ALLOW_NO_KEY")
        .map(|v| v.trim() == "1")
        .unwrap_or(false)
    {
        // 裸跑是「我自己要全放行」的显式声明：面板闸门必须让路，
        // 否则 /api/* 会被未注册闸门挡住，与声明自相矛盾
        agent2api_server::server::access::set_panel_gate(false);
        logging::log(
            "[Security]",
            "⚠️  AGENT2API_ALLOW_NO_KEY=1：未配置 API Key，网关对所有来源完全开放",
        );
    } else {
        agent2api_server::server::access::set_v1_fail_closed(true);
        if agent2api_server::server::access::panel_auth_enabled() {
            logging::log(
                "[Security]",
                "尚未配置 API Key：登录面板后在「网关 Key」页创建第一把，/v1/* 在此之前拒绝服务",
            );
        } else {
            logging::log(
                "[Security]",
                "⚠️  尚未注册管理员且未配置 API Key：请立即打开 http://<面板地址>/login 完成首次注册",
            );
            logging::log(
                "[Security]",
                "    /v1/* 已进入 fail-closed（拒绝服务）；管理接口在注册完成前不校验来源，注册后立即要求登录",
            );
        }
    }
}

/// 有句柄只走该协调器；无句柄的启动失败才单独清理已初始化的插件和签到。
async fn finish(running: &Running, deadline: Instant) -> Result<(), String> {
    let native = std::panic::catch_unwind(|| {
        agent2api_server::server::core::providers::claude_subscription::shutdown();
    })
    .map_err(|_| "原生会话取消发生异常，完成状态未确认".to_string());
    let managed = if let Some(server) = &running.server {
        server.request_shutdown_until(deadline);
        server.wait_shutdown().await
    } else if running.bootstrap_started {
        logging::console_line(
            "[Server]",
            "尚未取得服务器句柄；按首个截止清理启动期间的插件与签到，保留原启动错误",
        );
        let auto_checkin = std::panic::catch_unwind(|| {
            agent2api_server::server::core::auto_checkin::stop_global();
        })
        .map_err(|_| "签到调度停止异常；仍继续排空插件".to_string());
        let plugins =
            agent2api_server::server::core::plugin_host::shutdown_all_until(deadline).await;
        combine_results(auto_checkin, plugins)
    } else {
        Ok(())
    };
    combine_results(native, managed)
}

// ─── Windows 服务形态 ────────────────────────────────────────

#[cfg(windows)]
pub mod service;

/// Windows 服务形态：把网关交给 SCM 托管。
///
/// 调用方（main）已经打开日志文件通道 —— 服务进程没有控制台，
/// 不开文件通道等于整场运行都是哑的。
#[cfg(windows)]
fn run_as_windows_service() -> ExitCode {
    // 参数错误也在已连接SCM之后报告，避免启动失败没有服务退出码。
    let start_fn: service::StartFn = Box::new(move |stop, progress| {
        let launch = parse_launch()?;
        run_gateway(launch, stop.signal(), false, |event| match event {
            GatewayEvent::Ready => progress.ready(),
            GatewayEvent::Stopping => progress.stopping(),
        })
    });
    exit_code(service::dispatch(start_fn))
}

// ─── 参数解析 ──────────────────────────────────────────────

/// 解析启动参数（端口 / 地址 / 面板端口 / UI 目录）。
fn parse_launch() -> Result<Launch, String> {
    // 端口口径与桌面壳一致：新名 > 旧名 > 默认（非法值当未设置）
    let port = env_port("AGENT2API_PROXY_PORT")
        .or_else(|| env_port("WORKBUDDY_PROXY_PORT"))
        .unwrap_or(3065);

    let host: IpAddr = match std::env::var("AGENT2API_HOST") {
        Ok(value) if !value.trim().is_empty() => value
            .trim()
            .parse()
            .map_err(|_| format!("AGENT2API_HOST 不是合法的 IP 地址: {value}"))?,
        // headless 形态默认 0.0.0.0：它的宿主通常是容器 / 服务器，
        // 绑回环会让端口映射与外部访问全部失效。对外暴露的安全由
        // 「未配 Key 拒绝启动」闸门 + API Key 认证兜底。
        _ => IpAddr::from(Ipv4Addr::UNSPECIFIED),
    };

    // 面板分端口（可选）：设了就把管理面（静态界面 + /api/*）挪到独立端口，
    // 主端口只留 /v1/* 网关 —— 想收敛暴露面时，公网只映射主端口即可。
    // 与主端口相同没有意义（等于没分），直接报错不让部署带着歧义上线。
    let panel_port = match std::env::var("AGENT2API_PANEL_PORT") {
        Ok(value) if !value.trim().is_empty() => {
            let p: u16 = value
                .trim()
                .parse()
                .map_err(|_| format!("AGENT2API_PANEL_PORT 不是合法端口: {value}"))?;
            if p == port {
                return Err(format!(
                    "AGENT2API_PANEL_PORT 与网关端口相同（{p}）：分端口部署要求两者不同"
                ));
            }
            Some(p)
        }
        _ => None,
    };

    Ok(Launch {
        port,
        host,
        panel_port,
        ui_dir: resolve_ui_dir(),
    })
}

/// 读端口环境变量：未设置 / 非数字 / 0 一律当未设置（与桌面 env_port 同口径）
fn env_port(name: &str) -> Option<u16> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
}

/// UI 目录：AGENT2API_UI_DIR 优先，默认可执行文件同级的 `ui/`。
///
/// 这里只判目录**可定位**（能拼出路径），不判存在 —— 目录缺失时面板 404，
/// API 照常工作（纯 API 部署可以不带 ui/），启动不该因此失败。
fn resolve_ui_dir() -> PathBuf {
    if let Some(dir) = std::env::var("AGENT2API_UI_DIR")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .map(|parent| parent.to_path_buf())
        .unwrap_or_default()
        .join("ui")
}

/// 日志与提示里的地址：0.0.0.0 显示为 127.0.0.1（它表示「所有网卡」，
/// 用户实际访问用回环或机器 IP，直接打 0.0.0.0 在多数客户端里不通）
fn display_host(host: IpAddr) -> String {
    if host.is_unspecified() {
        "127.0.0.1".to_string()
    } else {
        host.to_string()
    }
}

/// 在bootstrap之前注册系统信号；安装失败是可清理的错误，不panic或伪装成正常停止。
fn install_shutdown_signals(signal: ShutdownSignal) -> Result<tokio::task::JoinHandle<()>, String> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal as unix_signal, SignalKind};
        let mut interrupt = unix_signal(SignalKind::interrupt())
            .map_err(|error| format!("注册 Ctrl+C 处理失败: {error}"))?;
        let mut term = unix_signal(SignalKind::terminate())
            .map_err(|error| format!("注册 SIGTERM 处理失败: {error}"))?;
        Ok(tokio::spawn(async move {
            let (source, event) = tokio::select! {
                event = interrupt.recv() => ("Ctrl+C", event),
                event = term.recv() => ("SIGTERM", event),
            };
            let error = event
                .is_none()
                .then(|| format!("{source} 信号监听意外结束"));
            signal.request_with_error(source, error);
        }))
    }
    #[cfg(windows)]
    {
        let mut interrupt = tokio::signal::windows::ctrl_c()
            .map_err(|error| format!("注册 Ctrl+C 处理失败: {error}"))?;
        Ok(tokio::spawn(async move {
            let error = interrupt
                .recv()
                .await
                .is_none()
                .then(|| "Ctrl+C 信号监听意外结束".to_string());
            signal.request_with_error("Ctrl+C", error);
        }))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(tokio::spawn(async move {
            let error = tokio::signal::ctrl_c()
                .await
                .err()
                .map(|error| format!("Ctrl+C 信号监听失败: {error}"));
            signal.request_with_error("Ctrl+C", error);
        }))
    }
}
