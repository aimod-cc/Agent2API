//! 复用原生错误对话框；最多一个待显示提示，不创建新前端组件或重复窗口。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use super::Coordinator;
use crate::{
    backend,
    native_ui::{self, TextKey},
    port_conflict::StartupFailure,
    state::AppState,
};

/// 内部固定错误继续用于 Ticket/日志，这里只翻译原生对话框正文。
fn localized_error(message: &str) -> &str {
    let key = if message == super::CLEANUP_FAILED {
        TextKey::ExitCleanupFailed
    } else if message == super::ACTION_FAILED {
        TextKey::ExitActionFailed
    } else if message == super::INTERRUPTED {
        TextKey::ExitInterrupted
    } else if message == super::CONFLICT {
        TextKey::ExitConflict
    } else {
        return message;
    };
    native_ui::text(key)
}

pub(super) fn error(app: &AppHandle, shared: &Arc<Coordinator>, message: &'static str) {
    {
        let mut state = shared.lock();
        if state.dialog_pending {
            return;
        }
        state.dialog_pending = true;
    }
    eprintln!("[Exit] {message}");
    let app_dialog = app.clone();
    let owner = shared.clone();
    if app
        .run_on_main_thread(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // 最终退出已获准时不再用先前冲突消息抢回焦点。
            let already_allowed = owner.lock().allowed.is_some();
            if already_allowed {
                owner.lock().dialog_pending = false;
                return;
            }
            if let Some(window) = app_dialog.get_webview_window(crate::MAIN_WINDOW_LABEL) {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
            // 托盘启动没有窗口时也可直接显示原生提示，不为错误再建一份WebView。
            let done = owner.clone();
                app_dialog
                    .dialog()
                    .message(localized_error(message))
                    .title(native_ui::text(TextKey::ExitFailureTitle))
                    .kind(MessageDialogKind::Error)
                    .buttons(MessageDialogButtons::OkCustom(
                        native_ui::text(TextKey::Ok).to_string(),
                    ))
                    .show(move |_| {
                    done.lock().dialog_pending = false;
                });
        }));
        if result.is_err() {
            owner.lock().dialog_pending = false;
            eprintln!("[Exit] 原生错误提示异常，程序与原始任务保持不变");
        }
        })
        .is_err()
    {
        shared.lock().dialog_pending = false;
        eprintln!("[Exit] 原生错误提示无法投递，已保留程序和原始任务");
    }
}

pub(crate) fn startup_result(app: &AppHandle, result: Result<(), StartupFailure>) {
    let app = app.clone();
    let dispatcher = app.clone();
    // 窗口/托盘退出回调同在主线程；投递之后重新查看门，不能携带旧ready快照。
    let _ = dispatcher.run_on_main_thread(move || {
        let state = app.state::<AppState>();
        let exit_gate = state.exit.lock();
        // 现有backend事件仅通知前端；持纯内存退出门完成投递，不允许Rust监听反向申请退出。
        if exit_gate.early_failure
            || exit_gate.backend_closing
            || exit_gate.terminal.is_some()
            || exit_gate.poisoned
        {
            return;
        }
        match result {
            Ok(())
                if backend::running_port(&state).is_some()
                    && crate::startup::phase() == "ready" =>
            {
                let _ = app.emit("backend:ready", ());
            }
            Err(failure) => {
                // 独立后端任务已保存StartupFailure；外层只通知，不覆盖关闭后产生的新信息。
                eprintln!("[backend] 启动失败: {}", failure.message);
                let _ = app.emit("backend:error", failure);
            }
            _ => {}
        }
    });
}
