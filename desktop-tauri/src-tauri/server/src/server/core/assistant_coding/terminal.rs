use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use super::{client::CodexClient, config, private_directory, sessions, terminal_gateway::TerminalTicket, terminal_policy, terminal_process::TerminalProcess, CodingError};

pub async fn open(client: Arc<CodexClient>, id: &str) -> Result<Value, CodingError> {
    ensure_desktop()?;
    sessions::action(&client, id, "resume").await?;
    let _operation = client.operations.lock().await;
    client.check_current().await?;
    {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.threads.get(id).is_some_and(|s| s.terminal_pending > 0) { return Err(CodingError::new(409, "官方终端正在打开，请稍候")); }
    }
    let directory = terminal_policy::display_directory(&client, id)?;
    materialize(&client, id).await?;
    let private = private_directory::create()?;
    let ticket = client.terminal_gateway.get().ok_or_else(|| CodingError::new(503, "共享终端入口未就绪"))?.ticket(id)?;
    let launched = launch(&client, id, &directory, &ticket, private.path());
    let mut child = match launched {
        Ok(child) => child,
        Err(error) => { if let Some(gateway) = client.terminal_gateway.get() { gateway.revoke(&ticket.token); } return Err(error); }
    };
    if let Some(thread) = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get_mut(id) {
        thread.terminal_pending += 1; thread.terminal_error = None;
    }
    client.state.changed(id);
    let owner = client.clone();
    let id = id.to_string();
    tokio::spawn(async move {
        let start = Instant::now();
        let mut opened = false;
        loop {
            if ticket.grant.attached.load(Ordering::Acquire) { opened = true; break; }
            if child.exited().ok().flatten().is_some_and(|success| cfg!(windows) || !success) { break; }
            if !owner.running.load(Ordering::Acquire) || start.elapsed() >= Duration::from_secs(100) { break; }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        {
            let mut state = owner.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(thread) = state.threads.get_mut(&id) {
                thread.terminal_pending = thread.terminal_pending.saturating_sub(1);
                if !opened { thread.terminal_error = Some("未收到官方终端的共享连接确认，请检查电脑上的终端窗口".into()); }
            }
        }
        owner.state.changed(&id);
        if !opened {
            if let Some(gateway) = owner.terminal_gateway.get() { gateway.revoke(&ticket.token); }
        }
        let mut process_done = false;
        while owner.running.load(Ordering::Acquire) {
            process_done = child.exited().ok().flatten().is_some();
            if process_done { break; }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if cfg!(windows) && process_done {
            if let Some(gateway) = owner.terminal_gateway.get() { gateway.revoke(&ticket.token); }
        }
        drop(private);
    });
    Ok(json!({ "opening": true }))
}

async fn materialize(client: &CodexClient, id: &str) -> Result<(), CodingError> {
    let (path, branch) = {
        let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        let thread = state.threads.get(id).ok_or_else(|| CodingError::new(404, "共享会话不存在"))?;
        (thread.thread.get("path").and_then(Value::as_str).map(PathBuf::from),
            thread.thread.pointer("/gitInfo/branch").cloned().unwrap_or(Value::Null))
    };
    if path.is_some_and(|path| path.is_file()) { return Ok(()); }
    client.rpc("thread/metadata/update", json!({ "threadId": id, "gitInfo": { "branch": branch } })).await?;
    Ok(())
}

fn ensure_desktop() -> Result<(), CodingError> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::{OpenProcessToken, GetCurrentProcess};
        use windows::Win32::Security::{GetTokenInformation, TokenSessionId, TOKEN_QUERY};
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        let mut token = HANDLE::default();
        let mut session = 0u32;
        let mut size = 0;
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|_| CodingError::new(500, "无法确认桌面会话"))?;
            let result = GetTokenInformation(token, TokenSessionId, Some((&mut session as *mut u32).cast()), 4, &mut size);
            let _ = CloseHandle(token);
            result.map_err(|_| CodingError::new(500, "无法确认桌面会话"))?;
        }
        if session == 0 { return Err(CodingError::new(409, "后台正在 Windows 服务会话运行，无法打开桌面终端；请使用桌面版 Agent2API")); }
    }
    Ok(())
}

