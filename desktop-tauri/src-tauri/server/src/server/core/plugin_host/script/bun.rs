use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};

use super::super::process::{self, ExitReport};

pub const VERSION: &str = "1.4.2";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const OUTPUT_BYTES: usize = 8192;

fn last_error() -> &'static Mutex<Option<String>> {
    static ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    ERROR.get_or_init(|| Mutex::new(None))
}

fn asset() -> Result<(&'static str, &'static str), String> {
    let musl = cfg!(target_env = "musl")
        || (cfg!(target_os = "linux")
            && std::fs::read_dir("/lib").ok().is_some_and(|entries| {
                entries
                    .flatten()
                    .any(|entry| entry.file_name().to_string_lossy().starts_with("ld-musl-"))
            }));
    match (std::env::consts::OS, std::env::consts::ARCH, musl) {
        ("windows", "x86_64", _) => Ok((
            "bun-windows-x64-baseline",
            "78c221c2376f79731ccf4e4af0b3bb46d81fefa3296c5abee09ad8a1b21e68c6",
        )),
        ("windows", "aarch64", _) => Ok((
            "bun-windows-aarch64",
            "a7a16b876a305fd1029c66dbd27007b4f6112ae896532f675878731a21e50cfd",
        )),
        ("macos", "x86_64", _) => Ok((
            "bun-darwin-x64",
            "80520d7e17526308c9185d261679ac6d27798d3803a0e9f7ff9121ab8affb012",
        )),
        ("macos", "aarch64", _) => Ok((
            "bun-darwin-aarch64",
            "90987a3a16d7db556d886ac3d551e7b6d3edf0a1cf43acaed622e8676be1d12f",
        )),
        ("linux", "x86_64", false) => Ok((
            "bun-linux-x64-baseline",
            "c678040f14fe0440eb839d37cbd0ce4c051a32da72806ac97de6a6aab6bf728f",
        )),
        ("linux", "aarch64", false) => Ok((
            "bun-linux-aarch64",
            "54328bbc2d9c8e0c9f892c544d66c57a83b84139e34909e5ee81758f1ac8fda7",
        )),
        ("linux", "x86_64", true) => Ok((
            "bun-linux-x64-musl-baseline",
            "76e1db84e98f22f78de0a87e309bfbbf297732847f9720db36750646c85c8c18",
        )),
        ("linux", "aarch64", true) => Ok((
            "bun-linux-aarch64-musl",
            "71760b6c8ea30623b81a4907cb815d48e2ea266f2e73e751534a44a0607950df",
        )),
        _ => Err("Bun 暂无适用于当前系统与处理器的运行时".into()),
    }
}

fn executable() -> &'static str {
    if cfg!(windows) {
        "bun.exe"
    } else {
        "bun"
    }
}
fn installed() -> Result<PathBuf, String> {
    Ok(crate::server::config::config_dir()
        .join("runtimes")
        .join("bun")
        .join(VERSION)
        .join(asset()?.0)
        .join(executable()))
}

pub fn snapshot() -> Value {
    match installed() {
        Ok(path) => {
            let error = last_error().lock().ok().and_then(|saved| saved.clone());
            json!({ "version": VERSION, "installed": path.is_file(), "path": path, "downloaded": true, "error": error })
        }
        Err(error) => json!({ "version": VERSION, "installed": false, "error": error }),
    }
}

/// 下载与清理共用同一把互斥锁：删除运行时目录不能与按需下载并发
fn download_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub async fn ensure() -> Result<PathBuf, String> {
    let _guard = download_lock().lock().await;
    let path = installed()?;
    if path.is_file() {
        validate(&path).await?;
        return Ok(path);
    }
    let (target, checksum) = asset()?;
    super::operations::stage("runtime", "正在下载 Bun 运行时");
    let url =
        format!("https://github.com/oven-sh/bun/releases/download/bun-v{VERSION}/{target}.zip");
    let bytes = super::download::bytes(&url, 200 * 1024 * 1024, Some(checksum)).await?;
    let destination = path.clone();
    tokio::task::spawn_blocking(move || extract(&bytes, target, &destination))
        .await
        .map_err(|error| format!("安装 Bun 任务中断：{error}"))??;
    validate(&path).await?;
    Ok(path)
}

/// 删除整个运行时目录（含全部版本），供启动失败后的重置入口调用，
/// 下次使用脚本插件时按需重新下载，目录不存在视为已清理
pub async fn clean() -> Result<(), String> {
    let _guard = download_lock().lock().await;
    let root = crate::server::config::config_dir()
        .join("runtimes")
        .join("bun");
    tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Bun 运行时目录清理失败：{error}")),
    })
    .await
    .map_err(|error| format!("清理 Bun 运行时任务中断：{error}"))?
}

