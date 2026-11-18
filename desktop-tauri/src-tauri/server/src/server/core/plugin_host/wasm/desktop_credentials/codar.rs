//! 固定读取当前用户的 Codar 配置；不接受插件路径，不跟随链接，不写回官方文件。
use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};

use super::{file, home, invalid};

fn candidate(profile: &str, value: &Value) -> Option<Value> {
    let token = value["speedai"]["token"].as_str()?.trim();
    if token.is_empty()
        || token.len() > 8192
        || token.split('.').count() != 3
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(token.split('.').nth(1)?.trim_end_matches('='))
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let username = claims["username"].as_str()?.trim();
    if username.is_empty() || username.len() > 256 || username.chars().any(char::is_control) {
        return None;
    }
    Some(json!({ "profile": profile, "username": username, "token": token }))
}

pub(super) fn read() -> Result<Value, proto::WireError> {
    let root = match std::env::var_os("CODAR_HOME").filter(|path| !path.is_empty()) {
        Some(path) => {
            let path = std::path::PathBuf::from(path);
            if !path.is_absolute() {
                return Err(invalid());
            }
            path
        }
        None => home()?.join(".codar"),
    };
    let bytes = zeroize::Zeroizing::new(file(&root.join("config.json"), 256 * 1024)?);
    let value: Value = super::super::managed_task::declaration::parse_object(&bytes, 256 * 1024)
        .map_err(|_| invalid())?;
    let provider = &value["provider"];
    let active = provider["active_profile"].as_str().unwrap_or("speedai");
    if active.is_empty() || active.len() > 128 || active.chars().any(char::is_control) {
        return Err(invalid());
    }
    let profiles = provider["provider_profiles"]
        .as_object()
        .or_else(|| value["provider_profiles"].as_object());
    if let Some(selected) = profiles.and_then(|profiles| profiles.get(active)) {
        return candidate(active, selected).ok_or_else(invalid);
    }
    candidate(active, provider).ok_or_else(invalid)
}
