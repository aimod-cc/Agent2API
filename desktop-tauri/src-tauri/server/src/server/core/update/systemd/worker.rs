use std::io::{BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

use serde_json::json;

use super::{files, transport, trust, UpdateTask};
use crate::server::core::update::{compare_versions, selfupdate, UpdateError, CURRENT_VERSION, MAX_INSTALLER_BYTES};

pub(super) fn run() -> i32 {
    let result = handle();
    if let Err(error) = result {
        let _ = respond(json!({ "ok": false, "error": error.message }));
        eprintln!("systemd 升级失败：{}", error.message);
        1
    } else { 0 }
}

fn respond(value: serde_json::Value) -> Result<(), UpdateError> {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &value).map_err(|error| UpdateError::new(error.to_string()))?;
    output.write_all(b"\n").map_err(transport::io_error)?;
    output.flush().map_err(transport::io_error)
}

fn handle() -> Result<(), UpdateError> {
    if unsafe { libc::geteuid() } != 0 { return Err(UpdateError::new("升级程序必须由系统服务启动")); }
    let stdin = std::io::stdin();
    let peer = transport::peer_uid(stdin.as_raw_fd())?;
    let service_uid = std::env::var("AGENT2API_UPDATER_UID").ok().and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| UpdateError::new("升级组件缺少服务账号配置"))?;
    if service_uid == 0 || peer != service_uid { return Err(UpdateError::new("无权访问本机升级入口")); }
    files::check_installation()?;
    let timeout = libc::timeval { tv_sec: 60, tv_usec: 0 };
    let result = unsafe {
        libc::setsockopt(stdin.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVTIMEO,
            (&timeout as *const libc::timeval).cast(), std::mem::size_of_val(&timeout) as libc::socklen_t)
    };
    if result != 0 { return Err(UpdateError::new("无法设置升级通道超时")); }
    let mut reader = BufReader::new(stdin.lock());
    let request: transport::Request = serde_json::from_str(&transport::read_line(&mut reader)?)
        .map_err(|_| UpdateError::new("升级请求格式无效"))?;
    if request.action == "probe" { return respond(json!({ "ok": true })); }
    if request.action != "install" || !valid_version(&request.version)
        || request.id.len() > 80 || request.id.is_empty()
        || !request.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || request.size == 0 || request.size > MAX_INSTALLER_BYTES
        || compare_versions(&request.version, CURRENT_VERSION) != Some(1)
    {
        return Err(UpdateError::new("更新请求版本或大小无效，不支持降级"));
    }
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .mode(0o600).open(format!("{}/update.lock", files::STATE_DIR)).map_err(transport::io_error)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(UpdateError::with_status(409, "已有升级任务正在执行"));
    }
    files::check_installation()?;
    let now = crate::server::logging::now_ms();
    if request.started_at > now + 60_000 || request.started_at < now - 24 * 60 * 60_000 {
        return Err(UpdateError::new("更新请求已过期，请重新检查更新"));
    }
    let mut task = UpdateTask {
        id: request.id, version: request.version, phase: "verifying".to_string(),
        started_at: request.started_at, updated_at: now, error: None,
    };
    files::publish(&task)?;
    let result = (|| {
        let mut archive = tempfile::NamedTempFile::new_in(files::STATE_DIR).map_err(transport::io_error)?;
        respond(json!({ "ok": true, "receiving": true }))?;
        let copied = std::io::copy(&mut reader.by_ref().take(request.size), archive.as_file_mut())
            .map_err(transport::io_error)?;
        if copied != request.size { return Err(UpdateError::new("更新包传输未完成，现有程序未修改")); }
        archive.as_file().sync_all().map_err(transport::io_error)?;
        // 接收完整载荷后即可让网页返回；后面的校验、替换、重启不依赖网页存活。
        respond(json!({ "ok": true, "accepted": true, "id": task.id }))?;
        let official = trust::verify(&task.version, request.github_token.as_deref())?;
        if official.size != request.size || selfupdate::file_sha256(archive.path())? != official.sha256 {
            return Err(UpdateError::new("更新包与官方校验值不一致，现有程序未修改"));
        }
        let prepared = files::prepare(archive.path())?;
        files::apply(prepared, &mut task)
    })();
    if let Err(error) = &result {
        task.set_phase("failed", Some(error.message.clone()));
        let _ = files::publish(&task);
    }
    drop(lock);
    result
}

fn valid_version(value: &str) -> bool {
    !value.is_empty() && value.len() <= 100
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        && crate::server::core::update::parse_version(value).is_some()
}
