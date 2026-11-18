use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::{component::Component, invalid, HELPER};
use super::super::native;

struct Cached { key: String, value: Value, at: Instant }
fn cache() -> &'static Mutex<Vec<Cached>> { static CACHE: OnceLock<Mutex<Vec<Cached>>> = OnceLock::new(); CACHE.get_or_init(Mutex::default) }

pub(super) async fn identity(component: &Component, account: &str, uid: &str, secret_digest: &str,
    cancel: CancellationToken, deadline: tokio::time::Instant) -> Result<Value, proto::WireError> {
    native::authorized_builtin("qoder-intl", HELPER)?;
    let key = format!("{account}:{uid}:{secret_digest}:{}", component.digest);
    if let Some(value) = cache().lock().map_err(|_| invalid())?.iter()
        .find(|item| item.key == key && item.at.elapsed() < Duration::from_secs(50 * 60)).map(|item| item.value.clone()) {
        return Ok(value);
    }
    let mut command = tokio::process::Command::new(&component.executable);
    command.current_dir(component.executable.parent().ok_or_else(invalid)?)
        .arg(component.environment.to_string()).kill_on_drop(true)
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
        .stdin(if cfg!(any(windows, target_os = "macos")) { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    command.env_remove("LD_PRELOAD").env_remove("DYLD_INSERT_LIBRARIES").env_remove("NODE_OPTIONS");
    #[cfg(windows)] command.creation_flags(0x0800_0000);
    #[cfg(unix)] command.process_group(0);
    let mut child = command.spawn().map_err(|_| invalid())?;
    let guard = super::super::super::process_guard::ProcessGuard::new(&child).map_err(|_| invalid())?;
    let work = async {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(format!("{}\n", json!({ "account": uid })).as_bytes()).await.map_err(|_| invalid())?;
            stdin.shutdown().await.map_err(|_| invalid())?;
        }
        let mut stdout = child.stdout.take().ok_or_else(invalid)?;
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        let mut chunk = [0u8; 8192];
        loop {
            let count = stdout.read(&mut chunk).await.map_err(|_| invalid())?;
            if count == 0 { break; }
            if bytes.len().saturating_add(count) > 1024 * 1024 { return Err(invalid()); }
            bytes.extend_from_slice(&chunk[..count]);
        }
        if !child.wait().await.map_err(|_| invalid())?.success() { return Err(invalid()); }
        let line = bytes.split(|byte| *byte == b'\n').next().ok_or_else(invalid)?;
        let value: Value = super::super::managed_task::declaration::parse_object(line, 16 * 1024).map_err(|_| invalid())?;
        let mut result = json!({});
        for key in ["machineToken", "machineCode", "machineType"] {
            let text = value[key].as_str().filter(|value| !value.is_empty() && value.len() <= 4096
                && !value.chars().any(char::is_control)).ok_or_else(invalid)?;
            result[key] = json!(text);
        }
        Ok(result)
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(invalid()),
        result = tokio::time::timeout_at(deadline.min(tokio::time::Instant::now() + Duration::from_secs(25)), work) => result.map_err(|_| invalid())?,
    };
    guard.kill();
    let _ = child.kill().await;
    let result = result?;
    native::authorized_builtin("qoder-intl", HELPER)?;
    if cancel.is_cancelled() { return Err(invalid()); }
    let mut cache = cache().lock().map_err(|_| invalid())?;
    cache.retain(|item| item.at.elapsed() < Duration::from_secs(50 * 60));
    if cache.len() >= 128 { cache.remove(0); }
    cache.push(Cached { key, value: result.clone(), at: Instant::now() });
    Ok(result)
}
