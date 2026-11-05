use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{UpdateTask, READY_PATH, STATUS_PATH};
use crate::server::core::update::{selfupdate, UpdateError};

pub(super) const STATE_DIR: &str = "/var/lib/agent2api-updater";
const INSTALL_DIR: &str = "/opt/agent2api";
const SERVICE: &str = "agent2api.service";

fn io(error: std::io::Error) -> UpdateError { UpdateError::new(error.to_string()) }

pub(super) fn check_installation() -> Result<(), UpdateError> {
    for name in [STATE_DIR, INSTALL_DIR, "/opt/agent2api/agent2api-server", "/opt/agent2api/ui"] {
        let metadata = std::fs::symlink_metadata(name).map_err(io)?;
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 || metadata.file_type().is_symlink() {
            return Err(UpdateError::new(format!("安装位置权限不安全：{name}，请用新安装包执行升级脚本")));
        }
        if std::fs::canonicalize(name).map_err(io)? != Path::new(name) {
            return Err(UpdateError::new("升级安装路径不能通过符号链接重定向"));
        }
    }
    if selfupdate::current_exe_path()? != Path::new(INSTALL_DIR).join("agent2api-server") {
        return Err(UpdateError::new("升级程序不在受管理的安装位置"));
    }
    Ok(())
}

pub(super) fn publish(task: &UpdateTask) -> Result<(), UpdateError> {
    let mut file = tempfile::NamedTempFile::new_in(STATE_DIR).map_err(io)?;
    serde_json::to_writer(file.as_file_mut(), task).map_err(|error| UpdateError::new(error.to_string()))?;
    file.as_file().set_permissions(std::fs::Permissions::from_mode(0o644)).map_err(io)?;
    file.as_file().sync_all().map_err(io)?;
    file.persist(STATUS_PATH).map_err(|error| io(error.error))?;
    Ok(())
}

pub(super) struct Prepared {
    _stage: tempfile::TempDir,
    binary: PathBuf,
    ui: PathBuf,
}

pub(super) fn prepare(archive: &Path) -> Result<Prepared, UpdateError> {
    let stage = tempfile::Builder::new().prefix(".update-").tempdir_in(INSTALL_DIR).map_err(io)?;
    let binary_dir = stage.path().join("bin");
    let ui = stage.path().join("ui");
    std::fs::create_dir(&binary_dir).map_err(io)?;
    std::fs::create_dir(&ui).map_err(io)?;
    let extracted = selfupdate::extract_bundle(archive, &binary_dir, Some(&ui))?;
    if !extracted.binary || extracted.ui_files == 0 || !ui.join("index.html").is_file() {
        return Err(UpdateError::new("更新包不完整，缺少程序或管理界面；现有程序未修改"));
    }
    let binary = binary_dir.join("agent2api-server");
    let mut header = [0u8; 20];
    std::fs::File::open(&binary).map_err(io)?.read_exact(&mut header).map_err(io)?;
    let machine = u16::from_le_bytes([header[18], header[19]]);
    let expected_machine = if cfg!(target_arch = "aarch64") { 183 } else { 62 };
    if &header[..4] != b"\x7fELF" || header[4] != 2 || header[5] != 1 || machine != expected_machine {
        return Err(UpdateError::new("更新程序的格式或 CPU 架构不匹配，现有程序未修改"));
    }
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).map_err(io)?;
    Ok(Prepared { _stage: stage, binary, ui })
}

