use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::{control, security};
use crate::server::core::update::{service_task::ServiceUpdateTask, UpdateError, MAX_INSTALLER_BYTES};

pub(super) fn read_json(path: &Path) -> Result<Value, UpdateError> {
    security::protected(path)?;
    let file = std::fs::OpenOptions::new().read(true).share_mode(1 | 4).open(path).map_err(security::error)?;
    let mut reader = file.take(65537);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(security::error)?;
    if bytes.len() > 65536 { return Err(security::error("更新状态文件过大")); }
    serde_json::from_slice(&bytes).map_err(security::error)
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), UpdateError> {
    let directory = path.parent().ok_or_else(|| security::error("更新状态路径无效"))?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(security::error)?;
    serde_json::to_writer(file.as_file_mut(), value).map_err(security::error)?;
    file.as_file_mut().flush().map_err(security::error)?;
    file.as_file().sync_all().map_err(security::error)?;
    file.persist(path).map_err(|error| security::error(error.error))?;
    Ok(())
}

pub(super) fn publish(root: &Path, task: &ServiceUpdateTask) -> Result<(), UpdateError> {
    write_json(&root.join("status.json"), task)
}

pub(super) fn extract(archive: &Path, stage: &Path) -> Result<(), UpdateError> {
    let file = std::fs::OpenOptions::new().read(true).share_mode(1).open(archive).map_err(security::error)?;
    if file.metadata().map_err(security::error)?.len() > MAX_INSTALLER_BYTES {
        return Err(security::error("更新压缩包体积异常"));
    }
    let mut zip = zip::ZipArchive::new(file).map_err(security::error)?;
    if zip.len() > 100_000 { return Err(security::error("更新包条目过多")); }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(security::error)?;
        let name = entry.name().replace('\\', "/");
        let relative = safe_name(&name)?;
        if !seen.insert(relative.to_string_lossy().to_lowercase()) {
            return Err(security::error("更新包包含重复或大小写冲突的条目"));
        }
        let kind = entry.unix_mode().map(|mode| mode & 0o170000).unwrap_or(0);
        if entry.is_symlink() || !matches!(kind, 0 | 0o100000 | 0o040000) {
            return Err(security::error("更新包包含链接或特殊文件"));
        }
        total = total.saturating_add(entry.size());
        if total > 1024 * 1024 * 1024 || entry.size() > MAX_INSTALLER_BYTES {
            return Err(security::error("更新包解压体积异常"));
        }
        let needed = relative == Path::new("agent2api-server.exe") || relative.starts_with("ui");
        if !needed { continue; }
        let target = stage.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(security::error)?;
        } else {
            if let Some(parent) = target.parent() { std::fs::create_dir_all(parent).map_err(security::error)?; }
            let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(target).map_err(security::error)?;
            let declared = entry.size();
            let copied = std::io::copy(&mut entry.by_ref().take(declared.saturating_add(1)), &mut output).map_err(security::error)?;
            if copied != declared { return Err(security::error("更新包条目长度不一致")); }
            output.sync_all().map_err(security::error)?;
        }
    }
    if !stage.join("ui/index.html").is_file() { return Err(security::error("更新包缺少管理界面")); }
    check_exe(&stage.join("agent2api-server.exe"))
}

fn safe_name(value: &str) -> Result<PathBuf, UpdateError> {
    if value.is_empty() || value.starts_with('/') || value.contains('\0') {
        return Err(security::error("更新包路径不安全"));
    }
    let mut path = PathBuf::new();
    for part in value.trim_end_matches('/').split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty() || part == "." || part == ".." || part.ends_with(['.', ' '])
            || part.chars().any(|c| c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            || matches!(stem.as_str(), "COM¹" | "COM²" | "COM³" | "LPT¹" | "LPT²" | "LPT³")
            || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return Err(security::error("更新包包含 Windows 不安全文件名"));
        }
        path.push(part);
    }
    Ok(path)
}

