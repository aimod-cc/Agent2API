use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{control, files, security};
use crate::server::core::update::{selfupdate, service_task::ServiceUpdateTask, service_trust, UpdateError, CURRENT_VERSION};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    task: ServiceUpdateTask,
    source_pid: u32,
    updater_sha256: String,
}

pub(super) fn launch(task: &ServiceUpdateTask, archive: &Path, token: Option<String>) -> Result<(), UpdateError> {
    let install = control::installation()?;
    let service = control::open(true)?;
    control::check_service(&service, &install, Some(std::process::id()))?;
    let run = install.updates.join(&task.id);
    std::fs::create_dir(&run).map_err(security::error)?;
    security::protected(&run)?;
    let source = selfupdate::current_exe_path()?;
    let updater = run.join("updater.exe");
    std::fs::copy(&source, &updater).map_err(security::error)?;
    let updater_sha256 = selfupdate::file_sha256(&source)?;
    if selfupdate::file_sha256(&updater)? != updater_sha256 { return Err(security::error("升级程序副本校验失败")); }
    std::fs::copy(archive, run.join("package.zip")).map_err(security::error)?;
    files::write_json(&run.join("request.json"), &Request { task: task.clone(), source_pid: std::process::id(), updater_sha256 })?;
    let log = std::fs::OpenOptions::new().create_new(true).write(true).open(run.join("updater.log")).map_err(security::error)?;
    let mut command = Command::new(&updater);
    command.arg("--windows-updater").arg(&task.id).current_dir(&run)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::from(log))
        .creation_flags(0x0000_0008 | 0x0000_0200);
    let mut child = command.spawn().map_err(security::error)?;
    let worker_pid = child.id();
    let worker_started = control::process_started_at(&control::process(worker_pid)?)?;
    files::write_json(&run.join("process.json"), &json!({ "pid": worker_pid, "createdAt": worker_started }))?;
    // 凭证只经继承的匿名管道传递，不进入命令行、日志或磁盘文件。
    let mut input = child.stdin.take().ok_or_else(|| security::error("升级进程输入通道未就绪"))?;
    if let Err(error) = serde_json::to_writer(&mut input, &json!({ "githubToken": token }))
        .map_err(security::error).and_then(|_| input.flush().map_err(security::error))
    {
        drop(input);
        let _ = child.wait();
        return Err(error);
    }
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if run.join("accepted.json").is_file() {
            files::write_json(&run.join("proceed.json"), &json!({ "id": task.id }))?;
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(security::error)? {
            return Err(security::error(format!("升级进程未能接手（{status}），请查看升级日志")));
        }
        if Instant::now() >= deadline {
            return Err(security::error("升级进程交接超时，未执行安装；请查看升级日志后重试"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn active(id: &str) -> Option<bool> {
    if !valid_id(id) { return Some(false); }
    let install = control::installation().ok()?;
    let run = install.updates.join(id);
    let record = files::read_json(&run.join("process.json")).ok()?;
    let pid = record["pid"].as_u64().and_then(|value| u32::try_from(value).ok())?;
    let created = record["createdAt"].as_u64()?;
    Some(control::worker_running(pid, created, &run.join("updater.exe")))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

pub(super) fn run(id: &str) -> i32 {
    match execute(id) {
        Ok(()) => 0,
        Err(error) => { eprintln!("{}", error.message); 1 }
    }
}

fn execute(id: &str) -> Result<(), UpdateError> {
    if !valid_id(id) {
        return Err(security::error("升级任务编号不正确"));
    }
    if !security::is_system()? { return Err(security::error("升级进程必须由已登记的系统服务启动")); }
    if security::in_job()? { return Err(security::error("服务受外部进程组托管，不能保证升级期间独立运行")); }
    let install = control::installation()?;
    let run = install.updates.join(id);
    security::protected(&run)?;
    let current = selfupdate::current_exe_path()?;
    if !security::same_path(&current, &run.join("updater.exe")) {
        return Err(security::error("升级程序运行位置不正确"));
    }
    let request: Request = serde_json::from_value(files::read_json(&run.join("request.json"))?).map_err(security::error)?;
    let mut task = request.task;
    let now = crate::server::logging::now_ms();
    if task.id != id || super::super::compare_versions(&task.version, CURRENT_VERSION) != Some(1)
        || task.started_at > now + 60_000 || task.started_at < now - 24 * 60 * 60_000
        || task.version.len() > 100
        || !task.version.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
    {
        return Err(security::error("升级版本或任务时效不正确"));
    }
    if selfupdate::file_sha256(&current)? != request.updater_sha256
        || selfupdate::file_sha256(&install.root.join("agent2api-server.exe"))? != request.updater_sha256
    {
        return Err(security::error("升级程序与当前安装不一致"));
    }
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).share_mode(0)
        .open(install.updates.join("update.lock")).map_err(|_| security::error("已有升级任务正在执行"))?;
    let service = control::open(true)?;
    control::check_service(&service, &install, Some(request.source_pid))?;
    let mut input = Vec::new();
    std::io::stdin().take(2049).read_to_end(&mut input).map_err(security::error)?;
    if input.len() > 2048 { return Err(security::error("升级凭证消息过大")); }
    let secret: serde_json::Value = serde_json::from_slice(&input).map_err(security::error)?;
    task.set_phase("verifying", None);
    files::publish(&install.updates, &task)?;
    files::write_json(&run.join("accepted.json"), &json!({ "id": id }))?;
    let result = (|| {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !run.join("proceed.json").is_file() {
            if Instant::now() >= deadline { return Err(security::error("主服务未确认交接，未执行安装")); }
            std::thread::sleep(Duration::from_millis(100));
        }
        let official = service_trust::verify_from(&task.version, secret["githubToken"].as_str(), control::repository().as_deref())?;
        let archive = run.join("package.zip");
        security::protected(&archive)?;
        if archive.metadata().map_err(security::error)?.len() != official.size
            || selfupdate::file_sha256(&archive)? != official.sha256
        {
            return Err(security::error("更新包与官方校验值不一致，现有程序未修改"));
        }
        let stage = run.join("stage");
        std::fs::create_dir(&stage).map_err(security::error)?;
        files::extract(&archive, &stage)?;
        files::install(&install, &stage, &mut task, request.source_pid)
    })();
    if let Err(error) = &result {
        task.set_phase("failed", Some(error.message.clone()));
        let _ = files::publish(&install.updates, &task);
    }
    drop(lock);
    result
}
