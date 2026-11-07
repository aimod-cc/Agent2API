//! 系统托盘：图标、右键菜单、左键唤起主窗口。
//!
//! 托盘是本应用唯一的「后台入口」：关窗后进程仍在跑（见 lib.rs 的关闭到托盘逻辑），
//! 用户只能靠托盘图标把界面叫回来或彻底退出，所以菜单项与左键点击都复用了
//! 同一套 `show_main_window` / 退出实现，避免出现「图标恢复不了窗口」这类死角。
//! 轻量模式下窗口会被整个销毁（见 lib.rs 的 `ensure_main_window`），唤起时按
//! 同一份建窗参数重建 —— 本文件只负责「叫回来」，不关心窗口此刻是隐藏、
//! 最小化还是不存在。
//!
//! 图标直接复用 `app.default_window_icon()`（即 tauri.conf.json 里配置的 bundle 图标），
//! 不额外引入托盘专用图片资源。
//!
//! ── macOS 的「图标被 Command 拖出删除」兜底 ────────────────────
//! macOS 允许用户按住 Command 把状态栏图标拖出删除。被删除后 `NSStatusItem`
//! 对象仍被 tray-icon 持有、没有任何事件通知，但它的 button 已脱离状态栏窗口
//! （`button.window()` 为 nil），于是 `TrayIcon::rect()` 返回 `None` ——
//! 这就是「图标是否还在」的可靠判据。`spawn_tray_guard` 周期性地在主线程
//! 检查这个判据，一旦发现图标不在了就移除旧托盘并用同一份参数重建，
//! 几秒内图标自动回来，用户无需重启 App（该守护仅 macOS 需要：
//! Windows / Linux 的托盘不存在被拖出删除的机制）。

use std::time::Duration;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

use crate::state::AppState;

pub const TRAY_ID: &str = "main-tray";
const MENU_SHOW: &str = "tray-show";
const MENU_CHECK_UPDATE: &str = "tray-check-update";
const MENU_QUIT: &str = "tray-quit";

/// 创建托盘图标。应用启动时调用一次即可。
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, MENU_SHOW, "显示主窗口", true, None::<&str>)?;
    let check_update_item =
        MenuItem::with_id(app, MENU_CHECK_UPDATE, "检查更新", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&show_item, &check_update_item, &separator, &quit_item],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        // 提示文字与窗口标题同一份（开发版带 "Dev" 标记，见 lib.rs 的 app_title）：
        // 两个实例同时跑时，托盘图标是唯一能一眼分清谁是谁的地方
        .tooltip(crate::app_title())
        .menu(&menu)
        // 左键单击留给「唤起窗口」，因此不让它弹菜单；右键照常弹
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => show_main_window(app),
            MENU_CHECK_UPDATE => request_update_check(app),
            MENU_QUIT => request_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 只认左键「抬起」：Windows 下按下与抬起各发一次事件，
            // 不过滤会让单击时恢复两次窗口
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    // 图标直接使用内置 32x32.png（与 tauri.conf.json 的 bundle.icon 第一项同源）。
    // 不用 `default_window_icon()`：实测该值在部分构建环境下取到的内嵌图标
    // 在 macOS 状态栏渲染异常（按钮有图像但屏幕上不可见），而内置 32x32.png
    // 是经过验证的蓝底白箭头图标，尺寸/格式稳定，状态栏必然可见。
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png")).ok();
    if let Some(icon) = icon {
        builder = builder.icon(icon);
    } else {
        eprintln!("[tray] 警告：未能加载任何托盘图标，将以空白占位显示");
    }
    // 即使拿不到默认图标也照样把托盘建起来：宁可是个无图标的占位，
    // 也不能让关窗后的用户失去唯一入口
    builder.build(app)?;

    // macOS 上托盘图标可能被用户 Command 拖出删除（且无事件通知），
    // 建好图标后挂一个自动重建守护，被删后几秒内自动恢复。
    #[cfg(target_os = "macos")]
    spawn_tray_guard(app);

    Ok(())
}

