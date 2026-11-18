//! 只读 Loomy 官方客户端的一次登录快照，不开放路径、数据库或系统浏览器。
//! 0.9.38 使用当前用户的 userData/auth-session.json；0.9.39 使用本家 Local Storage。
//! 日志只能在 CURRENT/清单/CRC/序号证明当前值时读取，墓碑、损坏或表内旧值不回落。
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{absolute_env, content, file, home, invalid, regular};

type Result<T> = std::result::Result<T, proto::WireError>;
const MAX_FILE: u64 = 8 * 1024 * 1024;
const MAX_VALUE: usize = 256 * 1024;
const MAX_SEQUENCE: u64 = (1u64 << 56) - 1;
const KEY: &str = "loomy-auth-session";

type Stamp = (u64, Option<SystemTime>);

fn stamp(path: &Path) -> Result<Stamp> {
    let meta = regular(path)?;
    Ok((meta.len(), meta.modified().ok()))
}

fn stable(
    path: &Path,
    maximum: u64,
    deadline: Instant,
    watched: &mut Vec<(PathBuf, Stamp)>,
) -> Result<Zeroizing<Vec<u8>>> {
    if Instant::now() >= deadline {
        return Err(invalid());
    }
    let before = stamp(path)?;
    let bytes = if before.0 == 0 {
        Zeroizing::new(Vec::new())
    } else {
        Zeroizing::new(file(path, maximum)?)
    };
    if stamp(path)? != before {
        return Err(invalid());
    }
    watched.push((path.to_path_buf(), before));
    Ok(bytes)
}

fn exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(invalid()),
    }
}

fn directory(path: &Path) -> Result<()> {
    let mut current = Some(path);
    while let Some(path) = current {
        let meta = std::fs::symlink_metadata(path).map_err(|_| invalid())?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(invalid());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(invalid());
            }
        }
        current = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
    }
    Ok(())
}

fn app_root() -> Result<PathBuf> {
    #[cfg(windows)]
    return Ok(match absolute_env("APPDATA") {
        Some(root) => root,
        None => home()?.join("AppData/Roaming"),
    }
    .join("Loomy"));
    #[cfg(target_os = "macos")]
    {
        let root = home()?.join("Library/Application Support");
        if exists(&root.join("loomy"))? {
            return Ok(root.join("loomy"));
        }
        return Ok(root.join("Loomy"));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    return Ok(match absolute_env("XDG_CONFIG_HOME") {
        Some(root) => root,
        None => home()?.join(".config"),
    }
    .join("Loomy"));
}

fn scalar(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| match &value[*key] {
            Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
            Value::Number(value) if value.is_i64() || value.is_u64() => Some(value.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn project(value: &Value) -> Result<Value> {
    let session = scalar(value, &["session"]);
    let uid = scalar(value, &["userid", "userId", "user_id"]);
    let phone = scalar(value, &["phone"]);
    if session.is_empty()
        || session.len() > 8192
        || !session.is_ascii()
        || session
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
        || uid.is_empty() && phone.is_empty()
        || uid.len() > 256
        || phone.len() > 256
        || uid.chars().any(char::is_control)
        || phone.chars().any(char::is_control)
        || uid.contains(&session)
        || phone.contains(&session)
    {
        return Err(invalid());
    }
    Ok(json!({ "session": session, "userId": uid, "phone": phone }))
}

fn decode_json(bytes: &[u8]) -> Result<Value> {
    super::super::managed_task::declaration::parse_object(bytes, MAX_VALUE).map_err(|_| invalid())
}

fn sidecar(root: &Path, deadline: Instant) -> Result<Value> {
    let mut candidates = vec![
        root.join("auth-session.json"),
        root.join("userData/auth-session.json"),
    ];
    #[cfg(windows)]
    {
        // 不枚举 C:/Users/Public 中其他用户的目录，只派生当前用户官方固定摘要路径。
        let username = std::env::var("USERNAME")
            .ok()
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 256
                    && !name.contains(['/', '\\'])
                    && !name.chars().any(char::is_control)
            })
            .or_else(|| home().ok()?.file_name()?.to_str().map(str::to_string))
            .ok_or_else(invalid)?;
        let user = format!("{:x}", Sha256::digest(username.as_bytes()));
        let public = absolute_env("PUBLIC").unwrap_or_else(|| PathBuf::from("C:/Users/Public"));
        candidates.push(
            public
                .join("Loomy")
                .join(&user[..12])
                .join("userData/auth-session.json"),
        );
    }
    for path in candidates {
        if !exists(&path)? {
            continue;
        }
        let mut watched = Vec::new();
        let bytes = stable(&path, MAX_VALUE as u64, deadline, &mut watched)?;
        let value = project(&decode_json(&bytes)?)?;
        verify(&watched, deadline)?;
        return content(&path, value);
    }
    Err(invalid())
}

fn verify(watched: &[(PathBuf, Stamp)], deadline: Instant) -> Result<()> {
    for (path, before) in watched {
        if Instant::now() >= deadline || stamp(path)? != *before {
            return Err(invalid());
        }
    }
    Ok(())
}

fn varint(bytes: &[u8], offset: &mut usize) -> Result<u64> {
    let mut result = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*offset).ok_or_else(invalid)?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            return Err(invalid());
        }
        result |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(result);
        }
    }
    Err(invalid())
}

