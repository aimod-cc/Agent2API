//! 受控官方 CLI 进程。沿用 Magpie 的参数化启动、私有目录与整棵进程树回收。
//! 不执行 shell、不接收客户端命令、不继承 API 端点、Bun/Node preload 或代理。
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde_json::{json, Value};
use tempfile::{NamedTempFile, TempDir};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use super::{accounts::Link, errors};
use crate::server::core::{
    plugin_host::process_guard::ProcessGuard,
    proxies::{self, ResolvedProxy},
};
use crate::server::errors::GatewayError;

pub(super) const MAX_LINE: usize = 32 * 1024 * 1024;
pub(super) const COMMAND_TIMEOUT: Duration = Duration::from_secs(25);

pub(super) fn token() -> Result<String, GatewayError> {
    let mut bytes = [0_u8; 24];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| errors::local("claude_random_unavailable", "无法生成 Claude 会话随机标识"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub(super) fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn home() -> Result<PathBuf, GatewayError> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| errors::local("claude_home_missing", "网关进程没有可用的本机用户目录"))
}

pub(super) fn executable(chosen: &str) -> Result<PathBuf, GatewayError> {
    let candidates: Vec<PathBuf> = if !chosen.trim().is_empty() {
        let path = PathBuf::from(chosen.trim());
        if !path.is_absolute() {
            return Err(GatewayError::bad_request("Claude Code 路径必须是原生程序的绝对路径"));
        }
        vec![path]
    } else {
        let filename = if cfg!(windows) { "claude.exe" } else { "claude" };
        let mut paths = Vec::new();
        if let Ok(home) = home() {
            paths.push(home.join(".local/bin").join(filename));
        }
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path).filter(|p| p.is_absolute()).map(|p| p.join(filename)));
        }
        paths
    };
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let path = std::fs::canonicalize(path)
            .map_err(|_| errors::local("claude_cli_missing", "无法访问所选 Claude Code 程序"))?;
        // 原生安装不需要 shell/node/bun；拒绝 .cmd、脚本和任意封装器。
        let mut signature = [0_u8; 4];
        use std::io::Read;
        if std::fs::File::open(&path).and_then(|mut f| f.read_exact(&mut signature)).is_err() {
            continue;
        }
        let native = if cfg!(windows) {
            signature[..2] == *b"MZ"
        } else {
            signature == *b"\x7fELF"
                || matches!(
                    signature,
                    [0xcf, 0xfa, 0xed, 0xfe]
                        | [0xce, 0xfa, 0xed, 0xfe]
                        | [0xfe, 0xed, 0xfa, 0xcf]
                        | [0xca, 0xfe, 0xba, 0xbe]
                        | [0xca, 0xfe, 0xba, 0xbf]
                )
        };
        if native {
            return Ok(path);
        }
    }
    Err(errors::local(
        "claude_cli_missing",
        "未找到 Claude Code 原生程序。请先安装官方原生 CLI；本入口不运行 npm 脚本、cmd 包装或 WSL",
    ))
}

pub(super) fn private_dir() -> Result<TempDir, GatewayError> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("agent2api-claude-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let dir = builder.tempdir().map_err(|_| errors::local("claude_temp_failed", "无法创建 Claude 私有暂存目录"))?;
    super::private_fs::directory(dir.path())?;
    Ok(dir)
}

pub(super) fn private_file(dir: &Path, data: &[u8]) -> Result<NamedTempFile, GatewayError> {
    let mut file =
        NamedTempFile::new_in(dir).map_err(|_| errors::local("claude_temp_failed", "无法创建 Claude 暂存文件"))?;
    file.write_all(data)
        .and_then(|_| file.flush())
        .map_err(|_| errors::local("claude_temp_failed", "无法写入 Claude 暂存文件"))?;
    Ok(file)
}

/// 与 Magpie interactiveArgs 同样的明确许可：仅桥接 MCP 工具，而非跳过权限。
pub(super) fn safe_args(mcp_path: Option<&Path>, disable_commands: bool) -> Vec<String> {
    let settings = json!({ "disableAllHooks": true, "disableDeepLinkRegistration": "disable", "enabledPlugins": {} });
    let mut args = vec![
        "--tools".into(),
        String::new(),
        "--strict-mcp-config".into(),
        "--setting-sources".into(),
        String::new(),
        "--settings".into(),
        settings.to_string(),
        "--permission-mode".into(),
        "default".into(),
        "--no-chrome".into(),
        "--no-session-persistence".into(),
    ];
    if disable_commands {
        args.push("--disable-slash-commands".into());
    }
    args.extend([
        "--mcp-config".into(),
        mcp_path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "{\"mcpServers\":{}}".into()),
    ]);
    args
}

