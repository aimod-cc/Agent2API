//! 内核的磁盘布局：**版本化目录 + current 指针**，安装 / 卸载 / 残留清理都在这里。
//!
//! ── 为什么要版本化（对照「就地覆盖 mihomo.exe」）──────────────
//! 旧布局把二进制固定成 `{kernelDir}/mihomo.exe`，于是两个动作都要动**正在
//! 运行的那个文件**：
//!   · 升级 / 重装：先删旧 exe 再解压，解压一失败旧版本也没了；
//!   · 删除内核：Windows 上运行中的 exe 删不掉，杀进程又依赖外部命令。
//! 换成版本目录后，二进制在 `versions/<版本>/` 里**只写一次**，切换 / 卸载
//! 都只动指针（一个几十字节的文本文件）——正在运行的 exe 永远不被覆盖，
//! 删不掉时也只是把整个版本目录改名成 `.trash-*`（目录改名不受目录内打开
//! 文件的影响），用户操作立刻返回，残留交给下次启动清。
//!
//! ```text
//! {kernelDir}/
//!   versions/v1.19.32/mihomo.exe   ← 二进制（只写一次）
//!   versions/v1.19.32/.ok          ← 完整安装标记（解压 + 校验都过了才写）
//!   current                        ← 文本：v1.19.32（写临时文件再 rename，原子）
//!   providers/ config.yaml kernel-state.json kernel.log
//!   downloads/<资产名>.part        ← 下载中的半截包（下完即删）
//! ```
//!
//! ── 旧布局的迁移 ────────────────────────────────────────────
//! 老用户目录里只有一个 `mihomo.exe`。启动时 `migrate_legacy()` 把它搬进
//! `versions/<pin 的版本>/` 并写指针；搬不动（内核正在跑，rename 被拒）就
//! 保持旧布局照常可用，下次启动再试。删除内核时旧布局的 exe 也会一并处理。
//!
//! ── 卸载永远「成功」────────────────────────────────────────
//! `installed()` 只认 `current` 指针，所以卸载第一步就是摘指针 —— 之后界面
//! 立刻显示未安装。文件删不干净不影响这个结论，只是留下 `pendingCleanup`
//! （状态 JSON 里的字段），下次启动由 `cleanup_residue()` 再清一遍。

use std::path::{Path, PathBuf};

use crate::server::core::sub_kernel::{kernel_binary_name, kernel_dir, log_line, KERNEL_VERSION};

/// 版本目录的父目录名
const VERSIONS_DIR: &str = "versions";
/// 指针文件名（内容是一行版本号）
const CURRENT_FILE: &str = "current";
/// 完整安装标记（空文件；存在即表示这个版本目录已解压完且校验过）
const OK_MARKER: &str = ".ok";
/// 下载暂存目录名
const DOWNLOADS_DIR: &str = "downloads";

/// 一次可用的安装：版本号 + 版本目录 + 二进制路径
#[derive(Clone, Debug)]
pub struct Install {
    pub version: String,
    pub dir: PathBuf,
    pub binary: PathBuf,
    /// 旧布局（二进制直接躺在 kernelDir 根下，没有版本目录）
    pub legacy: bool,
}

pub fn versions_dir() -> PathBuf {
    kernel_dir().join(VERSIONS_DIR)
}

fn current_file() -> PathBuf {
    kernel_dir().join(CURRENT_FILE)
}

fn downloads_dir() -> PathBuf {
    kernel_dir().join(DOWNLOADS_DIR)
}

/// 下载暂存目录（下载模块用；目录不存在时按需创建）
pub fn download_dir() -> PathBuf {
    downloads_dir()
}

fn version_dir(version: &str) -> PathBuf {
    versions_dir().join(version)
}

fn legacy_binary() -> PathBuf {
    kernel_dir().join(kernel_binary_name())
}

fn read_current() -> Option<String> {
    let text = std::fs::read_to_string(current_file()).ok()?;
    let version = text.trim();
    if version.is_empty() || version.contains(['/', '\\', ':']) {
        return None;
    }
    Some(version.to_string())
}