async fn validate(path: &Path) -> Result<(), String> {
    let result = probe(path).await;
    if let Ok(mut saved) = last_error().lock() {
        *saved = result.as_ref().err().cloned();
    }
    if let Err(error) = &result {
        process::event(
            "error",
            error,
            json!({
                "source": "plugin", "component": "bun", "action": "runtime_check",
                "version": VERSION, "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                "detail": error,
            }),
        );
    }
    result
}

async fn probe(path: &Path) -> Result<(), String> {
    let mut command = tokio::process::Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_remove("BUN_OPTIONS")
        .env_remove("NODE_OPTIONS")
        .env("DO_NOT_TRACK", "1");
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|error| {
        diagnostic(
            "Bun 运行时未能启动",
            path,
            &format!("系统错误：{error}"),
            spawn_hint(&error, path),
            &[],
            &[],
        )
    })?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = tokio::time::timeout(STARTUP_TIMEOUT, async {
        tokio::try_join!(
            child.wait(),
            capture(stdout_pipe, &mut stdout),
            capture(stderr_pipe, &mut stderr),
        )
    })
    .await;
    match result {
        Err(_) => {
            let report = process::reap(&mut child, Duration::ZERO).await;
            Err(diagnostic(
                "Bun 启动检查超时（15 秒）",
                path,
                &report.description(),
                "检查未按时完成；请结合下面的输出排查运行环境，不能据此判断为下载失败。",
                &stdout,
                &stderr,
            ))
        }
        Ok(Err(error)) => {
            let report = process::reap(&mut child, Duration::ZERO).await;
            Err(diagnostic(
                "Bun 启动检查未完成",
                path,
                &format!("{error}；{}", report.description()),
                "读取运行时输出或等待进程退出时发生错误。",
                &stdout,
                &stderr,
            ))
        }
        Ok(Ok((status, (), ()))) => {
            let report = ExitReport {
                status: Some(status),
                ..ExitReport::default()
            };
            if !status.success() {
                return Err(diagnostic(
                    "Bun 运行时启动后异常退出",
                    path,
                    &report.description(),
                    exit_hint(status, &stderr),
                    &stdout,
                    &stderr,
                ));
            }
            if stdout.len() > OUTPUT_BYTES || String::from_utf8_lossy(&stdout).trim() != VERSION {
                return Err(diagnostic(
                    "Bun 版本检查未通过",
                    path,
                    &report.description(),
                    "进程正常退出，但版本输出与预期不一致；请检查运行时文件是否被替换。",
                    &stdout,
                    &stderr,
                ));
            }
            Ok(())
        }
    }
}

async fn capture<R: AsyncRead + Unpin>(pipe: Option<R>, output: &mut Vec<u8>) -> io::Result<()> {
    let Some(mut pipe) = pipe else { return Ok(()) };
    let mut chunk = [0u8; 2048];
    loop {
        let count = pipe.read(&mut chunk).await?;
        if count == 0 {
            return Ok(());
        }
        // 达到保留上限后仍排空管道，避免子进程因输出堵塞而被误判为超时。
        let keep = count.min((OUTPUT_BYTES + 1).saturating_sub(output.len()));
        output.extend_from_slice(&chunk[..keep]);
    }
}

