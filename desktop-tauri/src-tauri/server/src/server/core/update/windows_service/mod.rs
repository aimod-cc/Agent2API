//! Windows 服务更新：由受保护目录中的临时进程接手，不占用正在替换的主程序。

#[cfg(windows)]
mod control;
#[cfg(windows)]
mod files;
#[cfg(windows)]
mod security;
#[cfg(windows)]
mod worker;

use serde_json::{json, Value};

use super::{service_task::ServiceUpdateTask, UpdateError, UpdateManager};

const TASK_KEY: &str = "windowsServiceUpdate";

pub fn unavailable_reason() -> Option<&'static str> {
    #[cfg(windows)]
    {
        if std::env::consts::ARCH != "x86_64" { return Some("unsupported_arch"); }
        if std::env::var("AGENT2API_WINDOWS_UPDATE").is_ok_and(|value| value.trim() == "0") {
            return Some("disabled");
        }
        if control::registered_root().is_none() { return Some("windows_setup_required"); }
        let Ok(install) = control::installation() else { return Some("windows_update_permissions"); };
        let Ok(service) = control::open(true) else { return Some("windows_update_permissions"); };
        if !security::is_system().unwrap_or(false) || security::in_job().unwrap_or(true) {
            return Some("windows_update_permissions");
        }
        if control::check_service(&service, &install, Some(std::process::id())).is_err()
            || !security::same_path(&super::selfupdate::current_exe_path().unwrap_or_default(), &install.root.join("agent2api-server.exe"))
        {
            return Some("custom_installation");
        }
        if let Some(ui) = std::env::var("AGENT2API_UI_DIR").ok().filter(|value| !value.trim().is_empty()) {
            if !security::same_path(std::path::Path::new(ui.trim()), &install.ui) {
                return Some("custom_installation");
            }
        }
        None
    }
    #[cfg(not(windows))]
    Some("unsupported_arch")
}

fn local_task() -> Option<ServiceUpdateTask> {
    crate::server::core::task_state::read(TASK_KEY).ok().and_then(|state| state.value)
        .and_then(|value| serde_json::from_value(value).ok())
}

fn save(task: &ServiceUpdateTask) -> Result<(), UpdateError> {
    crate::server::core::task_state::store_value(TASK_KEY, json!(task)).map_err(UpdateError::new)
}

pub fn status() -> Option<ServiceUpdateTask> {
    let local = local_task();
    #[cfg(windows)]
    let worker: Option<ServiceUpdateTask> = control::installation().ok()
        .and_then(|install| files::read_json(&install.updates.join("status.json")).ok())
        .and_then(|value| serde_json::from_value(value).ok());
    #[cfg(not(windows))]
    let worker: Option<ServiceUpdateTask> = None;
    let mut task = match (local, worker) {
        (Some(local), Some(worker)) if local.id == worker.id && worker.updated_at >= local.updated_at
            || worker.started_at > local.started_at => Some(worker),
        (Some(local), _) => Some(local),
        (_, worker) => worker,
    }?;
    let now = crate::server::logging::now_ms();
    if task.active() {
        let started = crate::server::core::system_info::started_at_ms();
        let stale = now.saturating_sub(task.updated_at);
        let interrupted = if task.phase == "downloading" {
            task.started_at < started
        } else {
            #[cfg(windows)]
            {
                match worker::active(&task.id) {
                    Some(true) => false,
                    Some(false) => task.updated_at < started || stale > 5_000,
                    None => task.updated_at < started || stale > 10 * 60_000,
                }
            }
            #[cfg(not(windows))]
            { stale > 10 * 60_000 }
        };
        if interrupted {
            task.set_phase("failed", Some("更新任务中断，请检查服务日志后重新更新".to_string()));
        }
    }
    Some(task)
}

pub fn notify_ready() {
    #[cfg(windows)]
    if let Ok(install) = control::installation() {
        let _ = files::write_json(&install.updates.join("ready.json"), &json!({
            "version": super::CURRENT_VERSION,
            "pid": std::process::id(),
            "startedAt": crate::server::core::system_info::started_at_ms(),
        }));
    }
}

pub fn run_helper(id: &str) -> i32 {
    #[cfg(windows)]
    { worker::run(id) }
    #[cfg(not(windows))]
    { let _ = id; 1 }
}

impl UpdateManager {
    pub fn start_windows_service_update(&self, wanted: &str) -> Result<Value, UpdateError> {
        if crate::server::core::system_info::deployment() != "windows-service" {
            return Err(UpdateError::with_status(400, "当前不是 Windows 服务部署"));
        }
        if let Some(reason) = unavailable_reason() {
            return Err(UpdateError::with_status(400, format!("Windows 服务一键更新不可用：{reason}")));
        }
        let mut guard = self.lock();
        if guard.service_update_active || status().is_some_and(|task| task.active())
            || guard.task.as_ref().is_some_and(|task| task.active)
        {
            return Err(UpdateError::with_status(409, "已有下载或更新任务正在执行"));
        }
        let release = guard.latest.as_ref().ok_or_else(|| UpdateError::with_status(400, "请先检查更新"))?;
        if release["tag"].as_str() != Some(wanted) || super::compare_versions(wanted, super::CURRENT_VERSION) != Some(1) {
            return Err(UpdateError::with_status(409, "更新版本已变化，请重新检查后确认"));
        }
        let url = release["serverBundle"]["url"].as_str().filter(|value| !value.is_empty())
            .ok_or_else(|| UpdateError::with_status(400, "本次发布没有 Windows 服务端更新包"))?.to_string();
        let now = crate::server::logging::now_ms();
        let mut nonce = [0u8; 8];
        getrandom::getrandom(&mut nonce).map_err(|error| UpdateError::new(error.to_string()))?;
        let suffix = nonce.iter().map(|value| format!("{value:02x}")).collect::<String>();
        let task = ServiceUpdateTask { id: format!("{now}-{suffix}"), version: wanted.to_string(),
            phase: "downloading".to_string(), started_at: now, updated_at: now, error: None };
        save(&task)?;
        guard.service_update_active = true;
        drop(guard);
        let response = json!(task);
        let manager = self.clone();
        crate::spawn_task(async move { manager.run_windows_update(task, url).await; });
        Ok(response)
    }

    async fn run_windows_update(&self, mut task: ServiceUpdateTask, url: String) {
        let result = self.prepare_windows_update(&mut task, &url).await;
        if let Err(error) = result {
            task.set_phase("failed", Some(error.message.clone()));
            let _ = save(&task);
            crate::server::logging::log("[Update]", &error.message);
        }
        self.lock().service_update_active = false;
    }

    async fn prepare_windows_update(&self, task: &mut ServiceUpdateTask, url: &str) -> Result<(), UpdateError> {
        let bundle = super::server_bundle_name().ok_or_else(|| UpdateError::new("当前架构没有服务端更新包"))?;
        self.start_download_inner(url, &bundle, true)?;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let progress = self.get_progress();
            if progress["active"] == true { continue; }
            if progress["done"] == true { break; }
            return Err(UpdateError::new(progress["error"].as_str().unwrap_or("更新下载已取消")));
        }
        task.set_phase("verifying", None);
        save(task)?;
        #[cfg(windows)]
        {
            let archive = self.download_dir().join(&bundle);
            let task = task.clone();
            let token = super::token::effective_token();
            tokio::task::spawn_blocking(move || worker::launch(&task, &archive, token))
                .await.map_err(|error| UpdateError::new(error.to_string()))?
        }
        #[cfg(not(windows))]
        Err(UpdateError::new("Windows 服务更新只支持 Windows"))
    }
}
