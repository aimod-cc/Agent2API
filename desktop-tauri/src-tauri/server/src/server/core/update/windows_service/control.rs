use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows_service::service::{Service, ServiceAccess, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::security::{self, error, Handle};
use crate::server::core::update::UpdateError;

pub(super) const NAME: &str = "Agent2API";
const KEY: &str = "SYSTEM\\CurrentControlSet\\Services\\Agent2API\\Parameters";

#[derive(Clone)]
pub(super) struct Installation {
    pub root: PathBuf,
    pub ui: PathBuf,
    pub updates: PathBuf,
}

pub(super) fn installation() -> Result<Installation, UpdateError> {
    let root = PathBuf::from(reg_string("UpdateInstallDir")?);
    let updates = PathBuf::from(reg_string("UpdateWorkDir")?);
    let ui = root.join("ui");
    if !security::same_path(&updates, &root.join(".service-update")) {
        return Err(error("升级工作目录与已登记安装不匹配"));
    }
    security::protected(&root)?;
    security::protected(&updates)?;
    security::protected(&root.join("agent2api-server.exe"))?;
    security::protected(&ui)?;
    let data = crate::paths::config_dir();
    if security::within(&data, &ui) || security::within(&data, &updates) {
        return Err(error("账号数据目录不能位于待替换的界面或升级工作目录内"));
    }
    Ok(Installation { root, ui, updates })
}

pub(super) fn registered_root() -> Option<PathBuf> {
    reg_string("UpdateInstallDir").ok().map(PathBuf::from)
}

pub(super) fn repository() -> Option<String> {
    reg_string("UpdateRepository").ok().filter(|value| !value.trim().is_empty())
}

fn reg_string(name: &str) -> Result<String, UpdateError> {
    let key = security::wide(std::ffi::OsStr::new(KEY));
    let name = security::wide(std::ffi::OsStr::new(name));
    let mut buffer = vec![0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut bytes)).ok()
    }.map_err(error)?;
    let units = bytes as usize / 2;
    let end = buffer[..units].iter().position(|value| *value == 0).unwrap_or(units);
    String::from_utf16(&buffer[..end]).map_err(error)
}

pub(super) fn open(control: bool) -> Result<Service, UpdateError> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).map_err(error)?;
    let mut access = ServiceAccess::QUERY_CONFIG | ServiceAccess::QUERY_STATUS;
    if control { access |= ServiceAccess::START | ServiceAccess::STOP; }
    manager.open_service(NAME, access).map_err(error)
}

pub(super) fn check_service(service: &Service, install: &Installation, expected_pid: Option<u32>) -> Result<(), UpdateError> {
    let config = service.query_config().map_err(error)?;
    let account = config.account_name.unwrap_or_default().to_string_lossy().to_lowercase();
    if account != "localsystem" && account != "local system" && account != "nt authority\\system" {
        return Err(error("当前服务使用自定义账号，一键更新不会修改其权限"));
    }
    let command = config.executable_path.to_string_lossy();
    let command = command.trim();
    let (path, arguments) = if let Some(rest) = command.strip_prefix('"') {
        rest.split_once('"').ok_or_else(|| error("服务启动路径格式错误"))?
    } else {
        command.split_once(char::is_whitespace).ok_or_else(|| error("服务启动参数缺少 --service"))?
    };
    if arguments.trim() != "--service"
        || !security::same_path(Path::new(path), &install.root.join("agent2api-server.exe"))
    {
        return Err(error("服务启动路径或参数与登记不匹配，请使用服务安装脚本修复"));
    }
    if let Some(pid) = expected_pid {
        let status = service.query_status().map_err(error)?;
        if status.process_id != Some(pid) || status.current_state != ServiceState::Running {
            return Err(error("运行进程与已登记 Windows 服务不匹配"));
        }
    }
    Ok(())
}

pub(super) fn process(pid: u32) -> Result<Handle, UpdateError> {
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid) }
        .map(Handle).map_err(error)
}

pub(super) fn process_is(pid: u32, path: &Path) -> bool {
    let Ok(process) = process(pid) else { return false; };
    process_matches(&process, path)
}

fn process_matches(process: &Handle, path: &Path) -> bool {
    if unsafe { WaitForSingleObject(process.0, 0) } != WAIT_TIMEOUT { return false; }
    let mut buffer = vec![0u16; 32768];
    let mut size = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut size) }.is_err() {
        return false;
    }
    let path_found = String::from_utf16_lossy(&buffer[..size as usize]);
    security::same_path(Path::new(&path_found), path)
}

pub(super) fn process_started_at(process: &Handle) -> Result<u64, UpdateError> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) }.map_err(error)?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

pub(super) fn worker_running(pid: u32, created: u64, path: &Path) -> bool {
    let Ok(process) = process(pid) else { return false; };
    process_started_at(&process).ok() == Some(created) && process_matches(&process, path)
}

pub(super) fn stop(service: &Service, pid: u32) -> Result<(), UpdateError> {
    let process = process(pid)?;
    service.stop().map_err(error)?;
    wait_state(service, ServiceState::Stopped, Duration::from_secs(60))?;
    match unsafe { WaitForSingleObject(process.0, 30_000) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(error("主服务尚未退出，未替换程序")),
        _ => Err(error("无法确认主服务已退出")),
    }
}

pub(super) fn start(service: &Service) -> Result<(), UpdateError> {
    if service.query_status().map_err(error)?.current_state == ServiceState::StopPending {
        wait_state(service, ServiceState::Stopped, Duration::from_secs(60))?;
    }
    if service.query_status().map_err(error)?.current_state == ServiceState::Stopped {
        service.start::<&str>(&[]).map_err(error)?;
    }
    wait_state(service, ServiceState::Running, Duration::from_secs(90))
}

fn wait_state(service: &Service, state: ServiceState, timeout: Duration) -> Result<(), UpdateError> {
    let deadline = Instant::now() + timeout;
    loop {
        if service.query_status().map_err(error)?.current_state == state { return Ok(()); }
        if Instant::now() >= deadline { return Err(error("等待 Windows 服务状态切换超时")); }
        std::thread::sleep(Duration::from_millis(200));
    }
}