fn launch(client: &CodexClient, id: &str, directory: &str, ticket: &TerminalTicket, private: &Path) -> Result<TerminalProcess, CodingError> {
    let executable = config::display_path(&client.executable.program);
    let home = config::display_path(&client.config.home());
    let args = client.executable.args.iter().map(String::as_str)
        .chain(["--remote", ticket.endpoint.as_str(), "--remote-auth-token-env", "AGENT2API_CODEX_TERMINAL_TOKEN", "resume", id]).collect::<Vec<_>>();
    #[cfg(windows)]
    {
        let script = private.join("open.ps1");
        let command_args = args.iter().map(|s| powershell(s)).collect::<Vec<_>>().join(", ");
        let content = format!("$ErrorActionPreference = 'Stop'\n$env:CODEX_HOME = {}\n$env:AGENT2API_CODEX_TERMINAL_TOKEN = {}\nSet-Location -LiteralPath {}\n$argsForCodex = @({})\n& {} @argsForCodex\n$code = $LASTEXITCODE\nRemove-Item Env:AGENT2API_CODEX_TERMINAL_TOKEN -ErrorAction SilentlyContinue\nif ($code -ne 0) {{ Read-Host 'Codex exited. Press Enter to close' }}\nexit $code\n",
            powershell(&home), powershell(&ticket.token), powershell(directory), command_args, powershell(&executable));
        std::fs::write(&script, format!("\u{feff}{content}")).map_err(|_| CodingError::new(500, "无法写入官方终端启动文件"))?;
        super::terminal_process::launch(&script, directory)
    }
    #[cfg(not(windows))]
    {
        let script = private.join("open.command");
        let command_args = args.iter().map(|s| shell(s)).collect::<Vec<_>>().join(" ");
        let content = format!("#!/bin/sh\nexport CODEX_HOME={}\nexport AGENT2API_CODEX_TERMINAL_TOKEN={}\ncd {} || exit 1\nexec {} {}\n",
            shell(&home), shell(&ticket.token), shell(directory), shell(&executable), command_args);
        std::fs::write(&script, content).map_err(|_| CodingError::new(500, "无法写入官方终端启动文件"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).map_err(|_| CodingError::new(500, "无法保护官方终端启动文件"))?;
        }
        #[cfg(target_os = "macos")]
        { Command::new("/usr/bin/open").args(["-a", "Terminal"]).arg(script).spawn().map(TerminalProcess).map_err(|_| CodingError::new(500, "无法打开系统终端")) }
        #[cfg(not(target_os = "macos"))]
        {
            if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() { return Err(CodingError::new(409, "当前后台没有图形桌面，无法在电脑上打开终端窗口")); }
            for (binary, flag) in [("x-terminal-emulator", "-e"), ("gnome-terminal", "--"), ("konsole", "-e"), ("xterm", "-e")] {
                if let Some(path) = find_program(binary) { return Command::new(path).arg(flag).arg(&script).spawn().map(TerminalProcess).map_err(|_| CodingError::new(500, "无法打开系统终端")); }
            }
            Err(CodingError::new(400, "没有找到图形终端，请先安装系统终端"))
        }
    }
}

#[cfg(windows)]
fn powershell(value: &str) -> String { format!("'{}'", value.replace('\'', "''")) }
#[cfg(not(windows))]
fn shell(value: &str) -> String { format!("'{}'", value.replace('\'', "'\\''")) }
#[cfg(not(any(windows, target_os = "macos")))]
fn find_program(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).filter(|p| p.is_absolute())
        .map(|p| p.join(name)).find(|p| p.is_file())
}
