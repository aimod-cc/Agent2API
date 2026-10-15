//! 容器内自更新：原地替换可执行文件与 `ui/` 目录，重启进程后生效。
//!
//! ── 为什么是「换文件 + 退出进程」，而不是调 Docker API ──────────
//! 挂 `docker.sock` 等于把宿主机 root 交给一个面向上网的网页面板；这条
//! 路（照 sub2api 的做法）不改容器配置：可写层里的文件由本进程自己替换，
//! 替换完让进程**优雅退出** —— Docker 的 `restart: unless-stopped` 会把
//! **同一个容器**再拉起来（可写层保留），新版本随之生效。容器以 root 运行，
//! 可执行文件（`/usr/local/bin`）与界面目录（`/app/ui`）都可写。
//!
//! ── 代价：镜像标签与实际运行的版本会漂移 ──────────────────────
//! 面板更新过之后，`docker inspect` / compose 里记的仍是旧 tag，实际跑的是
//! 换上去的新版本；一旦 `docker compose pull && up -d`（重建容器），又回到
//! 镜像里的版本。这是「不碰 Docker 就能更新」的必然代价，文档里如实写明，
//! 面板上更新完成后也会提示。
//!
//! ── 开关：只认环境变量 ────────────────────────────────────────
//! `AGENT2API_SELF_UPDATE=1`（Dockerfile 里默认设好）。裸机/桌面形态没有
//! 重启监管（systemd 单元、容器 restart 策略都没有），退出进程等于把服务
//! 停掉 —— 那两种形态不能走这条路，因此不设变量就不提供入口。
//!
//! ── 安全边界 ──────────────────────────────────────────────────
//! 三步都不能省：① 下载地址过 `assert_downloadable` 的域名白名单（在下载
//! 那一步就做了）；② **必须**校验 `.sha256`（发布时由 CI 生成，取不到就
//! 拒绝安装，不降级为「无校验替换」）；③ 解包只接受点名条目
//! （`agent2api-server` 与 `ui/` 前缀），路径穿越一律拒绝、总量封顶。
//!
//! ── 归档格式（CI 按此打包，改名要三方一起改）──────────────────
//! `agent2api-server-linux-<arch>.tar.gz`，根下两个条目：
//!   · `agent2api-server`   可执行文件
//!   · `ui/**`              管理界面静态目录
//! 上位机侧的打包见 `.github/workflows/build.yml` 的 `linux-server` job。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::server::logging;

use super::client;
use super::version::{UpdateError, MAX_INSTALLER_BYTES};
use super::USER_AGENT;

/// 归档里的可执行文件条目名（与 CI 打包逐字一致）
const BINARY_ENTRY: &str = "agent2api-server";
/// 界面目录的条目前缀（含斜杠：只认 `ui/` 下的东西）
const UI_PREFIX: &str = "ui/";
/// 解包总量上限：包本身已有 600MB 下载上限，这里再封一道，防解压炸弹
/// （gzip 能把几 KB 放大到几 GB）
const MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;
/// tar 元数据条目（`L` / `x`）的大小上限：正常只有几十到几百字节
const MAX_META_BYTES: u64 = 1024 * 1024;
/// 校验文件请求的超时（几十字节的文本）
const CHECKSUM_TIMEOUT_MS: u64 = 30_000;

/// 部署开关：`AGENT2API_SELF_UPDATE=1`（严格等于 1，与 ALLOW_NO_KEY 同口径）。
/// 设 `0` 或空即关闭，Dockerfile 里默认 `1`。
pub fn enabled() -> bool {
    std::env::var("AGENT2API_SELF_UPDATE")
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

/// 本部署是否可用容器内更新：Linux + 开关打开 + 架构有对应的包名。
pub fn supported() -> bool {
    cfg!(target_os = "linux") && enabled() && bundle_name().is_some()
}

/// 本机架构对应的更新包名（非 Linux / 未知架构为 None）。
pub fn bundle_name() -> Option<String> {
    super::version::server_bundle_name()
}

/// 当前可执行文件的真实路径（解析符号链接；`/proc/self/exe` 拿到的是真身）。
pub fn current_exe_path() -> Result<PathBuf, UpdateError> {
    let exe = std::env::current_exe()
        .map_err(|error| UpdateError::new(format!("无法定位当前可执行文件：{error}")))?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// 上一版本的备份路径：与目标同目录、加 `.backup` 后缀（只保留一代）。
fn backup_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "agent2api".to_string());
    target.with_file_name(format!("{name}.backup"))
}

