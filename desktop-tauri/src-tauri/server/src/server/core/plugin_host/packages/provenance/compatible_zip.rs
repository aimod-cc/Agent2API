//! 普通安装的安全解释层；保留Unicode、中央ZIP64和前缀归档的既有安装能力，不签发见证。
//! ZIP库会合并重名项，故先按原始中央目录计数/排重，再逐条核对库实际选择的本地头。

use std::collections::{BTreeMap, BTreeSet};

use super::zip_layout::{insert_path, part, path, u16_at, u32_at, u64_at, Entry};
use super::{MAX_FILES, MAX_TOTAL_BYTES};

const INVALID: &str = "插件ZIP存在重复覆盖、路径别名或不一致的文件头";

struct Directory { at: usize, entries: usize, size: usize, zip_end: usize }

fn directory(bytes: &[u8]) -> Result<Directory, String> {
    if bytes.len() < 22 { return Err(INVALID.into()); }
    let mut result = None;
    for end in (bytes.len().saturating_sub(65_557)..=bytes.len() - 22).rev() {
        if part(bytes, end, 4)? != b"PK\x05\x06"
            || end + 22 + usize::from(u16_at(bytes, end + 20)?) != bytes.len() { continue; }
        if result.is_some() { return Err(INVALID.into()); }
        if u16_at(bytes, end + 4)? != 0 || u16_at(bytes, end + 6)? != 0 { return Err(INVALID.into()); }
        let count = u16_at(bytes, end + 10)?;
        let count_disk = u16_at(bytes, end + 8)?;
        let size = u32_at(bytes, end + 12)?;
        let offset = u32_at(bytes, end + 16)?;
        let zip64 = end >= 20 && part(bytes, end - 20, 4)? == b"PK\x06\x07";
        let (entries, length, at) = if zip64 {
            let locator = end - 20;
            if u32_at(bytes, locator + 4)? != 0 || u32_at(bytes, locator + 16)? != 1 { return Err(INVALID.into()); }
            let at = usize::try_from(u64_at(bytes, locator + 8)?).map_err(|_| INVALID)?;
            if part(bytes, at, 4)? != b"PK\x06\x06" { return Err(INVALID.into()); }
            let length = usize::try_from(u64_at(bytes, at + 4)?).map_err(|_| INVALID)?;
            if length < 44 || at.checked_add(12).and_then(|at| at.checked_add(length)) != Some(locator)
                || u32_at(bytes, at + 16)? != 0 || u32_at(bytes, at + 20)? != 0
                || u64_at(bytes, at + 24)? != u64_at(bytes, at + 32)?
            { return Err(INVALID.into()); }
            let entries = usize::try_from(u64_at(bytes, at + 32)?).map_err(|_| INVALID)?;
            let length = usize::try_from(u64_at(bytes, at + 40)?).map_err(|_| INVALID)?;
            let start = usize::try_from(u64_at(bytes, at + 48)?).map_err(|_| INVALID)?;
            if start.checked_add(length) != Some(at) { return Err(INVALID.into()); }
            (entries, length, start)
        } else {
            if count != count_disk || count == u16::MAX || size == u32::MAX || offset == u32::MAX { return Err(INVALID.into()); }
            let length = size as usize;
            let start = end.checked_sub(length).ok_or(INVALID)?;
            if start < offset as usize { return Err(INVALID.into()); }
            (usize::from(count), length, start)
        };
        if entries == 0 || entries > MAX_FILES { return Err(INVALID.into()); }
        result = Some(Directory { at, entries, size: length, zip_end: end + 22 + usize::from(u16_at(bytes, end + 20)?) });
    }
    result.ok_or_else(|| INVALID.into())
}

fn central_count(bytes: &[u8], directory: &Directory) -> Result<Vec<usize>, String> {
    let mut at = directory.at;
    let end = at.checked_add(directory.size).ok_or(INVALID)?;
    let mut raw_names = BTreeSet::new();
    let mut positions = Vec::with_capacity(directory.entries);
    for _ in 0..directory.entries {
        let header = part(bytes, at, 46)?;
        if &header[..4] != b"PK\x01\x02" { return Err(INVALID.into()); }
        let names = usize::from(u16_at(header, 28)?);
        let extra = usize::from(u16_at(header, 30)?);
        let comment = usize::from(u16_at(header, 32)?);
        if names == 0 || names > 4096 || !raw_names.insert(part(bytes, at + 46, names)?.to_vec()) { return Err(INVALID.into()); }
        let next = at.checked_add(46 + names + extra + comment).ok_or(INVALID)?;
        if next > end { return Err(INVALID.into()); }
        positions.push(at);
        at = next;
    }
    if at != end { return Err(INVALID.into()); }
    Ok(positions)
}

fn unicode_path(bytes: &[u8]) -> Result<Option<&[u8]>, String> {
    let mut at = 0;
    let mut tags = BTreeSet::new();
    let mut path = None;
    while at < bytes.len() {
        let tag = u16_at(bytes, at)?;
        let length = usize::from(u16_at(bytes, at + 2)?);
        at += 4;
        let value = part(bytes, at, length)?;
        if !tags.insert(tag) { return Err(INVALID.into()); }
        if tag == 0x7075 {
            if length < 5 || value[0] != 1 || std::str::from_utf8(&value[5..]).is_err() { return Err(INVALID.into()); }
            path = Some(&value[5..]);
        }
        if tag == 0x9901 { return Err(INVALID.into()); }
        at += length;
    }
    Ok(path)
}

