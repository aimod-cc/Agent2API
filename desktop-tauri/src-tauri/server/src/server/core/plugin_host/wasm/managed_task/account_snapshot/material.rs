//! 固定请求材料只从真实RowSource解释；完整授权投影与出生版本仍由原来源保留。
//! 摘要只用于现有Binding，不把相同字符串当作来源、不从JSON重建RowSource。

use std::io;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::super::{input::FrozenUserAgent, policy::ProjectedCookie};
use super::{
    bounded_text, check_call, source_matches_call, Material, SnapshotError, MAX_IDENTITY_BYTES,
    MAX_SECRET_BYTES,
};
use crate::server::core::account_store::credential_version::RowSource;
use crate::server::core::plugin_host::wasm::conversation::TaskCallRef;

pub(super) fn from_source(
    source: RowSource,
    call: &TaskCallRef,
) -> Result<Material, SnapshotError> {
    check_call(call)?;
    if !source_matches_call(&source, call) || !bounded_text(source.identity(), MAX_IDENTITY_BYTES) {
        return Err(SnapshotError::InvalidAccount);
    }
    // RowSource已在有界解析与真实COMMIT之后产生；这里仅借用，不再克隆或解析整份secret。
    let secret = source.secret();
    if !secret.is_object() {
        return Err(SnapshotError::InvalidCredential);
    }
    let credential_digest = digest(secret)?;
    let mut token = None;
    for key in ["accessToken", "session", "token"] {
        let Some(raw) = secret.get(key).filter(|value| !value.is_null()) else { continue; };
        let candidate = raw.as_str().ok_or(SnapshotError::InvalidCredential)?.trim();
        if candidate.is_empty() { continue; }
        if token.is_some_and(|previous| previous != candidate) {
            return Err(SnapshotError::InvalidCredential);
        }
        token = Some(candidate);
    }
    let token = token.ok_or(SnapshotError::InvalidCredential)?;
    let cookie = ProjectedCookie::from_trusted_access_token(token)
        .map_err(|_| SnapshotError::InvalidCredential)?;
    let raw_user_agent = crate::server::core::user_agent::task_value(
        source.provider().id(), source.user_agent().unwrap_or(""),
    );
    if raw_user_agent.chars().any(char::is_control) {
        return Err(SnapshotError::InvalidUserAgent);
    }
    let user_agent = raw_user_agent.trim();
    if user_agent.is_empty() {
        return Err(SnapshotError::MissingUserAgent);
    }
    let user_agent = FrozenUserAgent::from_host_value(user_agent)
        .map_err(|_| SnapshotError::InvalidUserAgent)?;
    Ok(Material {
        source,
        credential_digest,
        cookie,
        user_agent,
    })
}

/// 借用有界真实投影直接摘要；不另存完整字节，也不把摘要反馈成来源凭证。
fn digest(secret: &Value) -> Result<[u8; 32], SnapshotError> {
    struct DigestWriter {
        hash: Sha256,
        remaining: usize,
    }
    impl io::Write for DigestWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.remaining {
                return Err(io::Error::other("task secret limit"));
            }
            self.remaining -= bytes.len();
            self.hash.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = DigestWriter {
        hash: Sha256::new(),
        remaining: MAX_SECRET_BYTES,
    };
    serde_json::to_writer(&mut writer, secret).map_err(|_| SnapshotError::TooLarge)?;
    Ok(writer.hash.finalize().into())
}
