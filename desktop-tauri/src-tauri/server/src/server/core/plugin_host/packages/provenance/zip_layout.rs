//! 先检查原始中央目录和本地头，再交给ZIP库；库自身按文件名去重，不能据其len排重。

use std::collections::{BTreeMap, BTreeSet};

use super::{MAX_FILES, MAX_TOTAL_BYTES};

const INVALID: &str = "插件ZIP存在重复、路径歧义或不一致的目录信息";
const MAX_PATH: usize = 512;
const MAX_COMPONENTS: usize = 32;

pub(super) struct Entry {
    pub name: String,
    pub directory: bool,
    pub size: u64,
    pub compressed: u64,
    pub crc: u32,
    pub header: u64,
    pub central: u64,
}

pub(super) fn part(bytes: &[u8], at: usize, length: usize) -> Result<&[u8], String> {
    bytes.get(at..at.checked_add(length).ok_or(INVALID)?).ok_or_else(|| INVALID.into())
}
pub(super) fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    let value: [u8; 2] = part(bytes, at, 2)?.try_into().map_err(|_| INVALID)?;
    Ok(u16::from_le_bytes(value))
}
pub(super) fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    let value: [u8; 4] = part(bytes, at, 4)?.try_into().map_err(|_| INVALID)?;
    Ok(u32::from_le_bytes(value))
}
pub(super) fn u64_at(bytes: &[u8], at: usize) -> Result<u64, String> {
    let value: [u8; 8] = part(bytes, at, 8)?.try_into().map_err(|_| INVALID)?;
    Ok(u64::from_le_bytes(value))
}

pub(super) fn path(name: &str, directory: bool) -> Result<&str, String> {
    let name = if directory { name.strip_suffix('/').ok_or(INVALID)? } else { name };
    if name.is_empty() || name.len() > MAX_PATH
        || name.chars().any(|value| value.is_control())
        || name.bytes().any(|byte| b"\\:*?\"<>|".contains(&byte))
    { return Err(INVALID.into()); }
    let mut count = 0;
    for component in name.split('/') {
        count += 1;
        if count > MAX_COMPONENTS || component.is_empty() || component.len() > 240
            || matches!(component, "." | "..") || component.ends_with(['.', ' '])
        { return Err(INVALID.into()); }
        if count == 1 {
            for fixed in ["pack.json", "wasm.json", "host.json", "ui"] {
                if component.eq_ignore_ascii_case(fixed) && component != fixed { return Err(INVALID.into()); }
            }
        }
        let stem = component.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            || stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9')
        { return Err(INVALID.into()); }
    }
    Ok(name)
}

/// 同名目录可由子路径隐式出现一次，再有一个显式目录条目；大小写别名一律拒绝。
pub(super) fn insert_path(nodes: &mut BTreeMap<String, (String, bool, bool)>, name: &str, dir: bool) -> Result<(), String> {
    let clean = path(name, dir)?;
    let parts: Vec<_> = clean.split('/').collect();
    let mut prefix = String::new();
    for (index, component) in parts.iter().enumerate() {
        if !prefix.is_empty() { prefix.push('/'); }
        prefix.push_str(component);
        let last = index + 1 == parts.len();
        let is_dir = !last || dir;
        let key = prefix.to_lowercase();
        if let Some((original, old_dir, explicit)) = nodes.get_mut(&key) {
            if *original != prefix || !*old_dir || !is_dir || last && *explicit { return Err(INVALID.into()); }
            if last { *explicit = true; }
        } else {
            if nodes.len() >= MAX_FILES * MAX_COMPONENTS { return Err(INVALID.into()); }
            nodes.insert(key, (prefix.clone(), is_dir, last));
        }
    }
    Ok(())
}

fn extras(bytes: &[u8], name: &[u8], local_sizes: Option<(u64, u64)>) -> Result<(), String> {
    let mut at = 0;
    let mut tags = BTreeSet::new();
    let mut found_zip64 = false;
    while at < bytes.len() {
        let tag = u16_at(bytes, at)?;
        let length = usize::from(u16_at(bytes, at + 2)?);
        at += 4;
        let value = part(bytes, at, length)?;
        if !tags.insert(tag) { return Err(INVALID.into()); }
        match tag {
            // 小归档不使用中央ZIP64；兼容写入器对小文件本地头强制ZIP64的明确尺寸。
            0x0001 => {
                found_zip64 = true;
                let (size, compressed) = local_sizes.ok_or(INVALID)?;
                if length != 16 || u64_at(value, 0)? != size || u64_at(value, 8)? != compressed {
                    return Err(INVALID.into());
                }
            }
            0x7075 if value.len() < 5 || value[0] != 1 || &value[5..] != name => return Err(INVALID.into()),
            0x9901 => return Err(INVALID.into()),
            _ => {}
        }
        at += length;
    }
    if local_sizes.is_some() && !found_zip64 { return Err(INVALID.into()); }
    Ok(())
}

