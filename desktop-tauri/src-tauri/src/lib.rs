/**
 * 应用入口 — 组装状态、注册插件与命令、创建主窗口。
 *
 * 前端源码在 desktop-tauri/ui/，通过 tauri.conf.json 的 frontendDist 引入。
 * 界面代码只依赖 window.workbuddyDesktop 这个接口，不感知具体壳，
 * 因此从 Electron 迁到 Tauri 时前端一行未改。职责拆成几块：
 *   server/      进程内 HTTP 服务器（网关本体；原为外部 node 后端进程）
 *                `server/config_migration.rs` 是 1.x 配置目录迁移的唯一实现，
 *                由本文件 setup 第一步调用（必须早于任何配置目录写盘）
 *   backend.rs   服务生命周期（启动/健康检查/退出停机信号）
 *   gateway.rs   管理 API 的 HTTP 客户端（含 API Key 读取与统一解包）
 *   login.rs     登录窗口与轮询
 *   commands.rs  暴露给前端的 invoke 命令（对齐原 preload 的 API 面）
 *   settings.rs  应用设置（关闭到托盘、开机自启）的持久化
 *   tray.rs      系统托盘图标、菜单与窗口唤起
 *
 * 窗口生命周期（关闭到托盘 / 托盘退出 / 单实例）集中在本文件：
 * 这些都是「应用级」行为，散落到各模块反而看不清谁拦了退出。
 *
 * 前端桥接：主窗口在创建时注入 bridge.js，在页面脚本执行前把
 * window.workbuddyDesktop 装好，因此渲染层代码无需感知 Tauri。
 */

mod backend;
mod bridge;
mod commands;
mod gateway;
mod legacy_install;
mod login;
mod login_profile;
mod settings;
mod state;
mod tray;
mod update;

// 网关本体与端口冲突分类已拆到独立 crate（`server/`，桌面与 headless 二进制
// 共用）。在这里以原名引入：crate 内所有 `crate::server::…` /
// `crate::port_conflict::…` 路径与拆分前完全一致，两侧代码零改动。
use agent2api_server::port_conflict;
use agent2api_server::server;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use server::config_migration::MigrationOutcome;
use state::AppState;

/** 主窗口默认尺寸与最小尺寸（与原 Electron 端保持一致） */
const WIN_WIDTH: f64 = 1060.0;
const WIN_HEIGHT: f64 = 800.0;
const WIN_MIN_WIDTH: f64 = 820.0;
const WIN_MIN_HEIGHT: f64 = 600.0;

/// 主窗口标签：托盘唤起、关闭拦截、单实例激活都按它查找
pub const MAIN_WINDOW_LABEL: &str = "main";

/// 应用标题：窗口标题、托盘提示、界面自绘标题栏共用同一份文案。
///
/// debug 构建（`tauri dev`）带 `[Dev]` 前缀 —— 开发版与正式版能同时运行
/// （端口与 identifier 各自独立，见 `gateway::DEV_PORT` 与 `tauri.dev.conf.json`），
/// 而两个实例的窗口与托盘图标长得一模一样，标题不带标记就分不清谁是谁。
/// 前缀放在最前面，任务栏那种宽度受限的地方也能一眼看到。
///
/// 判据取 `debug_assertions`，与下面「开发态不做安装迁移 / 自启刷新」同一个口径。
pub fn app_title() -> &'static str {
    if cfg!(debug_assertions) {
        "[Dev]Agent2API·多提供商本地网关"
    } else {
        "Agent2API · 多提供商本地网关"
    }
}

/// 开机自启时附加的命令行参数。启动时见到它就不显示窗口，
/// 只把托盘留在后台，避免开机弹窗打扰用户。
const AUTOSTART_FLAG: &str = "--autostart";

/// 本次启动是否由开机自启触发
fn launched_by_autostart() -> bool {
    std::env::args().any(|arg| arg == AUTOSTART_FLAG)
}

