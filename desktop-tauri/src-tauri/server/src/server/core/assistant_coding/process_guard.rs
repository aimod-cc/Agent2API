use tokio::process::Child;
use super::CodingError;

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct ProcessGuard(OwnedHandle);

    impl ProcessGuard {
        pub fn new(child: &Child) -> Result<Self, CodingError> {
            let error = || CodingError::new(500, "无法为 Codex 建立进程回收保护，未启动会话");
            let process = child.raw_handle().ok_or_else(error)?;
            unsafe {
                let job = CreateJobObjectW(None, None).map_err(|_| error())?;
                let owned = OwnedHandle::from_raw_handle(job.0);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(job, JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _, std::mem::size_of_val(&limits) as u32).map_err(|_| error())?;
                AssignProcessToJobObject(job, HANDLE(process)).map_err(|_| error())?;
                Ok(Self(owned))
            }
        }

        pub fn kill(&self) {
            unsafe { let _ = TerminateJobObject(HANDLE(self.0.as_raw_handle()), 1); }
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    pub struct ProcessGuard(libc::pid_t);

    impl ProcessGuard {
        pub fn new(child: &Child) -> Result<Self, CodingError> {
            let id = child.id().ok_or_else(|| CodingError::new(500, "Codex 进程已退出"))?;
            Ok(Self(id as libc::pid_t))
        }

        pub fn kill(&self) {
            unsafe { libc::kill(-self.0, libc::SIGKILL); }
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use super::*;
    pub struct ProcessGuard;
    impl ProcessGuard {
        pub fn new(_: &Child) -> Result<Self, CodingError> { Err(CodingError::new(400, "此系统不支持本机 Codex 进程管理")) }
        pub fn kill(&self) {}
    }
}

pub use platform::ProcessGuard;