/// 是否存在可回滚的上一版本（面板据此决定要不要显示回滚入口）。
pub fn backup_available() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    current_exe_path()
        .map(|exe| backup_path(&exe).is_file())
        .unwrap_or(false)
}

// ─── 校验和 ─────────────────────────────────────────────────

/// 取 `.sha256` 伴随文件里的期望哈希。
///
/// 内容按 `sha256sum` 的格式解析（`<64 位十六进制>␣␣<文件名>`），也容忍只有
/// 哈希本身。格式不对直接报错 —— 这一环是「装上去的东西确实是发布产物」的
/// 唯一依据，含糊接受等于没校验。
pub async fn fetch_checksum(url: &str) -> Result<String, UpdateError> {
    let headers = vec![
        ("User-Agent".to_string(), USER_AGENT.to_string()),
        ("Accept".to_string(), "text/plain".to_string()),
    ];
    let response = client::fetch_with_egress(url, &headers, Some(CHECKSUM_TIMEOUT_MS)).await?;
    if !response.status().is_success() {
        return Err(UpdateError::new(format!(
            "下载校验文件失败：GitHub 返回 HTTP {}",
            response.status().as_u16()
        )));
    }
    let text = response
        .text()
        .await
        .map_err(|error| UpdateError::new(format!("读取校验文件失败：{error}")))?;
    parse_checksum(&text)
}

/// 从校验文件正文里取哈希（首段 + 64 位十六进制）。
fn parse_checksum(text: &str) -> Result<String, UpdateError> {
    let token = text.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(token)
    } else {
        Err(UpdateError::new("校验文件格式异常（找不到 sha256 哈希），已放弃安装"))
    }
}

