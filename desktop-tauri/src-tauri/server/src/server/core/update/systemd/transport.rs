use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{UpdateTask, SOCKET_PATH};
use crate::server::core::update::UpdateError;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Request {
    pub action: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub github_token: Option<String>,
}

pub(super) fn peer_uid(fd: RawFd) -> Result<u32, UpdateError> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(), &mut size)
    };
    if result != 0 || size as usize != std::mem::size_of::<libc::ucred>() {
        return Err(UpdateError::new("无法核验本机升级通道身份"));
    }
    Ok(credentials.uid)
}

fn connect() -> Result<UnixStream, UpdateError> {
    let metadata = std::fs::symlink_metadata(SOCKET_PATH).map_err(io_error)?;
    if metadata.uid() != 0 || !metadata.file_type().is_socket() || metadata.mode() & 0o007 != 0 {
        return Err(UpdateError::new("升级通道权限异常"));
    }
    let stream = UnixStream::connect(SOCKET_PATH).map_err(io_error)?;
    if peer_uid(stream.as_raw_fd())? != 0 {
        return Err(UpdateError::new("升级通道不是由系统管理员提供"));
    }
    stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(io_error)?;
    stream.set_write_timeout(Some(Duration::from_secs(30))).map_err(io_error)?;
    Ok(stream)
}

pub(super) fn read_line<R: BufRead>(reader: &mut R) -> Result<String, UpdateError> {
    let mut bytes = Vec::new();
    reader.take(8193).read_until(b'\n', &mut bytes).map_err(io_error)?;
    if bytes.len() > 8192 || bytes.last() != Some(&b'\n') {
        return Err(UpdateError::new("升级通道消息无效或过大"));
    }
    String::from_utf8(bytes).map_err(|_| UpdateError::new("升级通道消息编码无效"))
}

fn response(reader: &mut impl BufRead) -> Result<Value, UpdateError> {
    let value: Value = serde_json::from_str(&read_line(reader)?)
        .map_err(|_| UpdateError::new("升级程序返回了无效响应"))?;
    if value["ok"] != true {
        return Err(UpdateError::new(value["error"].as_str().unwrap_or("升级程序拒绝了请求")));
    }
    Ok(value)
}

/// 能力探测只短连，不停服务、不读网络；短缓存避免状态轮询反复启动进程。
pub(super) fn probe() -> Result<(), UpdateError> {
    static CACHE: Mutex<Option<(Instant, Result<(), UpdateError>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
    if let Some((time, result)) = cache.as_ref() {
        if time.elapsed() < Duration::from_secs(15) { return result.clone(); }
    }
    let result = (|| {
        let mut stream = connect()?;
        stream.write_all(b"{\"action\":\"probe\"}\n").map_err(io_error)?;
        response(&mut BufReader::new(stream))?;
        Ok(())
    })();
    *cache = Some((Instant::now(), result.clone()));
    result
}

pub(super) fn submit(task: &UpdateTask, archive: &Path) -> Result<(), UpdateError> {
    let mut file = std::fs::File::open(archive).map_err(io_error)?;
    let size = file.metadata().map_err(io_error)?.len();
    let mut stream = connect()?;
    let request = Request {
        action: "install".to_string(), id: task.id.clone(), version: task.version.clone(),
        started_at: task.started_at, size,
        github_token: crate::server::core::update::token::effective_token(),
    };
    serde_json::to_writer(&mut stream, &request).map_err(|error| UpdateError::new(error.to_string()))?;
    stream.write_all(b"\n").map_err(io_error)?;
    let mut reader = BufReader::new(stream);
    response(&mut reader)?;
    let copied = std::io::copy(&mut file, reader.get_mut()).map_err(io_error)?;
    if copied != size { return Err(UpdateError::new("更新包发送不完整")); }
    reader.get_mut().flush().map_err(io_error)?;
    response(&mut reader)?;
    Ok(())
}

pub(super) fn io_error(error: std::io::Error) -> UpdateError {
    UpdateError::new(format!("本机升级通道失败：{error}"))
}
