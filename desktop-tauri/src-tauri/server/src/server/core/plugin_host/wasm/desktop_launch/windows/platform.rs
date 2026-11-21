mod credentials;
mod version;
pub use credentials::{read_credential, write_credential, Credential};
pub use version::product;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, LPARAM, WPARAM};
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

use super::super::declaration::HotSwitch;
use crate::server::core::plugin_host::wasm::desktop_launch::paths;

#[derive(Clone)]
pub struct ServiceProcess {
    pub pid: u32,
    pub path: PathBuf,
    pub started: u64,
}

#[derive(Clone)]
pub struct HotSwitchTargets {
    pub roots: Vec<ServiceProcess>,
    pub services: Vec<ServiceProcess>,
}

pub enum HotSwitchRespawn {
    Ready,
    ClientClosed,
    Canceled,
    TimedOut,
}

fn system() -> sysinfo::System {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing()
            .with_exe(sysinfo::UpdateKind::OnlyIfNotSet)
            .with_cmd(sysinfo::UpdateKind::OnlyIfNotSet),
    );
    system
}

fn is_non_main_cmd(cmd: &[OsString]) -> bool {
    cmd.iter().skip(1).any(|raw| {
        let arg = raw.to_string_lossy().to_ascii_lowercase();
        arg == "--type"
            || arg.starts_with("--type=")
            || arg == "--import"
            || arg.starts_with("--import=")
            || arg == "--require"
            || arg.starts_with("--require=")
            || arg == "-r"
            || arg == "--loader"
            || arg.starts_with("--loader=")
            || arg == "--experimental-loader"
            || arg.starts_with("--experimental-loader=")
            || arg == "--ms-enable-electron-run-as-node"
            || arg == "--node-ipc"
            || arg == "node-ipc"
            || arg == "--usenodeipc"
            || arg == "--standalone"
            || arg == "--subclient_type"
            || arg.starts_with("--subclient_type=")
            || arg == "--override_ide_name"
            || arg.starts_with("--override_ide_name=")
            || arg == "--clientprocessid"
            || arg.starts_with("--clientprocessid=")
            || arg == "--max-old-space-size"
            || arg.starts_with("--max-old-space-size=")
            || arg == "--inspect"
            || arg.starts_with("--inspect=")
            || arg == "--inspect-brk"
            || arg.starts_with("--inspect-brk=")
            || arg == "--eval"
            || arg == "-e"
            || arg == "--print"
            || arg == "-p"
    })
}

fn run_as_node_pids(system: &mut sysinfo::System, candidates: &[sysinfo::Pid]) -> HashSet<u32> {
    if candidates.is_empty() { return HashSet::new(); }
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(candidates),
        false,
        sysinfo::ProcessRefreshKind::nothing().with_environ(sysinfo::UpdateKind::OnlyIfNotSet),
    );
    candidates.iter().filter(|pid| {
        system.process(**pid).is_some_and(|process| {
            process.environ().iter().any(|entry| {
                let text = entry.to_string_lossy();
                let Some((key, value)) = text.split_once('=') else { return false; };
                key.eq_ignore_ascii_case("ELECTRON_RUN_AS_NODE")
                    && matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true")
            })
        })
    }).map(|pid| pid.as_u32()).collect()
}

pub fn running_executables(names: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut system = system();
    let candidates: Vec<_> = system.processes().iter().filter(|(_, process)| {
        names.iter().any(|name| process.name().to_string_lossy().eq_ignore_ascii_case(name))
            && !is_non_main_cmd(process.cmd())
    }).map(|(pid, _)| *pid).collect();
    let node_pids = run_as_node_pids(&mut system, &candidates);
    let mut unique: Vec<PathBuf> = Vec::new();
    for pid in candidates {
        if node_pids.contains(&pid.as_u32()) { continue; }
        let Some(path) = system.process(pid).and_then(|process| process.exe()) else { continue; };
        if !unique.iter().any(|existing| paths::same(existing, path)) {
            unique.push(path.to_path_buf());
        }
    }
    unique.sort();
    Ok(unique)
}

fn has_explicit_profile(cmd: &[OsString]) -> bool {
    cmd.iter().skip(1).any(|raw| {
        let arg = raw.to_string_lossy();
        arg == "--user-data-dir" || arg.starts_with("--user-data-dir=")
    })
}