/// 文件内容的 sha256（十六进制小写，流式读取，不整包进内存）。
pub fn file_sha256(path: &Path) -> Result<String, UpdateError> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| UpdateError::new(format!("打开更新包失败：{error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| UpdateError::new(format!("读取更新包失败：{error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ─── 解包 ───────────────────────────────────────────────────

/// 解包结果（二进制是否拿到、界面文件写了几个）。
pub struct Extracted {
    pub binary: bool,
    pub ui_files: usize,
}

/// 把归档解到两个暂存目录：可执行文件 → `bin_stage/agent2api-server`，
/// `ui/**` → `ui_stage/**`（前缀剥掉）。
///
/// 手写 tar 解析（照 `providers/qoder/risk.rs` 的 `tar_entry`，那条只取单个
/// 条目、这里要整包落盘）：处理 GNU longname（`L`）与 pax 扩展头（`x`）的
/// path 覆盖 —— Rust 官方 tar 打包时默认就带 pax 头，漏了会把长路径读成乱码。
///
/// 只接受白名单条目，其余条目**照样读完但丢弃**（tar 是流式格式，不读内容
/// 会错位）。路径穿越（`..`、绝对路径、反斜杠）一律拒绝。
pub fn extract_bundle(
    archive: &Path,
    bin_stage: &Path,
    ui_stage: Option<&Path>,
) -> Result<Extracted, UpdateError> {
    let file = std::fs::File::open(archive)
        .map_err(|error| UpdateError::new(format!("打开更新包失败：{error}")))?;
    let mut tar = flate2::read::GzDecoder::new(file);

    let mut header = [0u8; 512];
    let mut long_name: Option<String> = None;
    let mut total: u64 = 0;
    let mut out = Extracted { binary: false, ui_files: 0 };

    loop {
        if !read_header(&mut tar, &mut header)? {
            break;
        }
        if header.iter().all(|byte| *byte == 0) {
            break; // 结束块
        }
        let size = octal(&header[124..136]);
        let entry_type = header[156] as char;
        let prefix = cstr(&header[345..500]);
        let mut name = long_name.take().unwrap_or_else(|| {
            if prefix.is_empty() {
                cstr(&header[..100])
            } else {
                format!("{prefix}/{}", cstr(&header[..100]))
            }
        });

        // 元数据条目：GNU longname 直接给名字；pax 头里找 `path=` 覆盖
        match entry_type {
            'L' => {
                let body = read_meta(&mut tar, size)?;
                name = String::from_utf8_lossy(&body).trim_end_matches('\0').to_string();
                read_padding(&mut tar, size)?;
                long_name = Some(name);
                continue;
            }
            'x' => {
                let body = read_meta(&mut tar, size)?;
                let text = String::from_utf8_lossy(&body).to_string();
                if let Some(value) = text
                    .lines()
                    .find_map(|line| line.strip_prefix("path=").map(|value| value.trim().to_string()))
                {
                    long_name = Some(value);
                }
                read_padding(&mut tar, size)?;
                continue;
            }
            _ => {}
        }

        let relative = safe_relative(&name);
        let target: Option<PathBuf> = match (relative, entry_type) {
            (Some(relative), _) if relative == Path::new(BINARY_ENTRY) => {
                Some(bin_stage.join(BINARY_ENTRY))
            }
            // `ui/` 下的条目：把前缀剥掉再落到界面暂存目录（`Path` 的
            // `starts_with` 按组件比较，`"ui/"` 与 `"ui"` 等价）
            (Some(relative), _) if relative.starts_with(UI_PREFIX) => ui_stage.map(|stage| {
                let tail = relative.strip_prefix(UI_PREFIX).unwrap_or(&relative);
                stage.join(tail)
            }),
            _ => None,
        };

        match (target, entry_type) {
            // 目录条目：建出来即可（内容为空）
            (Some(target), '5') => {
                let _ = std::fs::create_dir_all(&target);
                read_padding(&mut tar, size)?;
            }
            // 普通文件：落盘；不在白名单里的照样跳过内容（tar 是流式的）
            (target, '0' | '\0') => {
                total = total.saturating_add(size);
                if total > MAX_EXTRACTED_BYTES {
                    return Err(UpdateError::new("更新包解压后体积异常，已中止"));
                }
                match target {
                    Some(target) => {
                        write_entry(&mut tar, &target, size)?;
                        if target.starts_with(bin_stage) {
                            out.binary = true;
                        } else {
                            out.ui_files += 1;
                        }
                    }
                    None => {
                        skip_exact(&mut tar, size)?;
                    }
                }
                read_padding(&mut tar, size)?;
            }
            // 其余类型（符号链接、设备文件等）：只跳过，不落盘
            _ => {
                skip_exact(&mut tar, size)?;
                read_padding(&mut tar, size)?;
            }
        }
    }
    Ok(out)
}

/// 条目名的安全归一：去掉 `./` 前缀，拒绝绝对路径 / `..` / 反斜杠 / 空段。
/// 返回可安全 `join` 到暂存目录的相对路径。
fn safe_relative(name: &str) -> Option<PathBuf> {
    let trimmed = name.trim_start_matches("./").trim();
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.contains('\\') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 读满一个 512 字节头：正常结束（归档尾部）返回 false。
fn read_header(reader: &mut impl Read, buffer: &mut [u8; 512]) -> Result<bool, UpdateError> {
    let mut filled = 0usize;
    while filled < buffer.len() {
        let read = reader
            .read(&mut buffer[filled..])
            .map_err(|error| UpdateError::new(format!("读取更新包失败：{error}")))?;
        if read == 0 {
            if filled == 0 {
                return Ok(false);
            }
            return Err(UpdateError::new("更新包已损坏（tar 头不完整）"));
        }
        filled += read;
    }
    Ok(true)
}

/// 读一个元数据条目的正文（有上限，超了直接报错）。
fn read_meta(reader: &mut impl Read, size: u64) -> Result<Vec<u8>, UpdateError> {
    if size > MAX_META_BYTES {
        return Err(UpdateError::new("更新包已损坏（扩展头异常）"));
    }
    let mut buffer = vec![0u8; size as usize];
    reader
        .read_exact(&mut buffer)
        .map_err(|error| UpdateError::new(format!("读取更新包失败：{error}")))?;
    Ok(buffer)
}

/// 落盘一个条目（带体积上限与文件权限）。
fn write_entry(reader: &mut impl Read, target: &Path, size: u64) -> Result<(), UpdateError> {
    if size > MAX_INSTALLER_BYTES {
        return Err(UpdateError::new("更新包里的条目体积异常，已中止"));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| UpdateError::new(format!("创建目录失败（{}）：{error}", parent.display())))?;
    }
    let mut file = std::fs::File::create(target)
        .map_err(|error| UpdateError::new(format!("写入文件失败（{}）：{error}", target.display())))?;
    let mut remaining = size;
    let mut buffer = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let chunk = buffer.len().min(remaining as usize);
        reader
            .read_exact(&mut buffer[..chunk])
            .map_err(|error| UpdateError::new(format!("读取更新包失败：{error}")))?;
        file.write_all(&buffer[..chunk])
            .map_err(|error| UpdateError::new(format!("写入文件失败（{}）：{error}", target.display())))?;
        remaining -= chunk as u64;
    }
    file.flush()
        .map_err(|error| UpdateError::new(format!("写入文件失败（{}）：{error}", target.display())))?;
    set_mode(target, 0o644);
    Ok(())
}

/// 跳过 size 字节（不在白名单里的条目）。
fn skip_exact(reader: &mut impl Read, size: u64) -> Result<(), UpdateError> {
    let mut remaining = size;
    let mut buffer = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let chunk = buffer.len().min(remaining as usize);
        reader
            .read_exact(&mut buffer[..chunk])
            .map_err(|error| UpdateError::new(format!("读取更新包失败：{error}")))?;
        remaining -= chunk as u64;
    }
    Ok(())
}

/// 跳过条目正文后的对齐填充（tar 每个条目按 512 字节对齐）。
fn read_padding(reader: &mut impl Read, size: u64) -> Result<(), UpdateError> {
    let padding = (512 - (size % 512)) % 512;
    if padding > 0 {
        skip_exact(reader, padding)?;
    }
    Ok(())
}

/// 八进制字段（tar 头的数字都是 ASCII 八进制，可能带空格 / NUL）。
fn octal(bytes: &[u8]) -> u64 {
    let text = cstr(bytes);
    u64::from_str_radix(text.trim(), 8).unwrap_or(0)
}

/// tar 头里的 C 字符串字段
fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

/// 设置文件权限（只对 unix 有意义；其它平台是空操作）。
fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

// ─── 替换与回滚 ─────────────────────────────────────────────

/// 删除文件或目录（不存在、失败都不报错 —— 调用点都是「尽力清掉」）。
fn remove_path(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// 同目录原子替换（文件）：`target` → `backup`，`staged` → `target`。
///
/// 暂存文件必须与目标**同目录**（同一文件系统），`rename` 才是原子的；
/// 第二步失败时把备份还原回去，不留半套。
///
/// 文件走 rename 是安全的：overlayfs 上改名的下层文件会先 copy-up 到可写层，
/// 实测（`/usr/local/bin/agent2api-server`）可行。**目录不能这么干** —— 见
/// [`replace_dir`]。
fn replace_file(target: &Path, staged: &Path, backup: &Path) -> Result<(), UpdateError> {
    remove_path(backup); // 只保留一代备份
    std::fs::rename(target, backup).map_err(|error| {
        UpdateError::new(format!("备份现行文件失败（{}）：{error}", target.display()))
    })?;
    if let Err(error) = std::fs::rename(staged, target) {
        let _ = std::fs::rename(backup, target);
        return Err(UpdateError::new(format!("替换失败（已还原）：{error}")));
    }
    Ok(())
}

/// 目录替换：先把现行目录**复制**成备份，再删掉它、把暂存目录改名进来。
///
/// ── 为什么不是 `rename(现行目录 → 备份)` ─────────────────────
/// 目录还留在**镜像层**（下层）时，overlayfs 的 rename 会报
/// `EXDEV: Invalid cross-device link` —— 实测 Docker Desktop 上 `/app/ui`
/// 就是这种目录（第一次跑通下载、走到换目录这一步才暴露）。
/// 所以目录一律走「复制备份 → 删除原件 → 改名进来」：
///   · 删除走 whiteout，是 overlayfs 明确支持的路径；
///   · 改名只发生在可写层内部（暂存目录与备份都是我们刚创建的）。
fn replace_dir(target: &Path, staged: &Path, backup: &Path) -> Result<(), UpdateError> {
    remove_path(backup);
    copy_dir_all(target, backup).map_err(|error| {
        UpdateError::new(format!("备份目录失败（{}）：{error}", target.display()))
    })?;
    remove_path(target);
    if let Err(error) = std::fs::rename(staged, target) {
        // 还原：备份就在可写层里，直接改名回去
        let _ = std::fs::rename(backup, target);
        return Err(UpdateError::new(format!("替换目录失败（已还原）：{error}")));
    }
    Ok(())
}

/// 递归复制目录（std 没有现成的）。只处理普通文件与目录 ——
/// `ui/` 里没有符号链接，CI 打的包里也是普通条目。
fn copy_dir_all(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let destination = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &destination)?;
        }
    }
    Ok(())
}

