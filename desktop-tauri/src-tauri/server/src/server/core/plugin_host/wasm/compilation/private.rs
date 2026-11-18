use std::path::Path;

// Wasmtime 缓存包含机器码；只读写主程序专属目录，不使用插件包提供的预编译文件。
pub(super) fn directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err("编译缓存路径不是普通目录".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    protect(path)
}

#[cfg(unix)]
fn protect(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn protect(path: &Path) -> Result<(), String> {
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
        fn drop(&mut self) { let _ = unsafe { CloseHandle(self.0) }; }
    }
    struct Local(HLOCAL);
    impl Drop for Local {
        fn drop(&mut self) { unsafe { let _ = LocalFree(Some(self.0)); } }
    }
    let failed = |_| "无法限制 Wasm 编译缓存目录访问权限".to_string();
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(failed)?;
    let token = Token(token);
    let mut needed = 0;
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    if needed == 0 || needed > 65536 { return Err("无法确定 Wasm 编译缓存目录所有者".into()); }
    let mut data = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    unsafe { GetTokenInformation(token.0, TokenUser, Some(data.as_mut_ptr().cast()), needed, &mut needed) }
        .map_err(failed)?;
    let user = unsafe { &*data.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) }.map_err(failed)?;
    let _sid = Local(HLOCAL(sid.0.cast()));
    let sid = unsafe { sid.to_string() }.map_err(|_| "无法读取 Wasm 编译缓存目录所有者".to_string())?;
    let text = format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)");
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(wide.as_ptr()), SDDL_REVISION_1, &mut descriptor, None)
    }.map_err(failed)?;
    let _descriptor = Local(HLOCAL(descriptor.0));
    let mut present = BOOL::default();
    let mut defaulted = BOOL::default();
    let mut acl: *mut ACL = std::ptr::null_mut();
    unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }.map_err(failed)?;
    if !present.as_bool() || acl.is_null() { return Err("Wasm 编译缓存目录访问权限无效".into()); }
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        SetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, None, None, Some(acl), None)
    }.ok().map_err(failed)
}

#[cfg(not(any(unix, windows)))]
fn protect(_: &Path) -> Result<(), String> {
    Err("此平台尚未实现 Wasm 编译缓存目录保护".into())
}
