use serde::Serialize;

#[cfg(target_os = "linux")]
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
#[cfg(target_os = "linux")]
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[cfg(target_os = "linux")]
pub const COMPATIBILITY_FLAG: &str = "--disable-gpu";
#[cfg(target_os = "linux")]
const COMPOSITING_ENV: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";
#[cfg(target_os = "linux")]
const INITIALIZED_ENV: &str = "_AGENT2API_GRAPHICS_INITIALIZED";
#[cfg(target_os = "linux")]
const ORIGINAL_ENV: &str = "_AGENT2API_WEBKIT_COMPOSITING_ORIGINAL";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphicsSettings {
    enabled: bool,
    active: bool,
    environment_override: bool,
    compatibility_mode: bool,
    restart_required: bool,
}

#[cfg(target_os = "linux")]
struct RuntimeGraphics {
    active: bool,
    environment_override: bool,
    compatibility_mode: bool,
    saved: AtomicBool,
}

#[cfg(target_os = "linux")]
static RUNTIME: OnceLock<RuntimeGraphics> = OnceLock::new();

pub fn initialize() {
    #[cfg(target_os = "linux")]
    {
        if RUNTIME.get().is_some() {
            return;
        }
        // Tauri 重启会继承参数和环境；一次性兼容参数不能覆盖重启后保存的新选择。
        let inherited = std::env::var_os(INITIALIZED_ENV).is_some();
        if inherited {
            if let Some(original) = std::env::var_os(ORIGINAL_ENV) {
                std::env::set_var(COMPOSITING_ENV, original);
            } else {
                std::env::remove_var(COMPOSITING_ENV);
            }
        }
        let original = std::env::var_os(COMPOSITING_ENV);
        let environment_override = original.as_ref().is_some_and(|value| value != "0");
        if let Some(value) = original {
            std::env::set_var(ORIGINAL_ENV, value);
        } else {
            std::env::remove_var(ORIGINAL_ENV);
        }
        std::env::set_var(INITIALIZED_ENV, "1");

        let compatibility = !inherited && std::env::args_os().any(|arg| arg == COMPATIBILITY_FLAG);
        let active = crate::settings::load().hardware_acceleration
            && !compatibility
            && !environment_override;
        // 环境只在 Tauri、WebKit 与后台线程启动前修改。
        if !active {
            std::env::set_var(COMPOSITING_ENV, "1");
        }
        let _ = RUNTIME.set(RuntimeGraphics {
            active,
            environment_override,
            compatibility_mode: compatibility,
            saved: AtomicBool::new(false),
        });
        eprintln!(
            "[Graphics] 硬件加速{}",
            if active {
                "已开启"
            } else {
                "已关闭（兼容模式）"
            }
        );
    }
}

#[tauri::command]
pub async fn get_graphics_settings() -> Result<GraphicsSettings, String> {
    #[cfg(target_os = "linux")]
    {
        crate::startup::wait().await?;
        let runtime = RUNTIME.get().ok_or("图形设置尚未初始化")?;
        let enabled = crate::settings::load().hardware_acceleration;
        Ok(GraphicsSettings {
            enabled,
            active: runtime.active,
            environment_override: runtime.environment_override,
            compatibility_mode: runtime.compatibility_mode,
            restart_required: runtime.saved.load(Ordering::Relaxed)
                || (enabled != runtime.active
                    && !runtime.compatibility_mode
                    && !runtime.environment_override),
        })
    }
    #[cfg(not(target_os = "linux"))]
    Err("当前平台不支持硬件加速设置".into())
}

#[tauri::command]
pub async fn save_graphics_settings(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        crate::startup::wait().await?;
        let runtime = RUNTIME.get().ok_or("图形设置尚未初始化")?;
        if enabled && runtime.environment_override {
            return Err(
                "启动环境已关闭硬件加速，请先移除 WEBKIT_DISABLE_COMPOSITING_MODE 后重新启动"
                    .into(),
            );
        }
        let mut settings = crate::settings::load();
        settings.hardware_acceleration = enabled;
        crate::settings::save(&settings)?;
        runtime.saved.store(true, Ordering::Relaxed);
        let restart = crate::commands::schedule_app_restart(&app)
            .map_err(|error| format!("设置已保存，但未能重启应用：{error}"))?;
        restart
            .wait()
            .await
            .map_err(|error| format!("设置已保存，但未能重启应用：{error}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, enabled);
        Err("当前平台不支持硬件加速设置".into())
    }
}

#[cfg(target_os = "linux")]
pub fn confirm_compatibility_restart(app: &tauri::AppHandle) {
    use crate::native_ui::{self, TextKey};

    let handle = app.clone();
    app.dialog()
        .message(native_ui::text(TextKey::CompatibilityRestartMessage))
        .title(native_ui::text(TextKey::CompatibilityRestart))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            native_ui::text(TextKey::Ok).to_string(),
            native_ui::text(TextKey::Cancel).to_string(),
        ))
        .show(move |accepted| {
            if !accepted {
                return;
            }
            tauri::async_runtime::spawn(async move {
                if let Err(error) = save_graphics_settings(handle.clone(), false).await {
                    handle
                        .dialog()
                        .message(error)
                        .title(native_ui::text(TextKey::CompatibilityRestartFailed))
                        .kind(MessageDialogKind::Error)
                        .buttons(MessageDialogButtons::OkCustom(
                            native_ui::text(TextKey::Ok).to_string(),
                        ))
                        .show(|_| {});
                }
            });
        });
}