fn slice<'a>(bytes: &'a [u8], offset: &mut usize) -> Result<&'a [u8]> {
    let length = usize::try_from(varint(bytes, offset)?).map_err(|_| invalid())?;
    if length > MAX_VALUE {
        return Err(invalid());
    }
    let end = offset.checked_add(length).ok_or_else(invalid)?;
    let value = bytes.get(*offset..end).ok_or_else(invalid)?;
    *offset = end;
    Ok(value)
}

fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        let mut crc = index as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0x82f63b78 } else { 0 };
        }
        *slot = crc;
    }
    table
}

/// 官方 LevelDB 32KiB/7字节头的物理记录，严格 CRC32C Mask 与碎片完整性。
fn records(
    bytes: &[u8],
    deadline: Instant,
    visit: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let table = crc_table();
    let mut fragments = Zeroizing::new(Vec::new());
    let mut open = false;
    for block in bytes.chunks(32768) {
        if Instant::now() >= deadline {
            return Err(invalid());
        }
        let mut offset = 0;
        while offset < block.len() {
            let remaining = &block[offset..];
            if remaining.len() < 7 {
                if block.len() != 32768 || remaining.iter().any(|byte| *byte != 0) {
                    return Err(invalid());
                }
                break;
            }
            let checksum = u32::from_le_bytes(remaining[..4].try_into().map_err(|_| invalid())?);
            let length = usize::from(u16::from_le_bytes([remaining[4], remaining[5]]));
            let kind = remaining[6];
            if kind == 0 && length == 0 && checksum == 0 {
                if open || remaining.iter().any(|byte| *byte != 0) {
                    return Err(invalid());
                }
                break;
            }
            let payload = remaining.get(7..7 + length).ok_or_else(invalid)?;
            let mut crc = !0u32;
            for byte in std::iter::once(&kind).chain(payload) {
                crc = table[((crc ^ u32::from(*byte)) & 255) as usize] ^ (crc >> 8);
            }
            let masked = (!crc).rotate_right(15).wrapping_add(0xa282ead8);
            if checksum != masked {
                return Err(invalid());
            }
            match kind {
                1 if !open => visit(payload)?,
                2 if !open => {
                    fragments.clear();
                    fragments.extend_from_slice(payload);
                    open = true;
                }
                3 | 4 if open => {
                    if fragments.len().saturating_add(payload.len()) > MAX_VALUE {
                        return Err(invalid());
                    }
                    fragments.extend_from_slice(payload);
                    if kind == 4 {
                        visit(&fragments)?;
                        fragments.clear();
                        open = false;
                    }
                }
                _ => return Err(invalid()),
            }
            offset += 7 + length;
        }
    }
    if open {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Default)]