fn main_root_pids(system: &mut sysinfo::System, executable: &Path) -> HashSet<u32> {
    let candidates: Vec<_> = system.processes().iter().filter(|(_, process)| {
        process.exe().is_some_and(|path| paths::same(path, executable)) && !is_non_main_cmd(process.cmd())
    }).map(|(pid, _)| *pid).collect();
    let node_pids = run_as_node_pids(system, &candidates);
    let raw_roots: HashSet<u32> = candidates.into_iter().map(|pid| pid.as_u32())
        .filter(|pid| !node_pids.contains(pid)).collect();
    raw_roots.iter().copied().filter(|pid| {
        let Some(process) = system.process(sysinfo::Pid::from_u32(*pid)) else { return false; };
        if has_explicit_profile(process.cmd()) { return true; }
        let mut cursor = process;
        let mut seen = HashSet::from([*pid]);
        while let Some(parent_pid) = cursor.parent().map(|id| id.as_u32()) {
            if !seen.insert(parent_pid) { break; }
            let Some(parent) = system.process(sysinfo::Pid::from_u32(parent_pid)) else { break; };
            if parent.start_time() > cursor.start_time() { break; }
            if raw_roots.contains(&parent_pid) { return false; }
            cursor = parent;
        }
        true
    }).collect()
}

fn expand_descendants(system: &sysinfo::System, roots: &HashSet<u32>, root_dir: &Path) -> HashSet<u32> {
    let mut selected = roots.clone();
    for _ in 0..32 {
        let mut changed = false;
        for (pid, process) in system.processes() {
            let pid_u32 = pid.as_u32();
            if selected.contains(&pid_u32) { continue; }
            let Some(parent_pid) = process.parent().map(|id| id.as_u32()) else { continue; };
            if !selected.contains(&parent_pid) { continue; }
            if system.process(sysinfo::Pid::from_u32(parent_pid)).is_some_and(|parent| parent.start_time() > process.start_time()) {
                continue;
            }
            if process.exe().is_some_and(|path| paths::checked(path, false).is_ok_and(|path| path.starts_with(root_dir))) {
                changed |= selected.insert(pid_u32);
            }
        }
        if !changed { break; }
    }
    selected
}

fn verified_processes(system: &sysinfo::System, pids: &HashSet<u32>, root_dir: &Path) -> Result<Vec<(u32, PathBuf, u64)>, String> {
    let mut result = Vec::new();
    for (pid, process) in system.processes() {
        let pid_u32 = pid.as_u32();
        if !pids.contains(&pid_u32) { continue; }
        let path = process.exe().ok_or("无法核对客户端进程路径，未关闭程序")?;
        let path = paths::checked(path, false)?;
        if !path.starts_with(root_dir) { return Err("客户端子进程不属于原安装目录".into()); }
        let (actual, started) = process_identity(pid_u32)?;
        if !paths::same(&actual, &path) { return Err("客户端进程路径已变化，未结束程序".into()); }
        result.push((pid_u32, path, started));
    }
    Ok(result)
}

fn targets(executable: &Path) -> Result<Vec<(u32, PathBuf, u64)>, String> {
    let executable = paths::checked(executable, false)?;
    let mut system = system();
    let root = executable.parent().ok_or("客户端安装目录不可用")?;
    let current = std::process::id();
    let roots = main_root_pids(&mut system, &executable);
    let selected = expand_descendants(&system, &roots, root);
    if selected.contains(&current) { return Err("不能关闭主程序自己的进程".into()); }
    verified_processes(&system, &selected, root)
}

fn service_process_match(path: &Path, service_executables: &[String]) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
        service_executables.iter().any(|allowed| name.eq_ignore_ascii_case(allowed))
    })
}

fn collect_hot_targets(system: &mut sysinfo::System, executable: &Path, hot_switch: &HotSwitch) -> Result<Option<HotSwitchTargets>, String> {
    let executable = paths::checked(executable, false)?;
    let root_dir = executable.parent().ok_or("客户端安装目录不可用")?;
    let current = std::process::id();
    let root_pids = main_root_pids(system, &executable);
    if root_pids.is_empty() { return Ok(None); }
    if root_pids.contains(&current) { return Err("不能关闭主程序自己的进程".into()); }
    let all_pids = expand_descendants(system, &root_pids, root_dir);
    let mut roots = Vec::new();
    let mut services = Vec::new();
    for (pid_u32, path, started) in verified_processes(system, &all_pids, root_dir)? {
        if pid_u32 == current { return Err("不能关闭主程序自己的进程".into()); }
        if root_pids.contains(&pid_u32) {
            roots.push(ServiceProcess { pid: pid_u32, path, started });
        } else if service_process_match(&path, &hot_switch.service_executables) {
            services.push(ServiceProcess { pid: pid_u32, path, started });
        }
    }
    if roots.is_empty() || services.is_empty() { return Ok(None); }
    Ok(Some(HotSwitchTargets { roots, services }))
}

