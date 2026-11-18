//! Magpie siwcHostID 的稳定安装身份。这里只保存随机 host ID，不含账号或令牌。

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

use crate::server::errors::GatewayError;

use super::account;

static HOST_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn valid(value: &str) -> bool {
    let Some(uuid) = value.strip_prefix("urn:uuid:") else {
        return false;
    };
    uuid.len() == 36
        && uuid.bytes().enumerate().all(|(index, ch)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                ch == b'-'
            } else {
                ch.is_ascii_hexdigit()
            }
        })
}

pub(super) fn random_token(size: usize) -> Result<String, GatewayError> {
    let mut bytes = vec![0u8; size];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| GatewayError::new("无法生成 ChatGPT 登录所需的安全随机数"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn random_host() -> Result<String, GatewayError> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| GatewayError::new("无法生成 ChatGPT 安装标识"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn read(path: &Path) -> Result<Option<String>, GatewayError> {
    let info = match std::fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(GatewayError::new("无法读取 ChatGPT 安装标识")),
    };
    if !info.is_file() || info.len() > 128 || info.file_type().is_symlink() {
        return Err(GatewayError::new(
            "ChatGPT 安装标识文件无效，请检查本机配置目录",
        ));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|_| GatewayError::new("无法读取 ChatGPT 安装标识"))?;
    if !valid(text.trim()) {
        return Err(GatewayError::new("ChatGPT 安装标识无效，未覆盖旧标识"));
    }
    Ok(Some(text.trim().to_string()))
}

pub(super) fn get() -> Result<String, GatewayError> {
    let _guard = HOST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let store = account::shared()?;
    let path = store.file().with_file_name("chatgpt-subscription-host");
    if let Some(id) = read(&path)? {
        return Ok(id);
    }
    let parent = path
        .parent()
        .ok_or_else(|| GatewayError::new("ChatGPT 配置目录无效"))?;
    std::fs::create_dir_all(parent).map_err(|_| GatewayError::new("无法创建 ChatGPT 配置目录"))?;
    let id = random_host()?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| GatewayError::new("无法保存 ChatGPT 安装标识"))?;
    file.write_all(format!("{id}\n").as_bytes())
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| GatewayError::new("无法保存 ChatGPT 安装标识"))?;
    match file.persist_noclobber(&path) {
        Ok(_) => Ok(id),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            read(&path)?.ok_or_else(|| GatewayError::new("ChatGPT 安装标识被其他进程修改，请重试"))
        }
        Err(_) => Err(GatewayError::new("无法保存 ChatGPT 安装标识")),
    }
}