struct Version {
    log: Option<u64>,
    previous: u64,
    sequence: Option<u64>,
    tables: BTreeMap<u64, u64>,
}

fn version(bytes: &[u8], deadline: Instant) -> Result<Version> {
    let mut current = Version::default();
    records(bytes, deadline, &mut |record| {
        let mut offset = 0;
        while offset < record.len() {
            match varint(record, &mut offset)? {
                1 => {
                    if slice(record, &mut offset)? != b"leveldb.BytewiseComparator" {
                        return Err(invalid());
                    }
                }
                2 => current.log = Some(varint(record, &mut offset)?),
                3 => {
                    varint(record, &mut offset)?;
                }
                4 => {
                    let sequence = varint(record, &mut offset)?;
                    if sequence > MAX_SEQUENCE || current.sequence.is_some_and(|old| old > sequence)
                    {
                        return Err(invalid());
                    }
                    current.sequence = Some(sequence);
                }
                5 => {
                    varint(record, &mut offset)?;
                    slice(record, &mut offset)?;
                }
                6 => {
                    varint(record, &mut offset)?;
                    current.tables.remove(&varint(record, &mut offset)?);
                }
                7 => {
                    varint(record, &mut offset)?;
                    let number = varint(record, &mut offset)?;
                    let size = varint(record, &mut offset)?;
                    if number == 0 || size > MAX_FILE || current.tables.len() >= 64 {
                        return Err(invalid());
                    }
                    let smallest = slice(record, &mut offset)?;
                    let largest = slice(record, &mut offset)?;
                    if smallest.len() < 8 || largest.len() < 8 {
                        return Err(invalid());
                    }
                    current.tables.insert(number, size);
                }
                9 => current.previous = varint(record, &mut offset)?,
                _ => return Err(invalid()),
            }
        }
        Ok(())
    })?;
    if current.log.is_none_or(|value| value == 0) || current.sequence.is_none() {
        return Err(invalid());
    }
    Ok(current)
}

fn encoded_text(bytes: &[u8]) -> Result<String> {
    match bytes.first() {
        Some(1) => Ok(bytes[1..].iter().map(|byte| char::from(*byte)).collect()),
        Some(0) if (bytes.len() - 1) % 2 == 0 => String::from_utf16(
            &bytes[1..]
                .chunks_exact(2)
                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| invalid()),
        _ => Err(invalid()),
    }
}

fn session_key(key: &[u8]) -> Result<bool> {
    let Some(separator) = key.iter().position(|byte| *byte == 0) else {
        return Ok(false);
    };
    // 正式 Electron 本机 origin；不从相同目录内的网页第三方存储认用户凭据。
    if &key[..separator] != b"_file://" {
        return Ok(false);
    }
    Ok(encoded_text(&key[separator + 1..])? == KEY)
}

struct Latest {
    sequence: u64,
    value: Option<Zeroizing<Vec<u8>>>,
    key: Zeroizing<Vec<u8>>,
}

fn choose(
    latest: &mut Option<Latest>,
    key: &[u8],
    value: Option<&[u8]>,
    sequence: u64,
) -> Result<()> {
    if !session_key(key)? {
        return Ok(());
    }
    if latest.as_ref().is_some_and(|old| old.key.as_slice() != key) {
        return Err(invalid());
    }
    if let Some(old) = latest.as_ref().filter(|old| old.sequence == sequence) {
        if old.value.as_deref().map(Vec::as_slice) != value {
            return Err(invalid());
        }
        return Ok(());
    }
    if latest.as_ref().is_none_or(|old| old.sequence < sequence) {
        *latest = Some(Latest {
            sequence,
            key: Zeroizing::new(key.to_vec()),
            value: value.map(|value| Zeroizing::new(value.to_vec())),
        });
    }
    Ok(())
}

