use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) fn architecture() -> Value {
    json!(std::env::consts::ARCH)
}

pub(super) fn client_fingerprint() -> Result<Value, proto::WireError> {
    let hostname = std::env::var("COMPUTERNAME")
        .ok().filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|value| !value.trim().is_empty()))
        .ok_or_else(|| super::error("客户端标识来源不可用"))?;
    if hostname.len() > 256 || hostname.chars().any(char::is_control) {
        return Err(super::error("客户端标识来源无效"));
    }
    let mut digest = Sha256::new();
    digest.update(b"agent2api:");
    digest.update(hostname.as_bytes());
    Ok(json!(format!("{:x}", digest.finalize())))
}