pub fn hot_switch_targets(executable: &Path, hot_switch: Option<&HotSwitch>) -> Result<Option<HotSwitchTargets>, String> {
    let Some(hot_switch) = hot_switch else { return Ok(None); };
    let mut system = system();
    collect_hot_targets(&mut system, executable, hot_switch)
}

pub fn stop_hot_services(executable: &Path, targets: &HotSwitchTargets) -> Result<(), String> {
    let executable = paths::checked(executable, false)?;
    let root_dir = executable.parent().ok_or("客户端安装目录不可用")?;
    for root in &targets.roots {
        let (actual, started) = process_identity(root.pid).map_err(|_| "客户端进程已变化，未重载后台服务".to_string())?;
        if started != root.started || !paths::same(&actual, &root.path) {
            return Err("客户端进程已变化，未重载后台服务".into());
        }
    }
    for service in &targets.services {
        if !service.path.starts_with(root_dir) { return Err("客户端子进程不属于原安装目录".into()); }
        if let Ok((actual, started)) = process_identity(service.pid) {
            if started == service.started && paths::same(&actual, &service.path) {
                terminate(service.pid, &service.path, service.started)?;
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let any_alive = targets.services.iter().any(|service| {
            process_identity(service.pid).is_ok_and(|(actual, started)| started == service.started && paths::same(&actual, &service.path))
        });
        if !any_alive {
            let roots_alive = targets.roots.iter().all(|root| {
                process_identity(root.pid).is_ok_and(|(actual, started)| started == root.started && paths::same(&actual, &root.path))
            });
            return if roots_alive { Ok(()) } else { Err("客户端进程已退出，请重新启动客户端".into()) };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("客户端后台服务未能结束，未完成在线切换".into())
}

pub fn wait_hot_services_respawn(executable: &Path, hot_switch: &HotSwitch, previous: &HotSwitchTargets, cancel: &CancellationToken) -> Result<HotSwitchRespawn, String> {
    let executable = paths::checked(executable, false)?;
    let root_dir = executable.parent().ok_or("客户端安装目录不可用")?;
    let expected_bins: HashSet<String> = previous.services.iter()
        .filter_map(|service| service.path.file_name().and_then(|name| name.to_str()).map(|name| name.to_ascii_lowercase()))
        .collect();
    let deadline = Instant::now() + Duration::from_millis(hot_switch.respawn_timeout_ms);
    while Instant::now() < deadline {
        if cancel.is_cancelled() { return Ok(HotSwitchRespawn::Canceled); }
        let system = system();
        let surviving_roots: HashSet<u32> = previous.roots.iter().filter(|root| {
            process_identity(root.pid).is_ok_and(|(actual, started)| started == root.started && paths::same(&actual, &root.path))
        }).map(|root| root.pid).collect();
        if surviving_roots.is_empty() { return Ok(HotSwitchRespawn::ClientClosed); }
        let descendants = expand_descendants(&system, &surviving_roots, root_dir);
        let current_services: Vec<ServiceProcess> = verified_processes(&system, &descendants, root_dir)
            .unwrap_or_default().into_iter()
            .filter(|(pid, path, _)| !surviving_roots.contains(pid) && service_process_match(path, &hot_switch.service_executables))
            .map(|(pid, path, started)| ServiceProcess { pid, path, started }).collect();
        let old_still_alive = previous.services.iter().any(|old| {
            current_services.iter().any(|current| current.pid == old.pid && current.started == old.started)
        });
        if !old_still_alive {
            let replacements: Vec<_> = current_services.iter().filter(|current| {
                !previous.services.iter().any(|old| old.pid == current.pid && old.started == current.started)
            }).collect();
            let all_respawned = expected_bins.iter().all(|bin| {
                replacements.iter().any(|service| service.path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.eq_ignore_ascii_case(bin)))
            });
            if all_respawned { return Ok(HotSwitchRespawn::Ready); }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(HotSwitchRespawn::TimedOut)
}

pub fn running(executable: &Path) -> Result<bool, String> { Ok(!targets(executable)?.is_empty()) }

unsafe extern "system" fn close_window(window: windows::Win32::Foundation::HWND, value: LPARAM) -> windows::core::BOOL {
    let selected = &*(value.0 as *const HashSet<u32>);
    let mut pid = 0;
    GetWindowThreadProcessId(window, Some(&mut pid));
    if selected.contains(&pid) { let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)); }
    windows::core::BOOL(1)
}

struct Handle(HANDLE);
impl Drop for Handle { fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } } }

fn created(handle: HANDLE) -> Result<u64, String> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }
        .map_err(|_| "无法重新核对客户端进程，未结束程序")?;
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