/// 只操作固定安装位置。先停服务再替换，文件阶段失败完整还原；成功后保留恢复副本。
pub(super) fn apply(prepared: Prepared, task: &mut UpdateTask) -> Result<(), UpdateError> {
    check_installation()?;
    let user = systemctl(&["show", SERVICE, "--property=User", "--value"], Duration::from_secs(5))?;
    let working = systemctl(&["show", SERVICE, "--property=WorkingDirectory", "--value"], Duration::from_secs(5))?;
    let command = systemctl(&["show", SERVICE, "--property=ExecStart", "--value"], Duration::from_secs(5))?;
    if user.trim() != "agent2api" || working.trim() != INSTALL_DIR
        || !command.contains("path=/opt/agent2api/agent2api-server ;")
    {
        return Err(UpdateError::new("主服务已自定义，一键更新不会修改其程序，请按原部署方式升级"));
    }
    let backup = tempfile::Builder::new().prefix(".before-update-").tempdir_in(INSTALL_DIR).map_err(io)?.keep();
    std::fs::write(backup.join("version.json"), json!({
        "previousVersion": crate::server::core::update::CURRENT_VERSION,
        "targetVersion": task.version,
        "createdAt": crate::server::logging::now_ms(),
    }).to_string()).map_err(io)?;
    task.set_phase("installing", None);
    publish(task)?;
    if let Err(error) = systemctl(&["stop", SERVICE], Duration::from_secs(90)) {
        let _ = systemctl(&["start", SERVICE], Duration::from_secs(90));
        return Err(UpdateError::new(format!("停止服务未完成，未替换程序：{}", error.message)));
    }
    let binary = Path::new(INSTALL_DIR).join("agent2api-server");
    let ui = Path::new(INSTALL_DIR).join("ui");
    let old_binary = backup.join("agent2api-server");
    let old_ui = backup.join("ui");
    let mut binary_saved = false;
    let mut ui_saved = false;
    let swap = (|| {
        std::fs::rename(&binary, &old_binary).map_err(io)?;
        binary_saved = true;
        std::fs::rename(&ui, &old_ui).map_err(io)?;
        ui_saved = true;
        std::fs::rename(&prepared.binary, &binary).map_err(io)?;
        std::fs::rename(&prepared.ui, &ui).map_err(io)?;
        Ok::<_, UpdateError>(())
    })();
    if let Err(error) = swap {
        let restore = (|| {
            if binary_saved {
                if binary.exists() { std::fs::remove_file(&binary).map_err(io)?; }
                std::fs::rename(&old_binary, &binary).map_err(io)?;
            }
            if ui_saved {
                if ui.exists() { std::fs::remove_dir_all(&ui).map_err(io)?; }
                std::fs::rename(&old_ui, &ui).map_err(io)?;
            }
            systemctl(&["start", SERVICE], Duration::from_secs(90))?;
            Ok::<_, UpdateError>(())
        })();
        return Err(UpdateError::new(match restore {
            Ok(()) => format!("替换失败，已还原旧程序并重新启动：{}", error.message),
            Err(restore) => format!("替换失败：{}；恢复未完成：{}。恢复文件保留于 {}", error.message, restore.message, backup.display()),
        }));
    }
    task.set_phase("restarting", None);
    // 程序已经替换，此时状态文件写失败也必须继续拉起服务。
    let _ = publish(task);
    let _ = std::fs::OpenOptions::new().write(true).truncate(true).open(READY_PATH);
    let restarted = systemctl(&["start", SERVICE], Duration::from_secs(90))
        .and_then(|_| wait_ready(task));
    if let Err(error) = restarted {
        return Err(UpdateError::new(format!(
            "新版本尚未确认启动成功：{}。旧程序与界面保留于 {}；数据未回退，请检查服务日志", error.message, backup.display())));
    }
    task.set_phase("completed", None);
    publish(task)
}

fn wait_ready(task: &UpdateTask) -> Result<(), UpdateError> {
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        let ready = std::fs::File::open(READY_PATH).ok().and_then(|file| {
            serde_json::from_reader::<_, Value>(std::io::Read::take(file, 4096)).ok()
        });
        if let Some(ready) = ready {
            let pid = ready["pid"].as_u64().unwrap_or(0);
            if ready["version"].as_str() == Some(task.version.as_str()) && pid > 0
                && ready["startedAt"].as_i64().unwrap_or(0) >= task.started_at
            {
                let main_pid = systemctl(&["show", SERVICE, "--property=MainPID", "--value"], Duration::from_secs(5))?;
                if main_pid.trim().parse::<u64>().ok() == Some(pid)
                    && std::fs::read_link(format!("/proc/{pid}/exe")).ok().as_deref()
                        == Some(Path::new("/opt/agent2api/agent2api-server"))
                    && systemctl(&["is-active", SERVICE], Duration::from_secs(5))?.trim() == "active"
                {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(UpdateError::new("等待新版服务就绪超时"))
}

fn systemctl(args: &[&str], timeout: Duration) -> Result<String, UpdateError> {
    let mut child = Command::new("/usr/bin/systemctl").args(args)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(io)?;
    let deadline = Instant::now() + timeout;
    while child.try_wait().map_err(io)?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UpdateError::new("等待系统服务操作超时"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let output = child.wait_with_output().map_err(io)?;
    if !output.status.success() {
        return Err(UpdateError::new(format!("系统服务操作失败：{}", String::from_utf8_lossy(&output.stderr).trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
