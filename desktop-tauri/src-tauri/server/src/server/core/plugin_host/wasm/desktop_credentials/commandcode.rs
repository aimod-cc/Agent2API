//! Command Code 官方 CLI 登录文件的固定只读快照；不读取远程控制或其他客户端凭据。
use agent2api_provider_protocol as proto;
use serde_json::json;

use super::{content, home, invalid, object};

pub(super) fn read() -> Result<serde_json::Value, proto::WireError> {
    let path = home()?.join(".commandcode/auth.json");
    let value = object(&path, 64 * 1024)?;
    let key = value["apiKey"]
        .as_str()
        .map(str::trim)
        .filter(|key| {
            key.len() <= 8192
                && key.strip_prefix("user_").is_some_and(|body| {
                    !body.is_empty()
                        && body
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
                })
        })
        .ok_or_else(invalid)?;
    // apiKey 是官方读取形态；不投影 remoteControl、BYOK 或客户端全局配置。
    content(&path, json!({ "apiKey": key }))
}