fn batches(
    bytes: &[u8],
    deadline: Instant,
    last: &mut u64,
    latest: &mut Option<Latest>,
) -> Result<()> {
    records(bytes, deadline, &mut |record| {
        if record.len() < 12 {
            return Err(invalid());
        }
        let start = u64::from_le_bytes(record[..8].try_into().map_err(|_| invalid())?);
        let count = u32::from_le_bytes(record[8..12].try_into().map_err(|_| invalid())?);
        if count > 65536 || start > MAX_SEQUENCE {
            return Err(invalid());
        }
        let end = start
            .checked_add(u64::from(count.saturating_sub(1)))
            .ok_or_else(invalid)?;
        if end > MAX_SEQUENCE || count > 0 && start <= *last {
            return Err(invalid());
        }
        let mut offset = 12;
        for index in 0..count {
            let kind = *record.get(offset).ok_or_else(invalid)?;
            offset += 1;
            let key = slice(record, &mut offset)?;
            let value = match kind {
                0 => None,
                1 => Some(slice(record, &mut offset)?),
                _ => return Err(invalid()),
            };
            choose(latest, key, value, start + u64::from(index))?;
        }
        if offset != record.len() {
            return Err(invalid());
        }
        if count > 0 {
            *last = end;
        }
        Ok(())
    })
}