fn diagnostic(
    title: &str,
    path: &Path,
    status: &str,
    hint: &str,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let target = asset().map(|(target, _)| target).unwrap_or("未知");
    let mut lines = vec![
        title.to_string(),
        format!(
            "运行环境：{} / {}；预期 Bun {VERSION}；下载版本：{target}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
        format!(
            "检查命令：{} --version",
            process::redact(&path.to_string_lossy())
        ),
        format!("退出详情：{}", process::redact(status)),
    ];
    if !hint.is_empty() {
        lines.push(hint.to_string());
    }
    for (label, bytes) in [
        ("错误输出（stderr，已脱敏）", stderr),
        ("标准输出（stdout，已脱敏）", stdout),
    ] {
        let text = output_summary(bytes);
        lines.push(format!(
            "{label}：\n{}",
            if text.is_empty() {
                "（无输出）"
            } else {
                &text
            }
        ));
    }
    lines.join("\n")
}

fn output_summary(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(&bytes[..bytes.len().min(OUTPUT_BYTES)]);
    let mut private_key = false;
    let safe = raw
        .lines()
        .map(|line| {
            let start = line.contains("-----BEGIN") && line.contains("PRIVATE KEY-----");
            let end = line.contains("-----END") && line.contains("PRIVATE KEY-----");
            if start || private_key {
                private_key = !end;
                "[私钥内容已隐藏]".to_string()
            } else {
                process::redact(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut summary = safe.chars().take(1600).collect::<String>();
    if bytes.len() > OUTPUT_BYTES || safe.chars().count() > 1600 {
        summary.push_str("\n[其余输出已省略]");
    }
    summary.trim().to_string()
}

fn spawn_hint(error: &io::Error, path: &Path) -> &'static str {
    match error.kind() {
        io::ErrorKind::PermissionDenied => {
            #[cfg(unix)]
            {
                "系统拒绝执行 Bun；请检查运行时文件的执行权限、挂载目录是否禁止执行（noexec）及系统安全策略。"
            }
            #[cfg(not(unix))]
            {
                "系统拒绝执行 Bun；请检查运行时文件的访问权限及安全软件拦截记录。"
            }
        }
        io::ErrorKind::NotFound if path.is_file() => {
            "运行时文件存在，但系统无法加载它；请检查动态加载器及系统兼容性，不一定是下载文件丢失。"
        }
        io::ErrorKind::NotFound => "运行时文件或所在目录已不存在；请检查文件是否被移走或清理。",
        _ => {
            #[cfg(unix)]
            if error.raw_os_error() == Some(libc::ENOEXEC) {
                return "系统无法识别运行时文件格式；请核对系统与处理器架构及文件完整性。";
            }
            #[cfg(windows)]
            if matches!(error.raw_os_error(), Some(193 | 216)) {
                return "运行时文件格式或处理器架构与当前 Windows 不兼容。";
            }
            "请保留系统错误和运行环境信息；目前无法确定具体原因。"
        }
    }
}

fn exit_hint(status: ExitStatus, stderr: &[u8]) -> &'static str {
    #[cfg(target_arch = "x86_64")]
    if !std::is_x86_feature_detected!("sse4.2") {
        return "当前运行环境未提供 SSE4.2，低于 Bun 的最低 CPU 要求；虚拟机用户还需核对分配的 CPU 能力。";
    }
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if (stderr.contains("glibc_") || stderr.contains("glibcxx_")) && stderr.contains("not found") {
        return "系统库版本不满足运行时要求；具体缺少的版本见错误输出。Docker 用户需检查容器内部的系统库。";
    }
    if stderr.contains("error while loading shared libraries")
        || stderr.contains("error loading shared library")
        || stderr.contains("library not loaded:")
    {
        return "动态库加载失败；请按错误输出核对具体库名，不要仅凭此提示安装不相关的依赖。";
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        match status.signal() {
            Some(libc::SIGILL) => return "进程收到 SIGILL（非法指令）；可能涉及 CPU／虚拟化兼容性或运行时自身崩溃，仅凭信号不能确定原因。",
            Some(libc::SIGKILL) => return "进程被 SIGKILL 强制结束；请核对主机或容器日志，不能仅凭此信号认定为内存不足。",
            Some(libc::SIGSEGV) => return "进程收到 SIGSEGV（内存访问异常）；这是运行时崩溃，不等于系统内存不足。",
            Some(libc::SIGABRT) => return "进程收到 SIGABRT（异常终止）；请结合错误输出确认原因。",
            _ => {}
        }
    }
    #[cfg(windows)]
    match status.code().map(|code| code as u32) {
        Some(0xC000001D) => return "Windows 报告非法指令；请结合 CPU／虚拟化环境及错误输出排查，也可能是运行时自身崩溃。",
        Some(0xC0000135) => return "Windows 报告缺少运行所需的 DLL；请结合错误输出和系统事件日志确认具体组件。",
        Some(0xC000007B) => return "Windows 报告程序或依赖库格式无效；请核对文件完整性与处理器架构。",
        Some(0xC0000005) => return "Windows 报告内存访问异常；这是运行时崩溃，不等于系统内存不足。",
        _ => {}
    }
    "请结合退出详情和错误输出排查；尚不能判断是系统环境问题还是 Bun 自身故障。"
}

fn extract(bytes: &[u8], target: &str, destination: &Path) -> Result<(), String> {
    use std::io::{Read, Write};
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let expected = format!("{target}/{}", executable());
    let mut file = zip
        .by_name(&expected)
        .map_err(|_| "Bun 下载包缺少当前平台入口".to_string())?;
    if file.size() > 300 * 1024 * 1024
        || file.is_dir()
        || file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
        return Err("Bun 下载包入口无效".into());
    }
    let directory = destination.parent().ok_or("Bun 安装路径无效")?;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    let copied = std::io::copy(
        &mut file.by_ref().take(300 * 1024 * 1024 + 1),
        &mut temporary,
    )
    .map_err(|error| error.to_string())?;
    if copied > 300 * 1024 * 1024 {
        return Err("Bun 入口超过大小限制".into());
    }
    temporary
        .flush()
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
    }
    temporary
        .persist_noclobber(destination)
        .map_err(|error| error.to_string())?;
    Ok(())
}