fn local_sizes(bytes: &[u8], size: u32, compressed: u32) -> Result<(u64, u64), String> {
    let mut at = 0;
    let mut extended = None;
    while at < bytes.len() {
        let tag = u16_at(bytes, at)?;
        let length = usize::from(u16_at(bytes, at + 2)?);
        at += 4;
        let value = part(bytes, at, length)?;
        if tag == 1 {
            if extended.replace(value).is_some() { return Err(INVALID.into()); }
        }
        at += length;
    }
    let mut cursor = 0;
    let mut take = || -> Result<u64, String> {
        let value = u64_at(extended.ok_or(INVALID)?, cursor)?;
        cursor += 8;
        Ok(value)
    };
    let size = if size == u32::MAX { take()? } else { u64::from(size) };
    let compressed = if compressed == u32::MAX { take()? } else { u64::from(compressed) };
    Ok((size, compressed))
}

pub(super) fn inspect(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let directory = directory(bytes)?;
    let positions = central_count(bytes, &directory)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..directory.zip_end])).map_err(|_| INVALID)?;
    if archive.len() != positions.len() || archive.central_directory_start() != directory.at as u64 { return Err(INVALID.into()); }
    let mut result = Vec::with_capacity(positions.len());
    let mut nodes = BTreeMap::new();
    let mut ranges = Vec::with_capacity(positions.len());
    let mut total = 0u64;
    for (index, at) in positions.into_iter().enumerate() {
        let file = archive.by_index_raw(index).map_err(|_| INVALID)?;
        let central = part(bytes, at, 46)?;
        let flags = u16_at(central, 8)?;
        let name_size = usize::from(u16_at(central, 28)?);
        let original = part(bytes, at + 46, name_size)?;
        if flags & 0x0800 != 0 && std::str::from_utf8(original).is_err() { return Err(INVALID.into()); }
        let local = usize::try_from(file.header_start()).map_err(|_| INVALID)?;
        let header = part(bytes, local, 30)?;
        let local_name = usize::from(u16_at(header, 26)?);
        let local_extra = usize::from(u16_at(header, 28)?);
        if &header[..4] != b"PK\x03\x04" || local_name != name_size
            || part(bytes, local + 30, local_name)? != original
            || u16_at(header, 4)? != u16_at(central, 6)? || u16_at(central, 34)? != 0
            || u16_at(header, 6)? != flags || u16_at(header, 8)? != u16_at(central, 10)?
            || file.central_header_start() != at as u64 || file.encrypted() || file.is_symlink()
            || flags & !(0x0800 | 0x0008 | 0x0006) != 0
        { return Err(INVALID.into()); }
        let name = file.name();
        let directory_entry = file.is_dir();
        let central_extra = usize::from(u16_at(central, 30)?);
        if unicode_path(part(bytes, at + 46 + name_size, central_extra)?)?.is_some_and(|value| value != name.as_bytes())
            || unicode_path(part(bytes, local + 30 + local_name, local_extra)?)?.is_some_and(|value| value != name.as_bytes())
        { return Err(INVALID.into()); }
        path(name, directory_entry)?;
        insert_path(&mut nodes, name, directory_entry)?;
        if file.unix_mode().is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o040000 | 0o100000))
            || file.size() > MAX_TOTAL_BYTES || directory_entry && file.size() != 0
        { return Err(INVALID.into()); }
        total = total.checked_add(file.size()).ok_or(INVALID)?;
        if total > MAX_TOTAL_BYTES { return Err(INVALID.into()); }
        let local_crc = u32_at(header, 14)?;
        let local_size = u32_at(header, 22)?;
        let local_compressed = u32_at(header, 18)?;
        let zip64_sizes = local_size == u32::MAX || local_compressed == u32::MAX;
        let (local_size, local_compressed) = local_sizes(part(bytes, local + 30 + local_name, local_extra)?, local_size, local_compressed)?;
        if flags & 8 == 0 && (local_crc != file.crc32() || local_size != file.size() || local_compressed != file.compressed_size())
            || flags & 8 != 0 && (local_crc != 0 && local_crc != file.crc32()
                || local_size != 0 && local_size != file.size() || local_compressed != 0 && local_compressed != file.compressed_size())
        { return Err(INVALID.into()); }
        let start = local.checked_add(30 + local_name + local_extra).ok_or(INVALID)?;
        let end = start.checked_add(usize::try_from(file.compressed_size()).map_err(|_| INVALID)?).ok_or(INVALID)?;
        let range_end = if flags & 8 != 0 {
            let at = if part(bytes, end, 4)? == b"PK\x07\x08" { end + 4 } else { end };
            let valid = if zip64_sizes {
                u64_at(bytes, at + 4)? == file.compressed_size() && u64_at(bytes, at + 12)? == file.size()
            } else {
                u64::from(u32_at(bytes, at + 4)?) == file.compressed_size() && u64::from(u32_at(bytes, at + 8)?) == file.size()
            };
            if u32_at(bytes, at)? != file.crc32() || !valid { return Err(INVALID.into()); }
            at + if zip64_sizes { 20 } else { 12 }
        } else { end };
        if file.data_start() != start as u64 || range_end > directory.at { return Err(INVALID.into()); }
        ranges.push((local, range_end));
        result.push(Entry { name: name.to_string(), directory: directory_entry, size: file.size(), compressed: file.compressed_size(),
            crc: file.crc32(), header: local as u64, central: at as u64 });
    }
    ranges.sort_unstable();
    for pair in ranges.windows(2) { if pair[0].1 > pair[1].0 { return Err(INVALID.into()); } }
    Ok(result)
}