/// 从备份还原：现行版本先挪到临时名，备份落位后再删临时名。
///
/// 为什么不直接 `rename(backup, target)`：目录的 rename 要求目标为空或不存在
/// （`ENOTEMPTY`），而这里是「用一个已有目录换掉另一个已有目录」。
///
/// 回滚这条路可以放心用 rename：走到这里时 target 与 backup **都在可写层**
/// （现行版本是 [`replace_dir`] 改名进来的，备份是它复制出来的），不存在
/// overlayfs 上层/下层跨层改名的问题（[`replace_dir`] 的 `EXDEV` 只出现在
/// 改**镜像层**里的目录时）。
fn restore(target: &Path, backup: &Path) -> Result<(), UpdateError> {
    if !backup.exists() {
        return Err(UpdateError::with_status(400, "没有可回滚的上一版本（备份不存在）"));
    }
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "agent2api".to_string());
    let stale = target.with_file_name(format!("{name}.rollback-tmp"));
    remove_path(&stale);
    std::fs::rename(target, &stale)
        .map_err(|error| UpdateError::new(format!("回滚失败（无法移开现行版本）：{error}")))?;
    if let Err(error) = std::fs::rename(backup, target) {
        let _ = std::fs::rename(&stale, target);
        return Err(UpdateError::new(format!("回滚失败（已还原）：{error}")));
    }
    remove_path(&stale);
    Ok(())
}

