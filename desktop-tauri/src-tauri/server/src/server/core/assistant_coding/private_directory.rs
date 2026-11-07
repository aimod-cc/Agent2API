use std::path::Path;
use tempfile::TempDir;
use super::CodingError;

pub fn create() -> Result<TempDir, CodingError> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("agent2api-codex-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir().map_err(|_| CodingError::new(500, "无法创建 Codex 终端私有目录"))?;
    protect(directory.path())?;
    Ok(directory)
}

#[cfg(not(windows))]
fn protect(_: &Path) -> Result<(), CodingError> { Ok(()) }

#[cfg(windows)]
fn protect(path: &Path) -> Result<(), CodingError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCWSTR, PWSTR, BOOL};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER, ACL,
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let failure = || CodingError::new(500, "无法保护 Codex 终端临时凭证，未打开终端");
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|_| failure())?;
        let result = (|| {
            let mut size = 0;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
            if size == 0 || size > 65536 { return Err(failure()); }
            let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
            GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), size, &mut size).map_err(|_| failure())?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut sid = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut sid).map_err(|_| failure())?;
            let sid_text = sid.to_string().map_err(|_| failure());
            let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
            let descriptor_text: Vec<u16> = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{})", sid_text?).encode_utf16().chain(Some(0)).collect();
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(descriptor_text.as_ptr()), 1, &mut descriptor, None).map_err(|_| failure())?;
            let applied = (|| {
                let mut present = BOOL::default();
                let mut defaulted = BOOL::default();
                let mut acl: *mut ACL = std::ptr::null_mut();
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted).map_err(|_| failure())?;
                if !present.as_bool() || acl.is_null() { return Err(failure()); }
                let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                SetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, None, None, Some(acl), None).ok().map_err(|_| failure())
            })();
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            applied
        })();
        let _ = CloseHandle(token);
        result
    }
}