/// 原子写指针：先写 `current.tmp` 再 rename 覆盖 —— 中途掉电不会留下半行
/// 版本号（rename 在同一卷上是原子的，Windows 上也覆盖已存在的目标文件）
fn write_current(version: &str) -> Result<(), String> {
    let dir = kernel_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("创建内核目录失败（{}）: {error}", dir.display()))?;
    let tmp = dir.join(format!("{CURRENT_FILE}.tmp"));
    std::fs::write(&tmp, format!("{version}\n"))
        .map_err(|error| format!("写入内核版本指针失败: {error}"))?;
    std::fs::rename(&tmp, current_file()).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("写入内核版本指针失败: {error}")
    })
}

fn write_ok_marker(dir: &Path, version: &str) -> Result<(), String> {
    std::fs::write(dir.join(OK_MARKER), format!("{version}\n"))
        .map_err(|error| format!("写入安装标记失败: {error}"))
}

/// 在 `versions/` 里找一个完整安装（有二进制 + `.ok`），优先 pin 的那个版本。
///
/// 只给「指针丢了」的自愈路径用：有多个候选时取 pin 的版本，其次取名字最大
/// 的（版本号字符串序，够用 —— 这里只是救急，不是排序功能）。
fn find_installed_version() -> Option<String> {
    let entries = std::fs::read_dir(versions_dir()).ok()?;
    let mut candidates: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| !name.starts_with('.') && !name.ends_with(".tmp"))
        .filter(|name| {
            let dir = version_dir(name);
            dir.join(kernel_binary_name()).exists() && dir.join(OK_MARKER).exists()
        })
        .collect();
    if candidates.iter().any(|name| name == KERNEL_VERSION) {
        return Some(KERNEL_VERSION.to_string());
    }
    candidates.sort();
    candidates.pop()
}

/// 当前生效的安装（指针 → 版本目录 → 二进制 + `.ok`，缺一不可）
pub fn current_install() -> Option<Install> {
    let version = read_current()?;
    let dir = version_dir(&version);
    let binary = dir.join(kernel_binary_name());
    if !binary.exists() || !dir.join(OK_MARKER).exists() {
        return None;
    }
    Some(Install {
        version,
        dir,
        binary,
        legacy: false,
    })
}

/// 旧布局的二进制（没迁移成功时仍算已安装）
fn legacy_install() -> Option<Install> {
    let binary = legacy_binary();
    if !binary.exists() {
        return None;
    }
    Some(Install {
        version: KERNEL_VERSION.to_string(),
        dir: kernel_dir(),
        binary,
        legacy: true,
    })
}

/// 当前可执行的安装：版本目录优先，其次旧布局
pub fn resolve_install() -> Option<Install> {
    current_install().or_else(legacy_install)
}

/// 内核是否已安装（二进制存在且安装完整）
pub fn installed() -> bool {
    resolve_install().is_some()
}

/// 已安装版本（状态 JSON 用；未安装为 None）
pub fn installed_version() -> Option<String> {
    resolve_install().map(|install| install.version)
}

/// 要 spawn 的二进制路径
pub fn binary_path() -> Option<PathBuf> {
    resolve_install().map(|install| install.binary)
}

// ─── 迁移旧布局 ────────────────────────────────────────────