pub(super) fn inspect(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    if bytes.len() < 22 { return Err(INVALID.into()); }
    let mut end = None;
    for at in (bytes.len().saturating_sub(65_557)..=bytes.len() - 22).rev() {
        if part(bytes, at, 4)? == b"PK\x05\x06"
            && at + 22 + usize::from(u16_at(bytes, at + 20)?) == bytes.len()
        {
            if end.replace(at).is_some() { return Err(INVALID.into()); }
        }
    }
    let end = end.ok_or(INVALID)?;
    let count = usize::from(u16_at(bytes, end + 10)?);
    let central_size = u32_at(bytes, end + 12)? as usize;
    let start = u32_at(bytes, end + 16)? as usize;
    if count == 0 || count > MAX_FILES || u16_at(bytes, end + 4)? != 0 || u16_at(bytes, end + 6)? != 0
        || usize::from(u16_at(bytes, end + 8)?) != count
        || start.checked_add(central_size) != Some(end)
    { return Err(INVALID.into()); }
    let mut entries = Vec::with_capacity(count);
    let mut ranges = Vec::with_capacity(count);
    let mut nodes = BTreeMap::new();
    let mut at = start;
    let mut total = 0u64;
    for _ in 0..count {
        let header = part(bytes, at, 46)?;
        if &header[..4] != b"PK\x01\x02" { return Err(INVALID.into()); }
        let flags = u16_at(header, 8)?;
        let method = u16_at(header, 10)?;
        let crc = u32_at(header, 16)?;
        let compressed = u32_at(header, 20)?;
        let size = u32_at(header, 24)?;
        let names = usize::from(u16_at(header, 28)?);
        let extra = usize::from(u16_at(header, 30)?);
        let comment = usize::from(u16_at(header, 32)?);
        let local = u32_at(header, 42)? as usize;
        let next = at.checked_add(46 + names + extra + comment).ok_or(INVALID)?;
        if next > end || flags & !(0x0800 | 0x0008 | 0x0006) != 0 || !matches!(method, 0 | 8)
            || u16_at(header, 34)? != 0 || u16_at(header, 6)? > 45
            || compressed == u32::MAX || size == u32::MAX || local >= start
        { return Err(INVALID.into()); }
        let raw_name = part(bytes, at + 46, names)?;
        let name = std::str::from_utf8(raw_name).map_err(|_| INVALID)?;
        if !name.is_ascii() || name.contains('~') { return Err(INVALID.into()); }
        let directory = name.ends_with('/');
        insert_path(&mut nodes, name, directory)?;
        let kind = (u32_at(header, 38)? >> 16) & 0o170000;
        if !matches!(kind, 0 | 0o040000 | 0o100000)
            || kind == 0o040000 && !directory || kind == 0o100000 && directory
            || directory && size != 0
        { return Err(INVALID.into()); }
        total = total.checked_add(u64::from(size)).ok_or(INVALID)?;
        if total > MAX_TOTAL_BYTES || u64::from(size) > MAX_TOTAL_BYTES { return Err(INVALID.into()); }
        extras(part(bytes, at + 46 + names, extra)?, raw_name, None)?;
        let local_header = part(bytes, local, 30)?;
        let local_names = usize::from(u16_at(local_header, 26)?);
        let local_extra = usize::from(u16_at(local_header, 28)?);
        if &local_header[..4] != b"PK\x03\x04" || u16_at(local_header, 4)? != u16_at(header, 6)?
            || u16_at(local_header, 6)? != flags || u16_at(local_header, 8)? != method || local_names != names
            || part(bytes, local + 30, local_names)? != raw_name
        { return Err(INVALID.into()); }
        let local_compressed = u32_at(local_header, 18)?;
        let local_size = u32_at(local_header, 22)?;
        let zip64 = local_compressed == u32::MAX && local_size == u32::MAX;
        extras(part(bytes, local + 30 + local_names, local_extra)?, raw_name,
            zip64.then_some((u64::from(size), u64::from(compressed))))?;
        if flags & 8 == 0 && (u32_at(local_header, 14)? != crc
            || !zip64 && (local_size != size || local_compressed != compressed))
        { return Err(INVALID.into()); }
        if flags & 8 != 0 && (u32_at(local_header, 14)? != 0 && u32_at(local_header, 14)? != crc
            || !zip64 && (local_size != 0 && local_size != size || local_compressed != 0 && local_compressed != compressed))
        { return Err(INVALID.into()); }
        let data_end = local.checked_add(30 + local_names + local_extra)
            .and_then(|value| value.checked_add(compressed as usize)).ok_or(INVALID)?;
        let mut range_end = data_end;
        if flags & 8 != 0 {
            let descriptor = if part(bytes, data_end, 4)? == b"PK\x07\x08" { data_end + 4 } else { data_end };
            let sizes_match = if zip64 {
                range_end = descriptor + 20;
                u64_at(bytes, descriptor + 4)? == u64::from(compressed) && u64_at(bytes, descriptor + 12)? == u64::from(size)
            } else {
                range_end = descriptor + 12;
                u32_at(bytes, descriptor + 4)? == compressed && u32_at(bytes, descriptor + 8)? == size
            };
            if u32_at(bytes, descriptor)? != crc || !sizes_match { return Err(INVALID.into()); }
        }
        if range_end > start { return Err(INVALID.into()); }
        ranges.push((local, range_end));
        entries.push(Entry { name: name.to_string(), directory, size: u64::from(size), compressed: u64::from(compressed),
            crc, header: local as u64, central: at as u64 });
        at = next;
    }
    if at != end { return Err(INVALID.into()); }
    ranges.sort_unstable();
    let mut cursor = 0;
    for (begin, finish) in ranges {
        if begin != cursor { return Err(INVALID.into()); }
        cursor = finish;
    }
    if cursor != start { return Err(INVALID.into()); }
    Ok(entries)
}