fn logs(root: &Path, log: u64, previous: u64) -> Result<BTreeMap<u64, PathBuf>> {
    directory(root)?;
    let mut logs = BTreeMap::new();
    let entries = std::fs::read_dir(root).map_err(|_| invalid())?;
    for (index, entry) in entries.enumerate() {
        if index >= 1024 {
            return Err(invalid());
        }
        let entry = entry.map_err(|_| invalid())?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(invalid)?;
        if let Some(number) = name.strip_suffix(".log").filter(|number| {
            !number.is_empty()
                && number.len() <= 20
                && number.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            let number = number.parse::<u64>().map_err(|_| invalid())?;
            if number >= log || previous != 0 && number == previous {
                if logs.len() >= 64 || logs.insert(number, entry.path()).is_some() {
                    return Err(invalid());
                }
            }
        }
    }
    if !logs.contains_key(&log) || previous != 0 && !logs.contains_key(&previous) {
        return Err(invalid());
    }
    Ok(logs)
}

fn decompress(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let mut offset = 0;
    let size = usize::try_from(varint(bytes, &mut offset)?).map_err(|_| invalid())?;
    if size > MAX_VALUE {
        return Err(invalid());
    }
    let mut output = Zeroizing::new(Vec::with_capacity(size));
    while offset < bytes.len() {
        let tag = bytes[offset];
        offset += 1;
        if tag & 3 == 0 {
            let mut length = usize::from(tag >> 2);
            if length >= 60 {
                let count = length - 59;
                let encoded = bytes.get(offset..offset + count).ok_or_else(invalid)?;
                length = encoded.iter().enumerate().fold(0usize, |n, (shift, byte)| {
                    n | (usize::from(*byte) << (shift * 8))
                });
                offset += count;
            }
            length = length.checked_add(1).ok_or_else(invalid)?;
            if output.len().saturating_add(length) > size {
                return Err(invalid());
            }
            output.extend_from_slice(bytes.get(offset..offset + length).ok_or_else(invalid)?);
            offset += length;
        } else {
            let (length, distance) = match tag & 3 {
                1 => {
                    let low = *bytes.get(offset).ok_or_else(invalid)?;
                    offset += 1;
                    (
                        usize::from((tag >> 2) & 7) + 4,
                        (usize::from(tag & 0xe0) << 3) | usize::from(low),
                    )
                }
                kind => {
                    let count = if kind == 2 { 2 } else { 4 };
                    let encoded = bytes.get(offset..offset + count).ok_or_else(invalid)?;
                    let distance = encoded.iter().enumerate().fold(0usize, |n, (shift, byte)| {
                        n | (usize::from(*byte) << (shift * 8))
                    });
                    offset += count;
                    (usize::from(tag >> 2) + 1, distance)
                }
            };
            if distance == 0
                || distance > output.len()
                || output.len().saturating_add(length) > size
            {
                return Err(invalid());
            }
            for _ in 0..length {
                let byte = output[output.len() - distance];
                output.push(byte);
            }
        }
    }
    if output.len() != size {
        return Err(invalid());
    }
    Ok(output)
}

fn block(bytes: &[u8], offset: u64, size: u64) -> Result<Zeroizing<Vec<u8>>> {
    let offset = usize::try_from(offset).map_err(|_| invalid())?;
    let size = usize::try_from(size).map_err(|_| invalid())?;
    if size > MAX_VALUE {
        return Err(invalid());
    }
    let end = offset.checked_add(size).ok_or_else(invalid)?;
    let payload = bytes.get(offset..end).ok_or_else(invalid)?;
    let trailer = bytes.get(end..end + 5).ok_or_else(invalid)?;
    let table = crc_table();
    let mut crc = !0u32;
    for byte in payload.iter().chain(&trailer[..1]) {
        crc = table[((crc ^ u32::from(*byte)) & 255) as usize] ^ (crc >> 8);
    }
    let masked = (!crc).rotate_right(15).wrapping_add(0xa282ead8);
    if masked != u32::from_le_bytes(trailer[1..].try_into().map_err(|_| invalid())?) {
        return Err(invalid());
    }
    match trailer[0] {
        0 => Ok(Zeroizing::new(payload.to_vec())),
        1 => decompress(payload),
        _ => Err(invalid()),
    }
}

fn entries(bytes: &[u8], visit: &mut impl FnMut(&[u8], &[u8]) -> Result<()>) -> Result<()> {
    if bytes.len() < 4 {
        return Err(invalid());
    }
    let count =
        u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().map_err(|_| invalid())?) as usize;
    let restarts = count
        .checked_add(1)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(invalid)?;
    let end = bytes.len().checked_sub(restarts).ok_or_else(invalid)?;
    if count == 0 || count > 65536 {
        return Err(invalid());
    }
    let mut previous = None;
    for restart in bytes[end..bytes.len() - 4].chunks_exact(4) {
        let restart = u32::from_le_bytes(restart.try_into().map_err(|_| invalid())?) as usize;
        if restart > end || previous.is_some_and(|old| old >= restart) {
            return Err(invalid());
        }
        previous = Some(restart);
    }
    let mut key = Zeroizing::new(Vec::new());
    let mut offset = 0;
    let mut count = 0;
    while offset < end {
        count += 1;
        if count > 65536 {
            return Err(invalid());
        }
        let shared = usize::try_from(varint(&bytes[..end], &mut offset)?).map_err(|_| invalid())?;
        let unshared =
            usize::try_from(varint(&bytes[..end], &mut offset)?).map_err(|_| invalid())?;
        let length = usize::try_from(varint(&bytes[..end], &mut offset)?).map_err(|_| invalid())?;
        if shared > key.len() || shared.saturating_add(unshared) > MAX_VALUE || length > MAX_VALUE {
            return Err(invalid());
        }
        key.truncate(shared);
        key.extend_from_slice(
            bytes
                .get(offset..offset + unshared)
                .filter(|_| offset + unshared <= end)
                .ok_or_else(invalid)?,
        );
        offset += unshared;
        let value = bytes
            .get(offset..offset + length)
            .filter(|_| offset + length <= end)
            .ok_or_else(invalid)?;
        offset += length;
        visit(&key, value)?;
    }
    Ok(())
}

fn handle(bytes: &[u8], offset: &mut usize) -> Result<(u64, u64)> {
    Ok((varint(bytes, offset)?, varint(bytes, offset)?))
}