/// 把根目录下的旧 `mihomo.exe` 搬进 `versions/<pin>/` 并写指针。
///
/// 返回 true = 迁移完成（或本来就不需要迁移）。搬不动时返回 false 并保持旧
/// 布局可用 —— 最常见的原因是内核正在运行（Windows 不允许 rename 运行中的
/// exe），下次启动内核没跑的时候会再试。
///
/// ── 指针丢失的自愈 ──────────────────────────────────────────
/// 先看 `current` 指针：它指向有效安装就什么都不做；指向一个不存在的目录
/// （手工删了 / 上次安装崩在写指针之前）时，从 `versions/` 里找一个带 `.ok`
/// 的目录重建指针 —— 否则「exe 明明在磁盘上，界面却一直说未安装」，只能重下。
pub fn migrate_legacy() -> bool {
    if current_install().is_some() {
        return true;
    }
    // 指针存在但无效（或干脆没有）：优先从已有的版本目录自愈
    if let Some(version) = find_installed_version() {
        if write_current(&version).is_ok() {
            log_line(&format!("📦 已修复内核版本指针（versions/{version}）"));
            return true;
        }
    }
    let Some(legacy) = legacy_install() else {
        return true;
    };
    let target_dir = version_dir(&legacy.version);
    if std::fs::create_dir_all(&target_dir).is_err() {
        return false;
    }
    let target = target_dir.join(kernel_binary_name());
    if target.exists() {
        // 上次迁移到一半：目标已在，删掉根目录那份即可
        let _ = std::fs::remove_file(&legacy.binary);
    } else if std::fs::rename(&legacy.binary, &target).is_err() {
        // 内核在跑（或被杀软拦着）：保持旧布局，下次再试
        let _ = std::fs::remove_dir_all(&target_dir);
        return false;
    }
    if write_ok_marker(&target_dir, &legacy.version).is_err() {
        return false;
    }
    match write_current(&legacy.version) {
        Ok(()) => {
            log_line(&format!(
                "📦 内核目录已迁移到版本化布局（versions/{}）",
                legacy.version
            ));
            true
        }
        Err(error) => {
            log_line(&format!("⚠️ 写入内核版本指针失败: {error}"));
            false
        }
    }
}

// ─── 安装 ──────────────────────────────────────────────────

/// 从已校验的压缩包安装，成功返回新安装（指针已切过去）。
///
/// 步骤：解到 `versions/.staging-<pid>/` → 校验产物（非空 + 平台魔数）→ 写
/// `.ok` → 目录改名到 `versions/<版本>/` → 原子切 `current` → 清旧版本。
/// 任何一步失败都不动 `current`：用户原来那份安装（如果有）照常可用。
pub fn install_from_archive(archive: &Path, version: &str) -> Result<Install, String> {
    let staging = versions_dir().join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("创建解压目录失败（{}）: {error}", staging.display()))?;

    let staged_binary = staging.join(kernel_binary_name());
    if let Err(error) = extract_binary(archive, &staged_binary) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = verify_binary(&staged_binary) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    write_ok_marker(&staging, version)?;

    let target = version_dir(version);
    if target.exists() {
        // 同版本重装：目标目录还在（上次没清干净）。删不掉说明有进程在用，
        // 这时不能继续（切指针会指到一个半旧半新的目录）
        std::fs::remove_dir_all(&target).map_err(|error| {
            let _ = std::fs::remove_dir_all(&staging);
            format!(
                "旧版本目录删不掉（{}，可能仍被进程占用）: {error}",
                target.display()
            )
        })?;
    }
    std::fs::rename(&staging, &target).map_err(|error| {
        let _ = std::fs::remove_dir_all(&staging);
        format!("安装内核失败（{}）: {error}", target.display())
    })?;

    if let Err(error) = write_current(version) {
        let _ = std::fs::remove_dir_all(&target);
        return Err(error);
    }

    prune_other_versions(version);
    Ok(Install {
        version: version.to_string(),
        binary: target.join(kernel_binary_name()),
        dir: target,
        legacy: false,
    })
}

/// 解压到指定文件：Windows 是单条目 zip，macOS / Linux 是裸 gzip
fn extract_binary(archive: &Path, dest: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        // 官方 Windows 资产只含一个 exe 条目
        super::zip::extract_entry_to(archive, dest, |name| {
            name.to_ascii_lowercase().ends_with(".exe")
        })?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use std::io::Write;
        let source = std::fs::File::open(archive)
            .map_err(|error| format!("打开压缩包失败（{}）: {error}", archive.display()))?;
        let mut decoder = flate2::read::GzDecoder::new(source);
        let mut output = std::fs::File::create(dest)
            .map_err(|error| format!("创建内核文件失败（{}）: {error}", dest.display()))?;
        std::io::copy(&mut decoder, &mut output)
            .map_err(|error| format!("解压内核失败（gzip 数据损坏）: {error}"))?;
        output
            .flush()
            .map_err(|error| format!("写入内核文件失败: {error}"))?;
        output
            .sync_all()
            .map_err(|error| format!("写入内核文件失败: {error}"))?;
        // 从网络下载的二进制默认没有执行权限
        use std::os::unix::fs::PermissionsExt;
        let _ = output.set_permissions(std::fs::Permissions::from_mode(0o755));
        Ok(())
    }
}