/// 安装结果（界面目录有没有一起换掉）。
pub struct ApplyOutcome {
    pub ui_updated: bool,
}

/// 应用更新包：解包 → 替换二进制 → 替换 `ui/` 目录。
///
/// 顺序有意固定：先换二进制（这一步失败则什么都没动），再换界面 ——
/// 界面替换失败会把二进制也还原，保证「要么整包生效，要么整包不动」。
/// `ui_dir` 为 None（纯 API 部署没有界面目录）时只换二进制。
pub fn apply(
    archive: &Path,
    exe: &Path,
    ui_dir: Option<&Path>,
) -> Result<ApplyOutcome, UpdateError> {
    let parent = exe
        .parent()
        .ok_or_else(|| UpdateError::new("无法定位可执行文件所在目录"))?;
    let id = format!("{}-{}", std::process::id(), logging::now_ms());
    let bin_stage = parent.join(format!(".a2a-update-{id}"));
    // 界面的暂存目录必须与界面目录同父（rename 要同文件系统）
    let ui_stage = match ui_dir {
        Some(dir) if dir.is_dir() => match dir.parent() {
            Some(parent) => Some(parent.join(format!(".a2a-ui-{id}"))),
            None => None,
        },
        _ => None,
    };

    // 暂存目录先建出来：只读根文件系统 / 权限不足在这里就报错，
    // 而不是替换到一半才发现写不进去
    std::fs::create_dir_all(&bin_stage).map_err(|error| {
        UpdateError::new(format!(
            "无法在 {} 下创建暂存目录（容器根文件系统可能只读）：{error}",
            parent.display()
        ))
    })?;
    if let Some(stage) = &ui_stage {
        if let Err(error) = std::fs::create_dir_all(stage) {
            let _ = std::fs::remove_dir_all(&bin_stage);
            return Err(UpdateError::new(format!("无法创建界面暂存目录：{error}")));
        }
    }

    let outcome = (|| {
        let extracted = extract_bundle(archive, &bin_stage, ui_stage.as_deref())?;
        if !extracted.binary {
            return Err(UpdateError::new("更新包里没有 agent2api-server，已放弃安装"));
        }
        let staged_bin = bin_stage.join(BINARY_ENTRY);
        set_mode(&staged_bin, 0o755);

        replace_file(exe, &staged_bin, &backup_path(exe))?;

        let mut ui_updated = false;
        if let (Some(dir), Some(stage)) = (ui_dir, ui_stage.as_deref()) {
            if dir.is_dir() {
                if let Err(error) = replace_dir(dir, stage, &backup_path(dir)) {
                    // 整单失败：二进制也还原，不留「新版程序 + 旧版界面」的半套
                    let _ = restore(exe, &backup_path(exe));
                    return Err(error);
                }
                ui_updated = true;
            }
        }
        Ok(ApplyOutcome { ui_updated })
    })();

    let _ = std::fs::remove_dir_all(&bin_stage);
    if let Some(stage) = &ui_stage {
        let _ = std::fs::remove_dir_all(stage);
    }
    outcome
}