fn check_exe(path: &Path) -> Result<(), UpdateError> {
    let mut file = std::fs::File::open(path).map_err(security::error)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header).map_err(security::error)?;
    let offset = u32::from_le_bytes(header[60..64].try_into().unwrap());
    if &header[..2] != b"MZ" || offset > 1024 * 1024 { return Err(security::error("更新程序格式错误")); }
    file.seek(SeekFrom::Start(u64::from(offset))).map_err(security::error)?;
    let mut pe = [0u8; 26];
    file.read_exact(&mut pe).map_err(security::error)?;
    if &pe[..4] != b"PE\0\0" || u16::from_le_bytes([pe[4], pe[5]]) != 0x8664
        || u16::from_le_bytes([pe[24], pe[25]]) != 0x020b
    {
        return Err(security::error("更新程序不是 Windows x64 服务端"));
    }
    Ok(())
}

fn rename(from: &Path, to: &Path) -> Result<(), UpdateError> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(error) => return Err(security::error(error)),
        }
    }
}

pub(super) fn install(install: &control::Installation, stage: &Path, task: &mut ServiceUpdateTask, source_pid: u32) -> Result<(), UpdateError> {
    let service = control::open(true)?;
    control::check_service(&service, install, Some(source_pid))?;
    let backup = install.updates.join(format!("backup-{}", task.id));
    std::fs::create_dir(&backup).map_err(security::error)?;
    write_json(&backup.join("version.json"), &json!({ "previousVersion": crate::server::core::update::CURRENT_VERSION,
        "targetVersion": task.version }))?;
    task.set_phase("installing", None);
    publish(&install.updates, task)?;
    if let Err(error) = control::stop(&service, source_pid) {
        return Err(security::error(match control::start(&service) {
            Ok(()) => format!("停止失败，未替换文件，旧服务运行已确认：{}", error.message),
            Err(restart) => format!("停止失败，未替换文件：{}；旧服务恢复未确认：{}", error.message, restart.message),
        }));
    }
    let binary = install.root.join("agent2api-server.exe");
    let old_binary = backup.join("agent2api-server.exe");
    let old_ui = backup.join("ui");
    let mut saved_binary = false;
    let mut saved_ui = false;
    let swapped = (|| {
        rename(&binary, &old_binary)?;
        saved_binary = true;
        rename(&install.ui, &old_ui)?;
        saved_ui = true;
        rename(&stage.join("agent2api-server.exe"), &binary)?;
        rename(&stage.join("ui"), &install.ui)?;
        Ok::<_, UpdateError>(())
    })();
    if let Err(error) = swapped {
        let restore = (|| {
            if saved_binary {
                if binary.exists() { rename(&binary, &stage.join("agent2api-server.exe"))?; }
                rename(&old_binary, &binary)?;
            }
            if saved_ui {
                if install.ui.exists() { rename(&install.ui, &stage.join("ui"))?; }
                rename(&old_ui, &install.ui)?;
            }
            control::start(&service)
        })();
        return Err(security::error(match restore {
            Ok(()) => format!("替换失败，已还原并启动旧服务：{}", error.message),
            Err(restore) => format!("替换失败：{}；恢复未完成：{}；恢复文件：{}", error.message, restore.message, backup.display()),
        }));
    }
    task.set_phase("restarting", None);
    let _ = publish(&install.updates, task);
    let started = control::start(&service).and_then(|_| wait_ready(&service, install, task));
    if let Err(error) = started {
        return Err(security::error(format!("新服务未确认恢复：{}；旧程序保留于 {}，数据库未回退", error.message, backup.display())));
    }
    task.set_phase("completed", None);
    publish(&install.updates, task)
}

fn wait_ready(service: &windows_service::service::Service, install: &control::Installation, task: &ServiceUpdateTask) -> Result<(), UpdateError> {
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        if let Ok(ready) = read_json(&install.updates.join("ready.json")) {
            let status = service.query_status().map_err(security::error)?;
            let pid = ready["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
            if ready["version"].as_str() == Some(task.version.as_str()) && pid != 0
                && ready["startedAt"].as_i64().unwrap_or(0) >= task.updated_at
                && status.current_state == windows_service::service::ServiceState::Running && status.process_id == Some(pid)
                && control::process_is(pid, &install.root.join("agent2api-server.exe"))
            {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(security::error("等待新版本确认超时"))
}
