use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use std::path::{Path, PathBuf};

use super::{absolute_env, home, invalid, object, project};

#[cfg(windows)]
fn registry_root(name: &str) -> Option<std::path::PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let key = HSTRING::from("Software\\AStudioStorage");
    let name = HSTRING::from(name);
    let mut bytes = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    }
    .ok()
    .ok()?;
    if bytes == 0 || bytes > 8192 {
        return None;
    }
    let mut buffer = vec![0u16; bytes as usize / 2 + 1];
    let mut size = buffer.len() as u32 * 2;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()
    .ok()?;
    let text =
        String::from_utf16(&buffer[..(size as usize / 2).saturating_sub(1).min(buffer.len())])
            .ok()?;
    let path = std::path::PathBuf::from(text);
    path.is_absolute().then_some(path)
}

pub(super) fn read() -> Result<Value, proto::WireError> {
    let mut roots = ["ASTUDIO_HOME", "SYNARA_HOME"]
        .into_iter()
        .filter_map(absolute_env)
        .collect::<Vec<_>>();
    #[cfg(windows)]
    for name in [
        "ProductionDataRoot",
        "CanaryDataRoot",
        "DevelopmentDataRoot",
    ] {
        if let Some(root) = registry_root(name) {
            roots.push(root);
        }
    }
    let mut candidates: Vec<(PathBuf, PathBuf)> = roots
        .iter()
        .map(|root| {
            (
                root.join("userdata/astron-session.json"),
                root.join("runtime/acode-home/config.toml"),
            )
        })
        .collect();
    let fallback = home()?.join(".acode/acode");
    candidates.push((
        fallback.join("userdata/astron-session.json"),
        fallback.join("config.toml"),
    ));
    for (session, config) in &candidates {
        if !exists(session)? {
            continue;
        }
        let value = object(session, 256 * 1024)?;
        if value["banned"] == true {
            return Err(invalid());
        }
        let mut credentials = project(
            &value,
            &[
                "modelBearerToken",
                "uid",
                "token",
                "accountId",
                "ssoSessionId",
                "banned",
            ],
        );
        // 只从同一数据根取配套票据，不能把另一个频道的模型密钥接到当前网页账号。
        // 官方 config 与 session 的 Bearer 用途可不同；网页 token 绝不投成模型凭据。
        let configured = config_credentials(config)?;
        if !configured.is_null() {
            let config_uid = scalar(&configured["uid"]);
            let session_uid = scalar(&credentials["uid"]);
            if !config_uid.is_empty()
                && config_uid != "0"
                && !session_uid.is_empty()
                && session_uid != "0"
                && config_uid != session_uid
            {
                return Err(invalid());
            }
            credentials["key"] = configured["key"].clone();
            if session_uid.is_empty() || session_uid == "0" {
                credentials["uid"] = configured["uid"].clone();
            }
        }
        let model = ["key", "modelBearerToken"]
            .iter()
            .any(|key| present(&credentials[*key]));
        let web = present(&credentials["ssoSessionId"])
            || present(&credentials["token"]) && !scalar(&credentials["accountId"]).is_empty();
        let legacy_model = present(&credentials["token"]) && !web;
        if model || web || legacy_model {
            return Ok(json!({"credentials": credentials}));
        }
    }
    for (_, config) in candidates {
        let credentials = config_credentials(&config)?;
        if !credentials.is_null() {
            return Ok(json!({"credentials": credentials}));
        }
    }
    Err(invalid())
}

fn exists(path: &Path) -> Result<bool, proto::WireError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(invalid()),
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(value) => value.trim().to_string(),
        Value::Number(value) if value.is_i64() || value.is_u64() => value.to_string(),
        _ => String::new(),
    }
}

fn present(value: &Value) -> bool {
    value.as_str().is_some_and(|value| !value.trim().is_empty())
}

fn config_credentials(path: &Path) -> Result<Value, proto::WireError> {
    if !exists(path)? {
        return Ok(Value::Null);
    }
    let bytes = zeroize::Zeroizing::new(super::file(path, 256 * 1024)?);
    let raw = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    let value: toml::Value = toml::from_str(raw).map_err(|_| invalid())?;
    let Some(provider) = value
        .get("model_providers")
        .and_then(|value| value.get("astron-spark"))
    else {
        return Ok(Value::Null);
    };
    let key = provider
        .get("experimental_bearer_token")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if key.is_empty() {
        return Ok(Value::Null);
    }
    let uid = provider
        .get("uid")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    Ok(json!({"key": key, "uid": uid}))
}