/// 回滚到上一版本（二进制与界面一起）。
pub fn rollback(exe: &Path, ui_dir: Option<&Path>) -> Result<ApplyOutcome, UpdateError> {
    if !backup_path(exe).is_file() {
        return Err(UpdateError::with_status(400, "没有可回滚的上一版本（备份不存在）"));
    }
    restore(exe, &backup_path(exe))?;
    let mut ui_updated = false;
    if let Some(dir) = ui_dir {
        if backup_path(dir).is_dir() {
            restore(dir, &backup_path(dir))?;
            ui_updated = true;
        }
    }
    Ok(ApplyOutcome { ui_updated })
}

// ─── 重启 ───────────────────────────────────────────────────

/// 重启钩子：由 headless 入口注册（桌面形态不注册 —— 那边重启归壳管）。
static RESTART_HOOK: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// 注册重启钩子（重复注册忽略；钩子里做「优雅停机」的实际动作）。
pub fn set_restart_hook(hook: Box<dyn Fn() + Send + Sync>) {
    let _ = RESTART_HOOK.set(hook);
}

/// 请求重启：延迟一小会儿再触发钩子，让接口的成功响应先回到浏览器。
///
/// 返回 false = 没有钩子（不是 headless 容器入口起的进程）—— 调用方据此
/// 拒绝请求，而不是静默什么都不做。
pub fn request_restart() -> bool {
    let Some(hook) = RESTART_HOOK.get() else {
        return false;
    };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        hook();
    });
    true
}