/// 启动期致命错误的用户提示与收尾：显示可读原因 → 结束进程。
///
/// ── 为什么不是 `return Err(...)` ─────────────────────────────
/// 当前 Tauri（2.11）在 `setup` 返回 `Err` 时是 `panic!("Failed to setup app")`，
/// 而本项目 release 是 `panic = "abort"` —— 用户只会看到一个闪退，拿不到
/// 「旧数据在哪、为什么失败、怎么恢复」这些关键信息。这里改为显式路径：
/// 错误原样（含路径与系统错误，不含文件内容）走控制台 + 原生错误对话框，
/// 用户确认后以非零码结束进程，全程不写任何配置文件。
///
/// ── 对话框为什么用非阻塞 `show` ─────────────────────────────
/// setup 钩子跑在主线程上，`blocking_show` 的文档明确要求不能在主线程调用
/// （会冻住事件循环）；插件文档给的 setup 用法就是 `show(callback)`。
/// 回调在用户点掉对话框后触发，那里再请求退出 —— 期间事件循环照常运行，
/// 但 setup 已提前返回、窗口与托盘都没建，不会有任何初始化或写盘动作。
///
/// ── 看门狗 ─────────────────────────────────────────────────
/// 对话框万一没能显示（例如主线程任务投递失败），进程会变成「没有窗口、
/// 没有托盘、却仍占着单实例锁」的僵尸：用户再点图标只会把消息发给这个
/// 隐形进程，永远启动不起来。看门狗保证失败路径一定有终点；超时足够长
/// （5 分钟），正常用户读提示的时间远小于它。
///
/// ── 退出路径不会写盘 ────────────────────────────────────────
/// 迁移失败时 `settings::load()` 从未执行，`close_to_tray` 保持默认 false，
/// 因此 `ExitRequested` 分支不会拦截退出，只会调 `backend::shutdown` ——
/// 那里只做停机信号与日志，不发网络请求、不写配置文件（后端从未启动，
/// 定时签到全局句柄也未初始化）。`app.exit` 只触发 ExitRequested/Exit，
/// 不会再走一遍 setup 之后的初始化；看门狗走 `process::exit` 更直接：
/// 此时没有窗口、托盘、后台任务或文件句柄需要收尾。
fn report_startup_failure(app: &tauri::AppHandle, error: &str) {
    /// 用户长时间不确认时强制结束的上限
    const FORCE_EXIT_AFTER: std::time::Duration = std::time::Duration::from_secs(300);

    let message = format!(
        "Agent2API 启动已中止：{error}\n\n\
         本次未改动、未删除任何旧数据，也未创建新配置目录。\
         排除原因（磁盘空间、文件占用、权限）后重新打开本程序即可自动重试迁移。"
    );
    eprintln!("[Startup] ❌ {message}");
    let handle = app.clone();
    app.dialog()
        .message(message)
        .title("Agent2API 无法启动")
        .kind(MessageDialogKind::Error)
        .show(move |_| handle.exit(1));

    std::thread::spawn(|| {
        std::thread::sleep(FORCE_EXIT_AFTER);
        eprintln!("[Startup] ⚠️  错误提示长时间未确认，强制结束本次启动");
        std::process::exit(1);
    });
}