/// 托管策略优先于命令行；不靠篡改策略绕过。存在时拒绝在此运行非交互桥接。
fn managed_guard() -> Result<(), GatewayError> {
    #[cfg(windows)]
    super::private_fs::managed_registry_guard()?;
    let roots: Vec<PathBuf> = if cfg!(windows) {
        [
            std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| "C:/ProgramData".into()),
            std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| "C:/Program Files".into()),
        ]
        .into_iter()
        .map(|p| p.join("ClaudeCode"))
        .collect()
    } else if cfg!(target_os = "macos") {
        vec!["/Library/Application Support/ClaudeCode".into()]
    } else {
        vec!["/etc/claude-code".into()]
    };
    for root in roots {
        for name in ["managed-settings.json", "managed-mcp.json", "managed-settings.d"] {
            match std::fs::symlink_metadata(root.join(name)) {
                Ok(_) => {
                    return Err(errors::local(
                        "claude_managed_policy",
                        "检测到 Claude 托管配置；当前桥接无法保证只加载客户端工具，未启动 CLI",
                    ))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(errors::local("claude_managed_policy", "无法检查 Claude 托管配置，未启动 CLI")),
            }
        }
    }
    Ok(())
}

fn clean_env(command: &mut Command, link: &Link, proxy: Option<&ResolvedProxy>) -> Result<(), GatewayError> {
    // OS 与证书相关白名单；不继承用户程序注入和 Claude/Bun/Node 特殊环境变量。
    let names = [
        "PATH",
        "HOME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
    ];
    let env: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(key, _)| names.iter().any(|name| key.to_string_lossy().eq_ignore_ascii_case(name)))
        .collect();
    command.env_clear().envs(env);
    if let Some(dir) = &link.config_dir {
        command.env("CLAUDE_CONFIG_DIR", dir);
    }
    command
        .env("ENABLE_CLAUDEAI_MCP_SERVERS", "0")
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")
        .env("CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("DISABLE_ERROR_REPORTING", "1")
        .env("DISABLE_AUTOUPDATER", "1")
        .env("NO_PROXY", "localhost,127.0.0.1,::1")
        .env("no_proxy", "localhost,127.0.0.1,::1");
    if let Some(proxy) = proxy {
        let proxy = proxies::chain::endpoint(proxy)
            .map_err(|_| errors::local("claude_proxy_invalid", "无法解析所选 Claude 代理，未回退直连"))?;
        if !matches!(proxy.protocol.as_str(), "http" | "https") {
            return Err(errors::local(
                "claude_proxy_unsupported",
                "Claude 原生 CLI 只接受 HTTP/HTTPS 代理；请为此账号选择 HTTP 出口，未回退直连",
            ));
        }
        let mut url = url::Url::parse("http://localhost").map_err(|_| errors::protocol("无法建立 Claude 代理地址"))?;
        let port = proxy
            .port
            .filter(|p| *p > 0)
            .ok_or_else(|| errors::local("claude_proxy_invalid", "Claude 代理缺少有效端口"))?;
        url.set_scheme(&proxy.protocol).map_err(|_| errors::local("claude_proxy_invalid", "Claude 代理协议无效"))?;
        url.set_host(Some(proxy.host.trim_matches(['[', ']'])))
            .map_err(|_| errors::local("claude_proxy_invalid", "Claude 代理主机无效"))?;
        url.set_port(Some(port)).map_err(|_| errors::local("claude_proxy_invalid", "Claude 代理端口无效"))?;
        url.set_username(&proxy.username).map_err(|_| errors::local("claude_proxy_invalid", "Claude 代理认证无效"))?;
        if !proxy.password.is_empty() {
            url.set_password(Some(&proxy.password))
                .map_err(|_| errors::local("claude_proxy_invalid", "Claude 代理认证无效"))?;
        }
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            command.env(name, url.as_str());
        }
    }
    Ok(())
}

pub(super) struct Running {
    child: Child,
    guard: ProcessGuard,
    pub input: ChildStdin,
    pub output: LineReader,
    stderr: tokio::task::JoinHandle<()>,
}

impl Running {
    pub async fn send(&mut self, value: &Value) -> Result<(), GatewayError> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| errors::protocol("Claude 输入无法序列化"))?;
        if bytes.len() > MAX_LINE {
            return Err(GatewayError::bad_request("Claude 单条输入超过 32MiB 限制"));
        }
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(15), self.input.write_all(&bytes))
            .await
            .map_err(|_| errors::protocol("Claude Code 输入写入超时"))?
            .map_err(|_| errors::protocol("Claude Code 在接收输入前退出"))
    }

    pub fn check_alive(&mut self) -> Result<(), GatewayError> {
        match self.child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err(errors::protocol("Claude Code 已退出，不能保留或继续此会话")),
            Err(_) => Err(errors::protocol("无法检查 Claude Code 子进程状态")),
        }
    }

    pub async fn stop(&mut self) -> Result<(), GatewayError> {
        self.guard.kill();
        let _ = self.child.start_kill();
        self.stderr.abort();
        tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .map_err(|_| errors::local("claude_reap_failed", "Claude 子进程未及时退出，未报告本次请求成功"))?
            .map_err(|_| errors::local("claude_reap_failed", "无法确认 Claude 子进程已回收"))?;
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.guard.kill();
        let _ = self.child.start_kill();
        self.stderr.abort();
    }
}

