#[cfg(windows)]
use std::path::Path;
use super::CodingError;
#[cfg(windows)]
use super::config;

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{CreateProcessW, GetExitCodeProcess, WaitForSingleObject,
        CREATE_NEW_CONSOLE, PROCESS_INFORMATION, STARTUPINFOW};

    pub struct TerminalProcess(OwnedHandle);

    impl TerminalProcess {
        pub fn exited(&mut self) -> Result<Option<bool>, CodingError> {
            let handle = HANDLE(self.0.as_raw_handle());
            match unsafe { WaitForSingleObject(handle, 0) } {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    unsafe { GetExitCodeProcess(handle, &mut code) }.map_err(|_| CodingError::new(500, "读取终端退出状态失败"))?;
                    Ok(Some(code == 0))
                }
                _ => Err(CodingError::new(500, "无法确认终端进程状态")),
            }
        }
    }

    pub fn launch(script: &Path, directory: &str) -> Result<TerminalProcess, CodingError> {
        let system = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:/Windows".into());
        let executable = config::display_path(&system.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
        let command = format!("\"{executable}\" -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"{}\"", config::display_path(script));
        let wide = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let application = wide(&executable);
        let mut command = wide(&command);
        let cwd = wide(directory);
        let startup = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut process = PROCESS_INFORMATION::default();
        unsafe {
            // GUI parents have no console handles; let the new console create its own.
            CreateProcessW(PCWSTR(application.as_ptr()), Some(PWSTR(command.as_mut_ptr())), None, None, false,
                CREATE_NEW_CONSOLE, None, PCWSTR(cwd.as_ptr()), &startup, &mut process)
                .map_err(|_| CodingError::new(500, "无法打开官方 Codex 终端窗口"))?;
            let _ = CloseHandle(process.hThread);
            Ok(TerminalProcess(OwnedHandle::from_raw_handle(process.hProcess.0)))
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    pub struct TerminalProcess(pub std::process::Child);
    impl TerminalProcess {
        pub fn exited(&mut self) -> Result<Option<bool>, CodingError> {
            self.0.try_wait().map(|status| status.map(|s| s.success())).map_err(|_| CodingError::new(500, "无法确认终端进程状态"))
        }
    }
}

pub use platform::TerminalProcess;
#[cfg(windows)]
pub use platform::launch;