fn process_identity(pid: u32) -> Result<(PathBuf, u64), String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(Handle).map_err(|_| "无法核对客户端进程路径，未关闭程序")?;
    Ok((process_path(handle.0)?, created(handle.0)?))
}

fn process_path(handle: HANDLE) -> Result<PathBuf, String> {
    let mut name = vec![0u16; 32768];
    let mut length = name.len() as u32;
    unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(name.as_mut_ptr()), &mut length) }
        .map_err(|_| "无法重新核对客户端进程，未结束程序")?;
    paths::checked(&PathBuf::from(String::from_utf16(&name[..length as usize]).map_err(|_| "客户端进程路径无效")?), false)
}

fn terminate(pid: u32, expected: &Path, started: u64) -> Result<(), String> {
    let system = system();
    let Some(process) = system.process(sysinfo::Pid::from_u32(pid)) else { return Ok(()); };
    if !process.exe().is_some_and(|path| paths::same(path, expected)) {
        return Err("客户端进程已变化，未结束新进程".into());
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE, false, pid) }
        .map(Handle).map_err(|_| "无法结束客户端，请先手动关闭程序")?;
    if created(handle.0)? != started { return Err("客户端进程已变化，未结束新进程".into()); }
    if !paths::same(&process_path(handle.0)?, expected) {
        return Err("客户端进程路径已变化，未结束程序".into());
    }
    unsafe { TerminateProcess(handle.0, 0) }.map_err(|_| "客户端未能关闭，请手动关闭后重试".into())
}

fn remaining(executable: &Path, original: &[(u32, PathBuf, u64)]) -> Result<Vec<(u32, PathBuf, u64)>, String> {
    let mut current = targets(executable)?;
    let system = system();
    for (id, path, started) in original {
        if current.iter().any(|(current_id, _, _)| current_id == id) { continue; }
        if let Some(process) = system.process(sysinfo::Pid::from_u32(*id)) {
            let (actual, creation) = process_identity(*id)?;
            if creation != *started { continue; }
            let observed = process.exe().ok_or("无法核对客户端进程路径，未关闭程序")?;
            if !paths::same(&actual, path) || !paths::same(observed, path) { return Err("客户端进程路径已变化，未结束程序".into()); }
            current.push((*id, path.clone(), *started));
        }
    }
    Ok(current)
}

pub fn close(executable: &Path) -> Result<(), String> {
    let original = targets(executable)?;
    if original.is_empty() { return Ok(()); }
    let selected: HashSet<_> = original.iter().map(|(id, _, _)| *id).collect();
    unsafe { EnumWindows(Some(close_window), LPARAM(&selected as *const _ as isize)) }
        .map_err(|_| "无法请求客户端关闭")?;
    let grace = Instant::now() + Duration::from_secs(8);
    while Instant::now() < grace {
        if remaining(executable, &original)?.is_empty() { return Ok(()); }
        std::thread::sleep(Duration::from_millis(200));
    }
    let known: HashMap<_, _> = original.iter().map(|(id, path, started)| (*id, (path, *started))).collect();
    for (id, path, started) in remaining(executable, &original)?.into_iter().rev() {
        if known.get(&id).is_none_or(|(before, at)| !paths::same(before, &path) || *at != started) {
            return Err("客户端在关闭期间产生了新进程，请手动关闭后重试".into());
        }
        terminate(id, &path, started)?;
    }
    drop(known);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if remaining(executable, &original)?.is_empty() { return Ok(()); }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("客户端进程尚未全部退出，未写入凭证".into())
}

pub fn start(executable: &Path) -> Result<(), String> {
    paths::checked(executable, false)?;
    if running(executable)? { return Err("客户端已经运行，未再次启动".into()); }
    let mut child = std::process::Command::new(executable)
        .current_dir(executable.parent().ok_or("客户端安装目录不可用")?)
        .creation_flags(0x08000000)
        .spawn().map_err(|_| "凭证已切换，但客户端未能启动")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if running(executable)? { return Ok(()); }
        if child.try_wait().map_err(|_| "客户端启动结果未确认")?.is_some() { break; }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("凭证已切换，但客户端启动结果未确认".into())
}