pub(super) fn spawn(
    link: &Link,
    args: &[String],
    work: &Path,
    proxy: Option<&ResolvedProxy>,
) -> Result<Running, GatewayError> {
    managed_guard()?;
    let executable = executable(
        link.executable.to_str().ok_or_else(|| errors::local("claude_cli_missing", "Claude Code 程序路径不可编码"))?,
    )?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(work)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    clean_env(&mut command, link, proxy)?;
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child =
        command.spawn().map_err(|_| errors::local("claude_cli_start_failed", "无法启动所选 Claude Code 原生程序"))?;
    let guard = match ProcessGuard::new(&child) {
        Ok(guard) => guard,
        Err(_) => {
            let _ = child.start_kill();
            return Err(errors::local("claude_process_guard", "无法建立 Claude 进程树回收保护，未发送请求"));
        }
    };
    let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let (Some(input), Some(output), Some(mut stderr)) = pipes else {
        guard.kill();
        let _ = child.start_kill();
        return Err(errors::protocol("Claude 没有完整的输入输出管道，已终止进程树"));
    };
    let stderr = crate::spawn_task(async move {
        let mut buffer = [0_u8; 8192];
        while let Ok(n) = stderr.read(&mut buffer).await {
            if n == 0 {
                break;
            }
        }
    });
    Ok(Running {
        child,
        guard,
        input,
        output: LineReader { inner: BufReader::new(output), partial: Vec::new() },
        stderr,
    })
}

/// partial 位于 reader 而非 next future 内；select! 的 tick/工具回调取消一次读取时
/// 不会丢掉已经消费的半条 NDJSON（AsyncBufReadExt::read_line 本身不保证这一点）。
pub(super) struct LineReader {
    inner: BufReader<ChildStdout>,
    partial: Vec<u8>,
}
impl LineReader {
    pub async fn next(&mut self) -> Result<Option<Vec<u8>>, GatewayError> {
        loop {
            let buffer = self.inner.fill_buf().await.map_err(|_| errors::protocol("Claude 输出读取中断"))?;
            if buffer.is_empty() {
                return if self.partial.is_empty() {
                    Ok(None)
                } else {
                    Err(errors::protocol("Claude 输出末行被截断"))
                };
            }
            let end = buffer.iter().position(|b| *b == b'\n');
            let count = end.map_or(buffer.len(), |n| n + 1);
            if self.partial.len().saturating_add(count) > MAX_LINE {
                return Err(errors::protocol("Claude 输出超过单帧大小限制"));
            }
            self.partial.extend_from_slice(&buffer[..count]);
            self.inner.consume(count);
            if end.is_some() {
                return Ok(Some(std::mem::take(&mut self.partial)));
            }
        }
    }
}

pub(super) async fn verify_version(link: &Link, proxy: Option<&ResolvedProxy>) -> Result<String, GatewayError> {
    let output = capture(link, &["--version".into()], proxy).await?;
    let text =
        std::str::from_utf8(&output).map_err(|_| errors::local("claude_cli_version", "无法识别 Claude Code 版本"))?;
    let version = text.split_whitespace().next().unwrap_or("");
    let parts: Vec<u32> = version
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| errors::local("claude_cli_version", "所选程序没有返回有效的 Claude Code 版本"))?;
    // 参考已覆盖的原生版本范围；旧版 strict MCP/非交互权限行为不能安全沿用。
    if parts.len() != 3 || (parts[0], parts[1], parts[2]) < (2, 1, 259) || !text.contains("Claude Code") {
        return Err(errors::local("claude_cli_version", "请升级官方原生 Claude Code 至 2.1.259 或更新版本再关联"));
    }
    Ok(version.into())
}

pub(super) async fn capture(
    link: &Link,
    args: &[String],
    proxy: Option<&ResolvedProxy>,
) -> Result<Vec<u8>, GatewayError> {
    let work = private_dir()?;
    let mut run = spawn(link, args, work.path(), proxy)?;
    run.input.shutdown().await.map_err(|_| errors::protocol("Claude 输入管道关闭失败"))?;
    let collect = async {
        let mut bytes = Vec::new();
        (&mut run.output.inner)
            .take((MAX_LINE + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| errors::protocol("Claude 输出读取失败"))?;
        if bytes.len() > MAX_LINE {
            return Err(errors::protocol("Claude 输出超过大小限制"));
        }
        let status = run.child.wait().await.map_err(|_| errors::protocol("Claude Code 退出状态不可用"))?;
        if !status.success() {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                return Err(errors::upstream(&value));
            }
            return Err(errors::local(
                "claude_cli_command_failed",
                "Claude Code 命令未成功完成，请在官方 CLI 中检查登录和版本",
            ));
        }
        Ok(bytes)
    };
    let result = tokio::time::timeout(COMMAND_TIMEOUT, collect)
        .await
        .map_err(|_| errors::local("claude_cli_timeout", "Claude Code 命令超时，已终止子进程"));
    let stopped = run.stop().await;
    stopped?;
    result?
}