/// 产物校验：非空 + 平台可执行文件魔数。
///
/// 魔数这一道能挡住「GitHub 返回了 HTML 错误页 / 代理返回了拦截页」这类
/// 内容型失败 —— 它们 sha256 也对不上，但报「解压出来不是可执行文件」比
/// 「校验失败」更容易让人明白发生了什么。
fn verify_binary(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("读取解压结果失败（{}）: {error}", path.display()))?;
    if metadata.len() < 1024 {
        return Err(format!(
            "解压结果不是有效的内核二进制（只有 {} 字节）",
            metadata.len()
        ));
    }
    use std::io::Read;
    let mut head = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .map_err(|error| format!("读取解压结果失败: {error}"))?;
    let looks_right = if cfg!(windows) {
        &head[..2] == b"MZ"
    } else if cfg!(target_os = "linux") {
        &head == b"\x7fELF"
    } else {
        // macOS：thin Mach-O（CF FA ED FE / CE FA ED FE）或 fat（CA FE BA BE）
        matches!(
            head,
            [0xcf, 0xfa, 0xed, 0xfe] | [0xce, 0xfa, 0xed, 0xfe] | [0xca, 0xfe, 0xba, 0xbe]
        )
    };
    if !looks_right {
        return Err("解压结果不是当前平台的可执行文件（下载到的可能不是内核包）".to_string());
    }
    Ok(())
}