/// 判断 macOS 上托盘图标当前是否「仍显示在菜单栏」。
///
/// 判据：正常显示时 `NSStatusItem` 的 button 挂在状态栏窗口上，
/// `TrayIcon::rect()` 返回 `Some`；被用户 Command 拖出删除后 button 脱离
/// 状态栏窗口，`rect()` 返回 `None`。这是 tray-icon 在 macOS 上唯一能
/// 拿到的「图标是否还在」信号（TrayIcon 本身没有任何「被移除」事件，
/// 对象也一直存活，只是不再显示）。
#[cfg(target_os = "macos")]
fn tray_is_missing(app: &AppHandle) -> bool {
    app.tray_by_id(TRAY_ID)
        .map(|tray| tray.rect().ok().flatten().is_none())
        .unwrap_or(false)
}

/// macOS 托盘图标自动重建守护。
///
/// 每 3 秒检查一次托盘是否被用户拖出删除；发现缺失时先移除旧托盘
/// （`remove_tray_by_id` 会触发 `NSStatusBar.removeStatusItem`，彻底清掉
/// 系统侧已失效的 item），再按启动时同一份参数重建。重建后图标出现在
/// 菜单栏，`rect()` 恢复 `Some`，守护进入待命。
///
/// 检查循环在后台线程跑，但 **重建必须投递回主线程**：
/// `remove_tray_by_id` 与 `TrayIconBuilder::build` 底层都直接触碰 AppKit
/// （NSStatusBar / NSStatusItem），非主线程调用是未定义行为。
/// `TrayIcon::rect()` 本身已内部投递主线程并阻塞等待，可以放心在后台线程调用。
#[cfg(target_os = "macos")]
fn spawn_tray_guard(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(3));
        if !tray_is_missing(&handle) {
            continue;
        }
        eprintln!("[tray] 检测到菜单栏图标被移除，正在自动重建…");
        let thread_handle = handle.clone();
        let closure_handle = handle.clone();
        let _ = thread_handle.run_on_main_thread(move || {
            // remove_tray_by_id 返回被移除的 TrayIcon，丢弃即触发 Drop 清理
            let _ = closure_handle.remove_tray_by_id(TRAY_ID);
            if let Err(error) = create(&closure_handle) {
                eprintln!("[tray] 自动重建托盘图标失败（下轮重试）: {error}");
            }
        });
    });
}

/// 显示并聚焦主窗口。窗口可能处于「隐藏」「最小化」两种状态，
/// 也可能在轻量模式下已被销毁 —— 三种都要能恢复：
/// 前两种按 unminimize → show → set_focus 走一遍；
/// 销毁的由 `ensure_main_window` 按建窗参数重建后再展示。
pub fn show_main_window(app: &AppHandle) {
    let Some(window) = crate::ensure_main_window(app) else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

fn request_update_check(app: &AppHandle) {
    app.state::<AppState>().window.request_update_check();
    show_main_window(app);
    let _ = app.emit_to(crate::MAIN_WINDOW_LABEL, "tray:check-update", ());
}

#[tauri::command]
pub fn take_update_check_request(app: AppHandle, window: tauri::WebviewWindow) -> bool {
    window.label() == crate::MAIN_WINDOW_LABEL
        && app.state::<AppState>().window.take_update_check_request()
}

/// 真正退出应用。
///
/// 先置位「正在退出」标志再 `exit(0)`：开启关闭到托盘时，`exit(0)` 同样会走
/// `RunEvent::ExitRequested`，没有这个标志就会被当成「用户关窗」再拦一次，
/// 表现为「托盘点了退出但进程还在」。
///
/// 这里还要显式回收后端：`exit(0)` 这种程序化退出不保证触发 `RunEvent::Exit`
/// （Tauri 已知行为），只靠 lib.rs 的兜底分支会漏掉，node 会带着 3065 端口
/// 残留在后台。shutdown 是幂等的，多调一次无害。
pub fn request_exit(app: &AppHandle) {
    app.state::<AppState>().begin_exit();
    crate::backend::shutdown(&app.state::<AppState>());
    app.exit(0);
}