/// 创建主窗口 —— 启动建一次；轻量模式下窗口被销毁后，托盘按需重建也走这里。
///
/// 建窗参数（标题 / 装饰与标题栏策略 / 尺寸 / 默认最大化 / 桥注入 /
/// WebView2 后台保活参数）**只在这一处声明**：重建的窗口是用户「重新打开」
/// 的界面，任何参数漂移都会表现为「托盘叫回来的窗口和原来长得不一样」，
/// 所以启动与重建必须共用同一份。
///
/// `visible` 由调用方决定：启动时按「是否开机自启」静默（比先显示再隐藏
/// 更干净，不会在任务栏闪一下），重建时恒为 true。
fn create_main_window(app: &AppHandle, visible: bool) -> tauri::Result<tauri::WebviewWindow> {
    // ── 窗口装饰：两个平台各走一条，差别只在这一处 ────────────────
    // 共同点是**都保留 32px 自绘标题条**（titlebar.js 动态创建，与界面同一套
    // 配色），平台差异只在「这三键谁来画、标题栏归谁管」：
    //
    //   · Windows / Linux —— `decorations(false)` 去掉系统标题栏，三键由界面
    //     自绘（右上角矩形键，Windows 11 惯例）。
    //   · macOS —— 保留系统装饰，但标题栏设为透明叠加（`TitleBarStyle: Overlay`
    //     + `hidden_title`）：红/黄/绿三颗圆点由 AppKit 画在左上角、盖在网页
    //     内容之上（Overlay 会一并打开 fullSizeContentView），我们那条 32px
    //     标题条就是它的背景板，界面只让出红绿灯占的那段左内边距
    //     （见 ui/css/layout.css 的 .titlebar.is-macos）。
    //
    // 为什么 macOS 不用 decorations(false) 自己画三键：自绘的圆点只能是**样子**，
    // 换不来系统那套行为 —— 长按绿灯进分屏、全屏 Space、窗口圆角与投影、
    // 失焦自动变灰、按住 Option 的绿灯缩放、以及「双击标题栏」跟随系统偏好
    // （系统偏好里那一项能设成最小化 / 缩放 / 什么都不做，自绘接不住）。
    // 代价是 macOS 的建窗参数与 Windows 不再逐字相同（这一处分支的由来）。
    //
    // 两个平台共同的脚本侧依赖：
    //   · 拖动 / 双击最大化：前端给标题栏元素声明 `data-tauri-drag-region`
    //     属性 —— Tauri 的 core 脚本监听 mousedown，目标元素带该属性
    //     才拖动（子元素不带不拖），双击（连击）切最大化。Overlay 下这条
    //     是**必须**的：系统标题栏在透明叠加态不接管拖动。
    //     已知限制：Overlay 下窗口未聚焦时拖不动标题栏（tauri#4316）——
    //     第一次按下只把窗口激活，要再拖一次。Tauri 侧的行为，绕不开。
    //   · 边缘拖拽缩放：`resizable(true)`（默认即开）。
    //   · 关闭：Windows 侧界面的 ✕ 走 window_close 命令 → CloseRequested，
    //     与系统关闭按钮同一条事件路径，托盘拦截逻辑不变；macOS 用系统红点，
    //     走的就是系统那条路（同样是 CloseRequested）。
    // ── WebView2 后台保活参数（Windows 专属）──────────────────────
    // 窗口隐藏或最小化时，Chromium 默认会对后台窗口启用定时器节流
    // （Timer Throttling）与挂起，导致后台验证码铸造循环（ui/zcode-captcha-pool.js）
    // 的 setTimeout 被严重降频甚至冻结。解除这三项节流限制以维持后台令牌正常补充
    // （轻量模式下窗口整个销毁，该循环随之停止 —— 取舍见 AppSettings::lightweight_mode）。
    //
    // 开头的 `--disable-features=…` **不能省**：wry 对 additional_browser_args 是
    // **替换**语义（`unwrap_or_else`，见 wry 的 webview2/mod.rs），设了它就看不到
    // wry 的默认值 —— 而默认值里正有关掉 WebView2「⋯」溢出菜单与 SmartScreen 检查
    // 的那三项（Tauri 在 additional_browser_args 的文档警告里说明了这一点）。
    // 后面三项才是本 PR 新增的保活开关，与前缀互不冲突，可以并在一串里。
    #[cfg(target_os = "windows")]
    let builder = WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::App("index.html".into()))
        .decorations(false)
        .additional_browser_args(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
             --disable-background-timer-throttling \
             --disable-backgrounding-occluded-windows \
             --disable-renderer-backgrounding",
        );

    // macOS：`decorations(true)` 不能省 —— tao 建窗时若 decorations 为 false
    // 会直接走 Borderless 分支并**忽略** title_bar_style（见 tao 的
    // platform_impl/macos/window.rs），Overlay 就白设了，红绿灯也不会出现。
    // `hidden_title` 只隐藏系统画的标题文字（不影响窗口标题本身，它仍供系统
    // 的窗口菜单 / 任务切换使用），不隐藏三键。
    #[cfg(target_os = "macos")]
    let builder = WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::App("index.html".into()))
        .decorations(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        // 圆点位置：x 是首颗圆点到窗口左边缘的距离（系统标准就是 20），
        // y 是 wry 的 inset 语义 —— 它按 `圆点高 + y` 收窄标题栏容器、让三颗
        // 圆点在其中垂直居中，所以 20 落在我们那条 32px 标题条里正好上下各
        // 留 10px。不给这个值就走系统默认（按系统标题栏高度摆，Big Sur+ 是
        // 28pt），在 32px 的条里会偏高约 2px。
        // 注意这里定的是**逻辑点**，不随界面缩放（设置里的 80%–130%）变化 ——
        // 缩放会改变那条 32px 标题条的逻辑高度，此时圆点不再严格居中（最多
        // 偏 5pt 上下），可接受；系统没给运行时改这个位置的接口。
        .traffic_light_position(tauri::LogicalPosition::new(20.0, 20.0));

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let builder = WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::App("index.html".into()))
        .decorations(false);

    builder
        .title(app_title())
        .inner_size(WIN_WIDTH, WIN_HEIGHT)
        .min_inner_size(WIN_MIN_WIDTH, WIN_MIN_HEIGHT)
        .center()
        .maximized(true)
        .visible(visible)
        .initialization_script(bridge::bridge_js())
        .build()
}