fn table(bytes: &[u8], deadline: Instant, latest: &mut Option<Latest>) -> Result<()> {
    if bytes.len() < 48 {
        return Err(invalid());
    }
    let footer = &bytes[bytes.len() - 48..];
    if footer[40..] != 0xdb4775248b80fb57u64.to_le_bytes() {
        return Err(invalid());
    }
    let mut offset = 0;
    handle(&footer[..40], &mut offset)?;
    let (index, size) = handle(&footer[..40], &mut offset)?;
    let index = block(bytes, index, size)?;
    let mut seen = std::collections::BTreeSet::new();
    entries(&index, &mut |_, value| {
        if Instant::now() >= deadline {
            return Err(invalid());
        }
        let mut offset = 0;
        let (position, size) = handle(value, &mut offset)?;
        if offset != value.len() || !seen.insert(position) || seen.len() > 4096 {
            return Err(invalid());
        }
        let block = block(bytes, position, size)?;
        entries(&block, &mut |key, value| {
            if key.len() < 8 {
                return Err(invalid());
            }
            let end = key.len() - 8;
            let tag = u64::from_le_bytes(key[end..].try_into().map_err(|_| invalid())?);
            if tag & 255 > 1 || tag >> 8 > MAX_SEQUENCE {
                return Err(invalid());
            }
            choose(
                latest,
                &key[..end],
                if tag & 255 == 0 { None } else { Some(value) },
                tag >> 8,
            )
        })
    })
}

fn from_storage(root: &Path, deadline: Instant) -> Result<Value> {
    let mut watched = Vec::new();
    let current_path = root.join("CURRENT");
    let current = stable(&current_path, 128, deadline, &mut watched)?;
    let name = std::str::from_utf8(&current).map_err(|_| invalid())?;
    let name = name.strip_suffix('\n').ok_or_else(invalid)?;
    let suffix = name.strip_prefix("MANIFEST-").ok_or_else(invalid)?;
    if suffix.is_empty() || suffix.len() > 20 || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let manifest = stable(&root.join(name), MAX_FILE, deadline, &mut watched)?;
    let version = version(&manifest, deadline)?;
    let log = version.log.ok_or_else(invalid)?;
    let files = logs(root, log, version.previous)?;
    let mut total = manifest.len();
    let mut latest = None;
    for (number, size) in &version.tables {
        let modern = root.join(format!("{number:06}.ldb"));
        let legacy = root.join(format!("{number:06}.sst"));
        let path = match (exists(&modern)?, exists(&legacy)?) {
            (true, false) => modern,
            (false, true) => legacy,
            _ => return Err(invalid()),
        };
        let bytes = stable(&path, MAX_FILE, deadline, &mut watched)?;
        if bytes.len() as u64 != *size {
            return Err(invalid());
        }
        total = total.checked_add(bytes.len()).ok_or_else(invalid)?;
        if total > 32 * 1024 * 1024 {
            return Err(invalid());
        }
        table(&bytes, deadline, &mut latest)?;
    }
    let mut sequence = 0;
    for path in files.values() {
        let bytes = stable(path, MAX_FILE, deadline, &mut watched)?;
        total = total.checked_add(bytes.len()).ok_or_else(invalid)?;
        if total > 32 * 1024 * 1024 {
            return Err(invalid());
        }
        batches(&bytes, deadline, &mut sequence, &mut latest)?;
    }
    let latest = latest.ok_or_else(invalid)?;
    // 活跃清单声明的全部SST与待恢复日志均已校验；最高序号的值或墓碑是唯一结果。
    let raw = latest.value.ok_or_else(invalid)?; // 墓碑是已退出登录，绝不回落旧记录。
    let text = Zeroizing::new(encoded_text(&raw)?);
    let value = project(&decode_json(text.as_bytes())?)?;
    verify(&watched, deadline)?;
    if logs(root, log, version.previous)? != files {
        return Err(invalid());
    }
    content(root, value)
}

pub(super) fn read() -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let root = app_root()?;
    let storage = root.join("Local Storage/leveldb");
    if exists(&storage)? {
        return from_storage(&storage, deadline);
    }
    sidecar(&root, deadline)
}
