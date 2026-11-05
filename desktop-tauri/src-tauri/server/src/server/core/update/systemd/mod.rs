//! systemd 升级的网关侧：本机受限通道、任务快照与后台下载。

pub use super::service_task::ServiceUpdateTask as UpdateTask;
use serde_json::{json, Value};

use super::{UpdateError, UpdateManager};

#[cfg(target_os = "linux")]
mod transport;
#[cfg(target_os = "linux")]
mod worker;
#[cfg(target_os = "linux")]
use super::service_trust as trust;
#[cfg(target_os = "linux")]
mod files;

pub const SOCKET_PATH: &str = "/run/agent2api-updater/control.sock";
pub const STATUS_PATH: &str = "/var/lib/agent2api-updater/status.json";
pub const READY_PATH: &str = "/var/lib/agent2api-updater/ready.json";
const TASK_KEY: &str = "systemdUpdate";

/// 不依赖新环境开关判断“是否 systemd”；旧安装也能得到正确的手动接入提示。
pub fn unavailable_reason() -> Option<&'static str> {
    if super::version::server_bundle_name().is_none() {
        return Some("unsupported_arch");
    }
    if std::env::var("AGENT2API_SYSTEMD_UPDATE").is_ok_and(|value| value.trim() == "0") {
        return Some("disabled");
    }
    let expected = std::path::Path::new("/opt/agent2api");
    if super::selfupdate::current_exe_path().ok().as_deref() != Some(expected.join("agent2api-server").as_path()) {
        return Some("custom_installation");
    }
    if let Ok(value) = std::env::var("AGENT2API_UI_DIR") {
        if !value.trim().is_empty() && std::fs::canonicalize(value.trim()).ok().as_deref() != Some(expected.join("ui").as_path()) {
            return Some("custom_installation");
        }
    }
    if !std::path::Path::new(SOCKET_PATH).exists() {
        return Some("helper_missing");
    }
    #[cfg(target_os = "linux")]
    {
        return transport::probe().err().map(|_| "helper_unavailable");
    }
    #[cfg(not(target_os = "linux"))]
    Some("unsupported_arch")
}

fn local_task() -> Option<UpdateTask> {
    crate::server::core::task_state::read(TASK_KEY).ok()
        .and_then(|state| state.value)
        .and_then(|value| serde_json::from_value(value).ok())
}

fn helper_task() -> Option<UpdateTask> {
    let file = std::fs::File::open(STATUS_PATH).ok()?;
    serde_json::from_reader(std::io::Read::take(file, 16 * 1024)).ok()
}

pub fn status() -> Option<UpdateTask> {
    let local = local_task();
    let helper = helper_task();
    let mut task = match (local, helper) {
        (Some(local), Some(helper)) if local.id == helper.id || helper.started_at > local.started_at => Some(helper),
        (Some(local), _) => Some(local),
        (_, helper) => helper,
    }?;
    let started = crate::server::core::system_info::started_at_ms();
    let now = crate::server::logging::now_ms();
    if task.active() && ((task.phase == "downloading" && task.started_at < started)
        || (task.phase != "downloading" && now - task.updated_at > 10 * 60_000))
    {
        task.set_phase("failed", Some("升级任务中断，请检查服务日志后重新发起更新".to_string()));
    }
    Some(task)
}

fn save(task: &UpdateTask) -> Result<(), UpdateError> {
    crate::server::core::task_state::store_value(TASK_KEY, json!(task))
        .map_err(UpdateError::new)
}

pub fn run_helper() -> i32 {
    #[cfg(target_os = "linux")]
    {
        return worker::run();
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("systemd 升级程序仅支持 Linux");
        1
    }
}

/// 本地就绪通知不含密钥；升级程序核对目标版本、PID 与 systemd 主进程后才报成功。
pub fn notify_ready() {
    if crate::server::core::system_info::deployment() != "systemd" {
        return;
    }
    let value = json!({
        "version": super::CURRENT_VERSION,
        "pid": std::process::id(),
        "startedAt": crate::server::core::system_info::started_at_ms(),
    });
    if let Ok(file) = std::fs::OpenOptions::new().write(true).truncate(true).open(READY_PATH) {
        let _ = serde_json::to_writer(file, &value);
    }
}

impl UpdateManager {
    /// 用户只提交目标版本；下载地址、文件名与校验来源均取已核对的发布信息。
    pub fn start_systemd_update(&self, wanted: &str) -> Result<Value, UpdateError> {
        if crate::server::core::system_info::deployment() != "systemd" {
            return Err(UpdateError::with_status(400, "当前运行形态不是 systemd 服务"));
        }
        if let Some(reason) = unavailable_reason() {
            return Err(UpdateError::with_status(400, format!("systemd 一键更新不可用：{reason}")));
        }
        let mut guard = self.lock();
        if guard.service_update_active || status().is_some_and(|task| task.active()) {
            return Err(UpdateError::with_status(409, "已有服务端更新正在进行，请等待完成"));
        }
        if guard.task.as_ref().is_some_and(|task| task.active) {
            return Err(UpdateError::with_status(409, "已有下载正在进行，请等待完成"));
        }
        let release = guard.latest.as_ref().ok_or_else(|| UpdateError::with_status(400, "请先检查更新"))?;
        let version = release["tag"].as_str().unwrap_or("");
        if version != wanted || super::compare_versions(version, super::CURRENT_VERSION) != Some(1) {
            return Err(UpdateError::with_status(409, "更新版本已变化，请重新检查后确认"));
        }
        let url = release["serverBundle"]["url"].as_str().filter(|url| !url.is_empty())
            .ok_or_else(|| UpdateError::with_status(400, "本次发布没有对应架构的服务端更新包"))?.to_string();
        let now = crate::server::logging::now_ms();
        let task = UpdateTask {
            id: format!("{now}-{}", std::process::id()),
            version: wanted.to_string(),
            phase: "downloading".to_string(),
            started_at: now,
            updated_at: now,
            error: None,
        };
        save(&task)?;
        guard.service_update_active = true;
        drop(guard);
        let manager = self.clone();
        let accepted = json!(task);
        crate::spawn_task(async move {
            manager.run_systemd_update(task, url).await;
        });
        Ok(accepted)
    }

    async fn run_systemd_update(&self, mut task: UpdateTask, url: String) {
        let result = self.prepare_systemd_update(&mut task, &url).await;
        if let Err(error) = result {
            task.set_phase("failed", Some(error.message.clone()));
            let _ = save(&task);
            crate::server::logging::log("[Update]", &format!("❌ systemd 更新失败：{}", error.message));
        }
        self.lock().service_update_active = false;
    }

    async fn prepare_systemd_update(&self, task: &mut UpdateTask, url: &str) -> Result<(), UpdateError> {
        let bundle = super::server_bundle_name().ok_or_else(|| UpdateError::new("不支持当前架构"))?;
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
        let archive = self.download_dir().join(&bundle);
        #[cfg(target_os = "linux")]
        {
            let request = task.clone();
            tokio::task::spawn_blocking(move || transport::submit(&request, &archive))
                .await.map_err(|error| UpdateError::new(format!("升级通道异常：{error}")))?
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = archive;
            Err(UpdateError::new("systemd 更新仅支持 Linux"))
        }
    }
}
