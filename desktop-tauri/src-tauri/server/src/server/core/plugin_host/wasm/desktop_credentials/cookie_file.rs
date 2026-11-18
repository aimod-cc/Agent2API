use std::io::Write;

use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use zeroize::Zeroizing;

use super::{invalid, kuku};

const MAX_BYTES: usize = 32 * 1024 * 1024;

pub(in crate::server::core::plugin_host::wasm) fn prepare(params: &Value) -> Result<bytes::Bytes, proto::WireError> {
    let source = &params["source"];
    let encoded = source["content"].as_str().filter(|value| !value.is_empty()
        && value.len() <= MAX_BYTES.div_ceil(3) * 4).ok_or_else(invalid)?;
    let raw = Zeroizing::new(STANDARD.decode(encoded).map_err(|_| invalid())?);
    if raw.len() > MAX_BYTES || !raw.starts_with(b"SQLite format 3\0") { return Err(invalid()); }
    Ok(bytes::Bytes::from_owner(raw))
}

pub(in crate::server::core::plugin_host::wasm) fn parse(raw: bytes::Bytes) -> Result<Value, proto::WireError> {
    if raw.len() > MAX_BYTES || !raw.starts_with(b"SQLite format 3\0") { return Err(invalid()); }
    let mut file = tempfile::NamedTempFile::new().map_err(|_| invalid())?;
    file.write_all(&raw).and_then(|_| file.as_file().sync_all()).map_err(|_| invalid())?;
    let db = rusqlite::Connection::open_with_flags(file.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| invalid())?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;").map_err(|_| invalid())?;
    let view = kuku::parse(&db);
    drop(db);
    file.close().map_err(|_| invalid())?;
    view
}
