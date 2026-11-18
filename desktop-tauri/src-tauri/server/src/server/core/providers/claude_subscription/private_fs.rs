//! Unix 0700 与 Windows 当前用户专属 DACL：只收紧本模块新建的临时目录。
//! 不改官方账号目录或系统策略；失败即不写入 MCP 回调能力令牌。
use super::errors;
use crate::server::errors::GatewayError;
use std::path::Path;

#[cfg(unix)]
pub(super) fn directory(path: &Path) -> Result<(), GatewayError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| errors::local("claude_private_directory", "无法限制 Claude 暂存目录访问权限"))
}

#[cfg(windows)]
pub(super) fn directory(path: &Path) -> Result<(), GatewayError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{BOOL, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
    struct Local(HLOCAL);
    impl Drop for Local {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(self.0));
            }
        }
    }
    let failed = |_| errors::local("claude_private_directory", "无法限制 Claude 私有暂存目录访问权限，未启动桥接");
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(failed)?;
    let token = Token(token);
    let mut needed = 0;
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    if needed == 0 || needed > 65536 {
        return Err(errors::local("claude_private_directory", "无法确定 Claude 暂存目录所有者"));
    }
    let mut data = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    unsafe { GetTokenInformation(token.0, TokenUser, Some(data.as_mut_ptr().cast()), needed, &mut needed) }
        .map_err(failed)?;
    let user = unsafe { &*data.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) }.map_err(failed)?;
    let _sid = Local(HLOCAL(sid.0.cast()));
    let sid = unsafe { sid.to_string() }
        .map_err(|_| errors::local("claude_private_directory", "无法读取 Claude 暂存目录所有者"))?;
    let descriptor_text = format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)");
    let wide: Vec<u16> = descriptor_text.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(failed)?;
    let _descriptor = Local(HLOCAL(descriptor.0));
    let mut present = BOOL::default();
    let mut defaulted = BOOL::default();
    let mut acl: *mut ACL = std::ptr::null_mut();
    unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }.map_err(failed)?;
    if !present.as_bool() || acl.is_null() {
        return Err(errors::local("claude_private_directory", "Claude 私有目录 ACL 无效"));
    }
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(acl),
            None,
        )
    }
    .ok()
    .map_err(failed)
}

#[cfg(windows)]
pub(super) fn managed_registry_guard() -> Result<(), GatewayError> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
    };
    let path: Vec<u16> = "SOFTWARE\\Policies\\ClaudeCode".encode_utf16().chain(std::iter::once(0)).collect();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let mut key = HKEY::default();
        let result = unsafe { RegOpenKeyExW(hive, PCWSTR(path.as_ptr()), None, KEY_READ, &mut key) };
        if result == ERROR_SUCCESS {
            let _ = unsafe { RegCloseKey(key) };
            return Err(errors::local(
                "claude_managed_policy",
                "存在 Claude 系统托管策略，未启动可能载入额外配置的桥接",
            ));
        }
        if result != ERROR_FILE_NOT_FOUND {
            return Err(errors::local("claude_managed_policy", "无法核对 Claude 系统托管策略，未启动桥接"));
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn directory(_: &Path) -> Result<(), GatewayError> {
    Err(errors::local("claude_private_directory", "此平台尚未实现 Claude 私有目录保护"))
}