/// 切指针之后清掉其它版本目录（best-effort：删不掉就留着，下次启动再清）
fn prune_other_versions(current: &str) {
    let Ok(entries) = std::fs::read_dir(versions_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == current || name.starts_with(".trash-") || name.starts_with(".staging-") {
            continue;
        }
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

// ─── 卸载 ──────────────────────────────────────────────────

/// 卸载结果。`residue = true` 表示「内核已经不可用（指针已摘），但磁盘上还有
/// 删不掉的文件」—— 对用户仍是成功，只是要如实说一句残留会稍后清理。
pub struct UninstallOutcome {
    pub residue: bool,
    pub detail: Option<String>,
}

/// 卸载内核：摘指针 → 删整个内核目录 → 删不掉的整目录改名进回收位。
///
/// 调用方**应先停内核进程**（`Command::Remove` 分支已经这么做）。但即使进程
/// 没停干净，这里也不会失败：摘掉指针后 `installed()` 立刻为 false，删不动的
/// 目录改名进回收位，用户的操作不被一个还活着的进程挡住。
///
/// ── 为什么降级用「整目录改名」而不是逐个删 ─────────────────
/// 实测（Windows 11）：正在运行的 exe 本身删不掉（共享冲突），但**它所在的
/// 目录可以改名** —— 目录改名只动父目录的项，不碰文件句柄。所以降级顺序是：
/// 先试整目录删（进程已停时一次成功），失败就整目录改名到同级的 `.trash-*`，
/// 配置里的 kernelDir 立刻变空，下次启动由 `cleanup_residue` 删回收位。
/// 这比「逐个版本目录改名再重删」少一半步骤，也不会在中途留下半套状态。
pub fn uninstall() -> Result<UninstallOutcome, String> {
    let dir = kernel_dir();
    if !dir.exists() {
        return Ok(UninstallOutcome {
            residue: false,
            detail: None,
        });
    }
    // ① 摘指针：这一步之后界面就是「未安装」，后面的删除都是收尾
    let _ = std::fs::remove_file(current_file());

    // ② 进程已停（常态）：一次删干净
    if std::fs::remove_dir_all(&dir).is_ok() {
        return Ok(UninstallOutcome {
            residue: false,
            detail: None,
        });
    }

    // ③ 还有文件被占着（内核没停干净 / 杀软在扫）：整目录改名进回收位。
    //    改名成功后配置里的 kernelDir 就是空的，下次下载会重新建目录。
    let trash = dir.with_file_name(format!(
        "{}.trash-{}",
        dir.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "proxy-kernel".to_string()),
        crate::server::logging::now_ms()
    ));
    match std::fs::rename(&dir, &trash) {
        Ok(()) => {
            // 改名后再试一次删除：占用可能刚好释放（杀软扫描结束之类）
            if std::fs::remove_dir_all(&trash).is_ok() {
                return Ok(UninstallOutcome {
                    residue: false,
                    detail: None,
                });
            }
            log_line(&format!(
                "🧹 内核目录仍有文件被占用，已移入回收位（{}），下次启动清理",
                trash.display()
            ));
            Ok(UninstallOutcome {
                residue: true,
                detail: Some(trash.to_string_lossy().to_string()),
            })
        }
        // ④ 连改名都不行（目录本身被句柄钉住 / 权限不足）：如实报，指针已摘，
        //    内核已经不可用；下次启动会再试一次清理
        Err(error) => {
            let detail = format!("{} 没能删除（{error}）", dir.display());
            log_line(&format!("⚠️ {detail}"));
            Ok(UninstallOutcome {
                residue: true,
                detail: Some(detail),
            })
        }
    }
}

/// 是否还有等待清理的残留（状态 JSON 的 `pendingCleanup`）
pub fn pending_cleanup() -> bool {
    let dir = kernel_dir();
    let has_trash = |path: &Path| {
        std::fs::read_dir(path)
            .map(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".trash-")
                })
            })
            .unwrap_or(false)
    };
    if has_trash(&versions_dir()) {
        return true;
    }
    // 同级回收位（整目录改名那条路）
    let parent = dir.parent().map(Path::to_path_buf);
    let prefix = dir
        .file_name()
        .map(|name| format!("{}.trash-", name.to_string_lossy()))
        .unwrap_or_default();
    if let (Some(parent), false) = (parent, prefix.is_empty()) {
        if std::fs::read_dir(parent)
            .map(|entries| {
                entries
                    .flatten()
                    .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            })
            .unwrap_or(false)
        {
            return true;
        }
    }
    // 旧布局：指针在、根目录还躺着一个 exe（迁移没成功）不算残留；
    // 指针没了却还有 exe，说明卸载时删不动
    !current_file().exists() && legacy_binary().exists()
}

/// 启动时的残留清理：半截下载包、解压暂存目录、回收位、没有指针指向的版本目录。
///
/// 只在内核**启动之前**调用（`spawn_worker` 里，autostart 之前）。删不掉的一律
/// 忽略 —— 清理是尽力而为，绝不能因为它失败就影响启动。
pub fn cleanup_residue() {
    let dir = kernel_dir();
    if !dir.exists() {
        return;
    }

    // 半截下载包
    if let Ok(entries) = std::fs::read_dir(downloads_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".part") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    // 版本目录：暂存 / 回收位；有指针时再清不被指针引用的旧版本。
    // **没有有效指针时不动版本目录**：指针丢失（崩溃在 rename 与写指针之间）
    // 时那些目录可能是唯一一份可用的安装，删掉就成了「明明装过却要重下」；
    // 这份自愈由 migrate_legacy 负责（它先跑）。
    let current = read_current().filter(|version| {
        let dir = version_dir(version);
        dir.join(kernel_binary_name()).exists() && dir.join(OK_MARKER).exists()
    });
    if let Ok(entries) = std::fs::read_dir(versions_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_current = current.as_deref() == Some(name.as_str());
            let disposable = name.starts_with(".trash-")
                || name.starts_with(".staging-")
                || name.ends_with(".tmp")
                || (current.is_some() && !is_current);
            if disposable {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    // 同级回收位
    if let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) {
        let prefix = format!("{}.trash-", name.to_string_lossy());
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
}
