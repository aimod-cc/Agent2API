mod credentials;
mod version;
pub use credentials::{read_credential, write_credential, Credential};
pub use version::product;

use std::collections::{HashMap, HashSet};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, LPARAM, WPARAM};
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

use crate::server::core::plugin_host::wasm::desktop_launch::paths;

fn system() -> sysinfo::System {
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    system
}

pub fn running_executables(names: &[String]) -> Result<Vec<PathBuf>, String> {
    let system = system();
    let mut paths: Vec<_> = system.processes().values().filter(|process| names.iter().any(|name|
        process.name().to_string_lossy().eq_ignore_ascii_case(name)))
        .filter_map(|process| process.exe().map(Path::to_path_buf)).collect();
    paths.sort(); paths.dedup();
    Ok(paths)
}

fn targets(executable: &Path) -> Result<Vec<(u32, PathBuf, u64)>, String> {
    let executable = paths::checked(executable, false)?;
    let system = system();
    let mut selected = HashSet::new();
    let root = executable.parent().ok_or("客户端安装目录不可用")?;
    let current = std::process::id();
    for (pid, process) in system.processes() {
        if process.exe().is_some_and(|path| paths::same(path, &executable)) { selected.insert(pid.as_u32()); }
    }
    for _ in 0..32 {
        let mut changed = false;
        for (pid, process) in system.processes() {
            if process.parent().is_some_and(|parent| selected.contains(&parent.as_u32()))
                && process.exe().is_some_and(|path| paths::checked(path, false).is_ok_and(|path| path.starts_with(root))) {
                changed |= selected.insert(pid.as_u32());
            }
        }
        if !changed { break; }
    }
    if selected.contains(&current) { return Err("不能关闭主程序自己的进程".into()); }
    let mut result = Vec::new();
    for (pid, process) in system.processes() {
        if selected.contains(&pid.as_u32()) {
            let path = process.exe().ok_or("无法核对客户端进程路径，未关闭程序")?;
            let path = paths::checked(path, false)?;
            if !path.starts_with(root) { return Err("客户端子进程不属于原安装目录".into()); }
            let (actual, started) = process_identity(pid.as_u32())?;
            if !paths::same(&actual, &path) { return Err("客户端进程路径已变化，未结束程序".into()); }
            result.push((pid.as_u32(), path, started));
        }
    }
    Ok(result)
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
