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
#[cfg(target_os = "linux")]
const MENU_COMPATIBILITY: &str = "tray-compatibility";

/// 创建托盘图标。应用启动时调用一次即可。
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, MENU_SHOW, "显示主窗口", true, None::<&str>)?;
    let check_update_item =
        MenuItem::with_id(app, MENU_CHECK_UPDATE, "检查更新", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    #[cfg(target_os = "linux")]
    let compatibility_item = MenuItem::with_id(
        app, MENU_COMPATIBILITY, "以兼容模式重启", true, None::<&str>,
    )?;
    #[cfg(target_os = "linux")]
    let menu = Menu::with_items(
        app,
        &[&show_item, &check_update_item, &compatibility_item, &separator, &quit_item],
    )?;
    #[cfg(not(target_os = "linux"))]
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
            #[cfg(target_os = "linux")]
            MENU_COMPATIBILITY => crate::graphics::confirm_compatibility_restart(app),
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

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    // 即使拿不到默认图标也照样把托盘建起来：宁可是个无图标的占位，
    // 也不能让关窗后的用户失去唯一入口
    builder.build(app)?;
    Ok(())
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

/// 托盘退出只提交意图；后台清理成功后才由统一协调器请求真正退出。
pub fn request_exit(app: &AppHandle) {
    let _ = crate::exit::request_quit(app, 0);
}