/// 确保主窗口存在并返回它：没有（轻量模式销毁后 / 尚未创建）就按建窗参数重建。
///
/// 托盘唤起与单实例激活共用这条路径 —— 两者都只负责「把界面叫回来」，
/// 都不该假定窗口一定还在。重建失败（WebView 起不来等）返回 None，
/// 调用方按「窗口不可用」安静处理，绝不 panic。
pub fn ensure_main_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        return Some(window);
    }
    create_main_window(app, true).ok()
}

pub fn run() {
    let mut builder = tauri::Builder::default();

    // 单实例必须第一个注册：插件按注册顺序执行，晚于其它插件时，
    // 第二个实例可能已经建好窗口/托盘才被判定为重复启动
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 第二个实例已被插件结束进程，这里只负责把已有窗口叫到前台
            tray::show_main_window(app);
        }));
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            // 自启动登记带 --autostart，启动时据此判断要不要静默（不显示窗口）。
            // MacosLauncher 是占位参数，本目标只关心 Windows。
            tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![AUTOSTART_FLAG])),
        )
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::api_request,
            commands::api_request_text,
            commands::backend_status,
            commands::port_occupant,
            commands::end_port_occupant,
            commands::restart_app,
            commands::check_port,
            commands::change_port,
            commands::panel_admin_status,
            commands::panel_register,
            commands::change_lan_access,
            // 界面直连网关时的凭证头（AI 助手页不经桥发请求，见 commands.rs）
            commands::direct_credentials,
            commands::local_ip,
            commands::start_login,
            commands::start_autoclaw_oauth_login,
            commands::login_kuku_sms_send,
            commands::login_kuku_sms_verify,
            commands::login_state,
            commands::cancel_login,
            commands::export_logs,
            commands::pick_folder,
            commands::get_app_settings,
            commands::save_app_settings,
            commands::export_accounts,
            commands::import_accounts,
            commands::export_database,
            commands::import_database,
            commands::check_update,
            commands::download_update,
            commands::update_progress,
            commands::cancel_update,
            commands::run_installer,
            commands::set_window_theme,
            commands::set_zoom,
            commands::open_release_page,
            // 自定义标题栏的窗口三键（窗口已 decorations(false)，见建窗处）
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::window_is_maximized,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // ── 第一步：一次性配置目录迁移（`~/.workbuddy-proxy` → `~/.agent2api`）──
            // 必须是**全进程最早**的一处配置目录访问：晚于任何写盘（桌面设置、
            // config.json、日志库、账号库）时，一旦迁移失败，后续写盘就会把新目录
            // 建出来，`target.exists()` 从此为真，迁移再也无法重试，用户看到的是
            // 「账号/配置全没了」。放在这里，失败时下面所有会写盘的初始化都不执行。
            //
            // 这时日志库还没装，所以迁移日志只能走控制台（`eprintln!`，见 logging）。
            match server::config_migration::migrate_config_dir() {
                Ok(MigrationOutcome::Migrated { from, to }) => {
                    eprintln!(
                        "[Config] 📦 配置目录已迁移: {} → {}（旧目录保留，可回退）",
                        from.display(),
                        to.display()
                    );
                }
                Ok(MigrationOutcome::NothingToDo) => {}
                // 用户用环境变量显式指定了配置目录：迁移不适用，也不该提示打扰
                Ok(MigrationOutcome::SkippedEnvOverride) => {}
                Err(error) => {
                    // 迁移失败不能继续：以空配置启动会让新目录被后续写盘建出来，
                    // 迁移从此不会重试，且用户看到的是「数据消失」。
                    // 这里显示可读错误（原生对话框）后结束本次启动 ——
                    // 旧目录原样保留，新目录未被创建，下次启动自动重试。
                    report_startup_failure(&handle, &error);
                    return Ok(());
                }
            }

            // 设置先读一次并缓存：窗口事件回调是同步的，需要立即拿到
            // close_to_tray 才能决定关窗是隐藏还是退出；lightweight 同理 ——
            // 它决定关窗是「销毁窗口」还是「隐藏」。#188：早先只缓存了前者，
            // 轻量模式要等设置页读过/存过一次（get_app_settings / save_app_settings）
            // 才写进缓存，于是「开机自启 → 直接关窗」这条路径上缓存恒为默认 false，
            // 关窗走了隐藏、轻量模式形同没开。
            let app_settings = settings::load();
            {
                let state = app.state::<AppState>();
                state.window.set_close_to_tray(app_settings.close_to_tray);
                state.window.set_lightweight(app_settings.lightweight_mode);
            }

            // ── 旧「当前用户」级安装的清理 + 自启登记刷新（1.x → perMachine 迁移）──
            // 安装器改成 perMachine 后以管理员身份运行，此时 `%LOCALAPPDATA%` 与
            // HKCU 指向管理员账户，读不到也删不掉发起安装的用户的旧安装 ——
            // 只有这里（普通用户上下文的启动期）能覆盖到它。
            //
            // 旧版还把自启登记写成 `%LOCALAPPDATA%\<旧产品名>`、值名用旧产品名；
            // 新版装在 Program Files、值名换成 productName，两条互不覆盖，旧的那条
            // 指向已被删除的路径，用户不开设置页就永远不会被修正，自启静默失效。
            //
            // ── 为什么两件事放在同一个后台任务里顺序执行 ──────────────
            // 两件事都要读/写 HKCU 的 Run 键（清理判「旧登记是否已失效」、刷新写
            // 新登记），顺序执行让刷新看到的是清理之后的状态，也避免两个线程交错
            // 操作同一处注册表。
            //
            // 放后台：旧目录可能含 88MB 的 node.exe，`remove_dir_all` 叠加杀软扫描
            // 会明显拖慢启动，而这两件事与后续初始化没有任何依赖关系。
            // 失败只记日志、下次启动重试，绝不阻断启动（见 legacy_install 模块头）。
            let autostart_wanted = app_settings.autostart;
            let cleanup_handle = handle.clone();
            tauri::async_runtime::spawn_blocking(move || {
                // ── 开发态（debug 构建）不做安装迁移 ────────────────────────
                // 这两个动作都会改真实系统状态：清理会删掉本机已安装的正式版
                // （安装目录 + 快捷方式 + 卸载项），自启刷新会把开机自启登记指向
                // `target\debug` 下的调试副本。而 `tauri dev` 是日常动作 ——
                // 跑一次就把用户装的正式版删了，代价远大于收益（实际发生过：
                // 开发态启动后，开始菜单/桌面快捷方式全部失效）。
                // release 打包（唯一的发布形态）行为完全不变；要验证清理逻辑
                // 本身，用 `tauri build` 出的安装包跑。
                if cfg!(debug_assertions) {
                    return;
                }
                legacy_install::cleanup_legacy_user_install();
                if !autostart_wanted {
                    return;
                }
                let name = cleanup_handle.package_info().name.clone();
                // 幂等：登记已指向当前 exe 时不做任何写入；用户在任务管理器
                // 「启动」页里停用过的也不重建（判定见 legacy_install）
                if !legacy_install::autostart_needs_refresh(&name) {
                    return;
                }
                match cleanup_handle.autolaunch().enable() {
                    Ok(()) => {
                        eprintln!("[Cleanup] 已按当前安装路径重新登记开机自启（升级迁移）: {name}")
                    }
                    Err(error) => {
                        eprintln!("[Cleanup] 重新登记开机自启失败（下次启动再试）: {error}")
                    }
                }
            });

            // 托盘先建好再建窗口：开机自启不显示窗口，托盘是唯一入口
            if let Err(error) = tray::create(&handle) {
                eprintln!("[tray] 创建托盘图标失败: {error}");
            }

            // 主窗口先建好并加载界面，后端在后台异步拉起，
            // 界面不会因为等待 node 启动而白屏。
            // 开机自启时不显示窗口（visible(false) 比先显示再隐藏更干净，
            // 不会在任务栏闪一下）。
            //
            // ── 开机自启 + 轻量模式：窗口整个不建 ──────────────────────
            // 只 hide 不 destroy 的话，WebView2 那 6 个进程照样常驻（约
            // 200MB）—— 轻量模式的意义要等用户「打开窗口再关一次」才兑现，
            // 而自启路径永远走不到那一步（它不经过关窗）。这里直接跳过建窗，
            // 只留托盘与后端；用户点托盘（或双击图标触发单实例激活）时由
            // `ensure_main_window` 按同一份参数重建 —— 与轻量模式下「销毁
            // 再唤起」走的是同一条路径，因此窗口长得完全一样。
            //
            // 代价与轻量模式本身一致：跑在 WebView 里的 ZCode 验证码令牌池
            // 在用户打开窗口前不补货，该通道请求会以 503 落进请求日志（见
            // `AppSettings::lightweight_mode` 的说明）。关掉轻量模式或非自启
            // 启动都照旧建窗，不走这条。
            //
            // 建窗参数（无装饰标题栏、WebView2 保活、桥注入等）只在
            // create_main_window 一处声明：轻量模式下托盘重建也走它，
            // 两边必须长得一模一样。
            let autostarted = launched_by_autostart();
            if !(autostarted && app_settings.lightweight_mode) {
                create_main_window(&handle, !autostarted)?;
            }

            // 后端负责维护任务的持久化排期，桌面壳不额外触发刷新。
            tauri::async_runtime::spawn(async move {
                if let Err(failure) = backend::ensure_ready(&handle).await {
                    eprintln!("[backend] 启动失败: {}", failure.message);
                    // 存一份到 AppState：事件是一次性的，界面可能在事件发出之后
                    // 才订阅（窗口还在加载、WebView 被刷新），那时只能靠主动查
                    // （backend_status）拿到失败原因，否则状态灯永远说不清为什么灰着
                    if let Some(state) = handle.try_state::<AppState>() {
                        if let Ok(mut guard) = state.backend.lock() {
                            guard.failure = Some(failure.clone());
                        }
                    }
                    let _ = handle.emit("backend:error", failure);
                    return;
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Tauri 应用初始化失败")
        .run(|app, event| match event {
            // 关窗时按设置决定「隐藏到托盘」还是「真退出」。
            // 拦截关闭后窗口只是隐藏，后端进程继续存活，网关照常转发。
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                if label != MAIN_WINDOW_LABEL {
                    return;
                }
                let state = app.state::<AppState>();
                // 用户主动退出（托盘菜单）时就别拦了，否则退出路径被自己挡死
                if state.window.close_to_tray() && !state.window.is_exiting() {
                    api.prevent_close();
                    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                        if state.window.lightweight() {
                            // 轻量模式：直接销毁窗口 —— WebView2 那 6 个进程
                            // 随之退出（释放约 200MB 常驻内存），网关在进程内
                            // 不受影响。销毁后 `ExitRequested` 由下面的分支拦下
                            // （close_to_tray 开着 → prevent_exit，进程留在托盘），
                            // 托盘点击时由 `ensure_main_window` 按同一份参数重建。
                            let _ = window.destroy();
                        } else {
                            let _ = window.hide();
                        }
                    }
                }
            }
            // 最后一个窗口关闭后 Tauri 会请求退出，这里再兜一次：
            // 只拦「用户关窗」这一种，托盘退出与主动 exit 都要放行。
            // 放行时顺手回收后端 —— 例如用户关掉了「关闭到托盘」后直接关窗真退出，
            // 这条路径不会走到下面的 Exit 分支，不在这里回收就会漏下 node 进程
            // （shutdown 幂等，与其它调用点重复也不会有副作用）。
            tauri::RunEvent::ExitRequested { api, .. } => {
                let state = app.state::<AppState>();
                if state.window.close_to_tray() && !state.window.is_exiting() {
                    api.prevent_exit();
                } else {
                    backend::shutdown(&state);
                }
            }
            // 最后兜底回收后端进程（复用外部服务的不会被动）。
            // 注意：程序化退出（托盘退出、安装前退出走的都是 `AppHandle::exit(0)`）
            // 不保证触发本事件，这是 Tauri 的已知行为，所以那几条主动退出路径
            // 已各自显式调用了 shutdown，这里只覆盖其余自然退出。
            tauri::RunEvent::Exit => {
                let state = app.state::<AppState>();
                backend::shutdown(&state);
            }
            _ => {}
        });
}
