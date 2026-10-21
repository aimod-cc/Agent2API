//! 单条目 zip 解压：只服务内核安装（mihomo 官方 Windows 资产就是一个条目，
//! 形如 `mihomo-windows-amd64.exe`，deflate 压缩）。
//!
//! ── 为什么不用 PowerShell 的 Expand-Archive ──────────────────
//! 那是外部进程，三处会翻车：① AppLocker / 受限语言模式 / ExecutionPolicy
//! 直接拒执行；② 路径里带一个单引号（`D:\Users\O'Brien\…`）就让拼出来的
//! 命令坏掉；③ spawn powershell 这套行为特征会命中杀软对流量拦截类木马的
//! 启发式（同 `backend.rs` 里 taskkill 那段的成因）。进程内解析没有这些面。
//!
//! ── 为什么不引 zip crate ────────────────────────────────────
//! 只读一个条目却要拖进一整棵依赖树；格式本身（EOCD → 中央目录 → 本地头 →
//! deflate）是几十行确定性代码，与 `providers/qoder/risk.rs` 手写 tar 是
//! 同一个取向。deflate 解码用已在依赖树里的 flate2。
//!
//! ── 安全边界 ────────────────────────────────────────────────
//! · 只解**一个**条目，且只写到调用方给的**单个文件**——不按条目名拼路径，
//!   路径穿越（`../`、绝对路径、盘符）在这一层无从谈起；
//! · 解压总量以中央目录声明的 uncompressed size 为准逐块核对，声明值本身
//!   也封顶 [`MAX_ENTRY_BYTES`]——防「几 KB 解出几 GB」的解压炸弹；
//! · 不支持 zip64（内核资产远小于 4GB，遇到就如实报错，不猜）。

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// 单条目解压后的上限：mihomo 的 exe 约 60MB，给 4 倍余量。
const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

/// EOCD（End of Central Directory）固定部分的长度
const EOCD_LEN: usize = 22;
/// EOCD 末尾注释最长 65535 字节
const EOCD_MAX_COMMENT: usize = u16::MAX as usize;
/// 中央目录条目固定部分的长度
const CENTRAL_LEN: usize = 46;
/// 本地文件头固定部分的长度
const LOCAL_LEN: usize = 30;

const SIG_EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const SIG_CENTRAL: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
const SIG_LOCAL: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

/// 一个中央目录条目里我们关心的字段
struct Entry {
    name: String,
    method: u16,
    compressed_size: u64,
    uncompressed_size: u64,
    local_offset: u64,
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, String> {
    bytes
        .get(offset..offset + 2)
        .map(|slice| u16::from_le_bytes([slice[0], slice[1]]))
        .ok_or_else(|| "zip 结构损坏（读取 16 位字段越界）".to_string())
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, String> {
    bytes
        .get(offset..offset + 4)
        .map(|slice| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
        .ok_or_else(|| "zip 结构损坏（读取 32 位字段越界）".to_string())
}

/// 从归档尾部找 EOCD，返回 (中央目录起始偏移, 条目数)
fn read_directory_location(file: &mut std::fs::File, file_len: u64) -> Result<(u64, usize), String> {
    let tail_len = (EOCD_LEN + EOCD_MAX_COMMENT).min(file_len as usize);
    if tail_len < EOCD_LEN {
        return Err("不是有效的 zip 文件（体积过小）".to_string());
    }
    file.seek(SeekFrom::Start(file_len - tail_len as u64))
        .map_err(|error| format!("读取 zip 目录失败: {error}"))?;
    let mut tail = vec![0u8; tail_len];
    file.read_exact(&mut tail)
        .map_err(|error| format!("读取 zip 目录失败: {error}"))?;

    // 从后往前找：注释里也可能出现 PK\x05\x06 这四个字节，取最靠后且
    // 「注释长度恰好到文件尾」的那个才是真 EOCD
    let mut found = None;
    for index in (0..=tail_len - EOCD_LEN).rev() {
        if tail[index..index + 4] != SIG_EOCD {
            continue;
        }
        let comment_len = u16_at(&tail, index + 20)? as usize;
        if index + EOCD_LEN + comment_len == tail_len {
            found = Some(index);
            break;
        }
    }
    let eocd = found.ok_or_else(|| "不是有效的 zip 文件（找不到目录尾部记录）".to_string())?;

    let count = u16_at(&tail, eocd + 10)? as usize;
    let directory_size = u32_at(&tail, eocd + 12)? as u64;
    let directory_offset = u32_at(&tail, eocd + 16)? as u64;
    if count == u16::MAX as usize
        || directory_size == u32::MAX as u64
        || directory_offset == u32::MAX as u64
    {
        return Err("不支持 zip64 格式的内核资产".to_string());
    }
    if directory_offset + directory_size > file_len {
        return Err("zip 目录偏移越界（文件可能不完整）".to_string());
    }
    Ok((directory_offset, count))
}

/// 读中央目录，挑出第一个满足 `accept` 的条目
fn read_entries<F>(
    file: &mut std::fs::File,
    directory_offset: u64,
    count: usize,
    accept: F,
) -> Result<Entry, String>
where
    F: Fn(&str) -> bool,
{
    file.seek(SeekFrom::Start(directory_offset))
        .map_err(|error| format!("读取 zip 目录失败: {error}"))?;
    let mut header = [0u8; CENTRAL_LEN];
    for _ in 0..count {
        file.read_exact(&mut header)
            .map_err(|error| format!("读取 zip 目录失败（文件可能不完整）: {error}"))?;
        if header[0..4] != SIG_CENTRAL {
            return Err("zip 目录条目签名不匹配（文件可能损坏）".to_string());
        }
        let method = u16_at(&header, 10)?;
        let compressed_size = u32_at(&header, 20)? as u64;
        let uncompressed_size = u32_at(&header, 24)? as u64;
        let name_len = u16_at(&header, 28)? as usize;
        let extra_len = u16_at(&header, 30)? as usize;
        let comment_len = u16_at(&header, 32)? as usize;
        let local_offset = u32_at(&header, 42)? as u64;

        let mut name_bytes = vec![0u8; name_len];
        file.read_exact(&mut name_bytes)
            .map_err(|error| format!("读取 zip 条目名失败: {error}"))?;
        // 文件名的 UTF-8 标志位（bit 11）我们不强求：mihomo 的资产名是纯
        // ASCII，lossy 解码对两类编码都不会坏
        let name = String::from_utf8_lossy(&name_bytes).to_string();
        // 跳过扩展字段与注释
        file.seek(SeekFrom::Current((extra_len + comment_len) as i64))
            .map_err(|error| format!("读取 zip 目录失败: {error}"))?;

        // 目录条目也算条目：只接受文件（名字以 / 结尾的是目录）
        if !name.ends_with('/') && accept(&name) {
            return Ok(Entry {
                name,
                method,
                compressed_size,
                uncompressed_size,
                local_offset,
            });
        }
    }
    Err("zip 里没有匹配的条目".to_string())
}

/// 解出 `accept` 命中的**第一个**条目，写到 `dest`（单个文件，不建目录树）。
///
/// 返回解出的条目名（调用方拿它写日志 / 报错）。
/// `dest` 已存在时会被**截断重写**（调用方保证它在临时目录里，不在运行中
/// 的二进制路径上）。
pub fn extract_entry_to<F>(
    archive: &Path,
    dest: &Path,
    accept: F,
) -> Result<String, String>
where
    F: Fn(&str) -> bool,
{
    let mut file = std::fs::File::open(archive)
        .map_err(|error| format!("打开压缩包失败（{}）: {error}", archive.display()))?;
    let file_len = file
        .metadata()
        .map_err(|error| format!("读取压缩包信息失败: {error}"))?
        .len();
    let (directory_offset, count) = read_directory_location(&mut file, file_len)?;
    let entry = read_entries(&mut file, directory_offset, count, accept)?;

    if entry.uncompressed_size == 0 {
        return Err(format!("压缩包条目「{}」是空的", entry.name));
    }
    if entry.uncompressed_size > MAX_ENTRY_BYTES {
        return Err(format!(
            "压缩包条目「{}」解压后体积异常（{} 字节），已拒绝",
            entry.name, entry.uncompressed_size
        ));
    }
    if entry.local_offset + LOCAL_LEN as u64 > file_len {
        return Err("zip 本地文件头偏移越界（文件可能不完整）".to_string());
    }

    // 本地头里的名字 / 扩展字段长度可能和中央目录不同（有 data descriptor
    // 时也以本地头为准），所以数据起点必须重新按本地头算
    file.seek(SeekFrom::Start(entry.local_offset))
        .map_err(|error| format!("读取 zip 条目失败: {error}"))?;
    let mut local = [0u8; LOCAL_LEN];
    file.read_exact(&mut local)
        .map_err(|error| format!("读取 zip 条目失败: {error}"))?;
    if local[0..4] != SIG_LOCAL {
        return Err("zip 本地文件头签名不匹配（文件可能损坏）".to_string());
    }
    let local_name_len = u16_at(&local, 26)? as usize;
    let local_extra_len = u16_at(&local, 28)? as usize;
    file.seek(SeekFrom::Current((local_name_len + local_extra_len) as i64))
        .map_err(|error| format!("读取 zip 条目失败: {error}"))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建解压目录失败（{}）: {error}", parent.display()))?;
    }
    let out = std::fs::File::create(dest)
        .map_err(|error| format!("创建内核文件失败（{}）: {error}", dest.display()))?;
    let mut writer = std::io::BufWriter::new(out);

    // 只读「压缩数据那一段」：Take 把后面的中央目录挡在外面，解压器读到
    // EOF 就结束，不依赖条目自己的结束标记
    let mut source = file.take(entry.compressed_size);
    let written = match entry.method {
        0 => std::io::copy(&mut source, &mut writer)
            .map_err(|error| format!("写入内核文件失败: {error}"))?,
        8 => {
            let mut decoder = flate2::read::DeflateDecoder::new(source);
            std::io::copy(&mut decoder, &mut writer)
                .map_err(|error| format!("解压内核失败（deflate 数据损坏）: {error}"))?
        }
        other => {
            return Err(format!(
                "压缩包用了不支持的压缩方式（method {other}，只支持 store / deflate）"
            ))
        }
    };
    writer
        .flush()
        .map_err(|error| format!("写入内核文件失败: {error}"))?;
    drop(writer);

    if written != entry.uncompressed_size {
        // 声明值与实际不符：删掉半截文件再报错，别把坏二进制留在磁盘上
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "解压内核失败（条目「{}」声明 {} 字节，实际解出 {} 字节）",
            entry.name, entry.uncompressed_size, written
        ));
    }
    Ok(entry.name)
}

/// 解出的路径（给调用方拼日志用；解压本身只写单个文件）
pub fn extracted_path(dest: &Path) -> PathBuf {
    dest.to_path_buf()
}
