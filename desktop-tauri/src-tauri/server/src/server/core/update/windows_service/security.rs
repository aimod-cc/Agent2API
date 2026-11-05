use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, Prefix};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{ConvertStringSidToSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::Security::{
    EqualSid, GetAce, GetTokenInformation, IsWellKnownSid, ACCESS_ALLOWED_ACE, ACL,
    DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    TOKEN_QUERY, TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows::Win32::System::JobObjects::IsProcessInJob;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::server::core::update::UpdateError;

pub(super) struct Handle(pub HANDLE);
impl Drop for Handle {
    fn drop(&mut self) { let _ = unsafe { CloseHandle(self.0) }; }
}

struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) { unsafe { let _ = LocalFree(Some(HLOCAL(self.0.0))); } }
}

pub(super) fn error(value: impl std::fmt::Display) -> UpdateError {
    UpdateError::new(format!("Windows 服务更新失败：{value}"))
}

pub(super) fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

pub(super) fn is_system() -> Result<bool, UpdateError> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(error)?;
    let token = Handle(token);
    let mut required = 0;
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut required) };
    if required == 0 || required > 65536 { return Err(error("无法读取服务账号身份")); }
    let mut buffer = vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
    unsafe {
        GetTokenInformation(token.0, TokenUser, Some(buffer.as_mut_ptr().cast()), required, &mut required)
            .map_err(error)?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        Ok(IsWellKnownSid(user.User.Sid, WinLocalSystemSid).as_bool())
    }
}

pub(super) fn in_job() -> Result<bool, UpdateError> {
    let mut value = windows::core::BOOL::default();
    unsafe { IsProcessInJob(GetCurrentProcess(), None, &mut value) }.map_err(error)?;
    Ok(value.as_bool())
}

pub(super) fn no_links(path: &Path) -> Result<(), UpdateError> {
    if !path.is_absolute() { return Err(error("更新路径必须是绝对路径")); }
    match path.components().next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => {}
        _ => return Err(error("更新路径必须位于本机磁盘，不支持网络共享或设备路径")),
    }
    for part in path.ancestors() {
        let metadata = std::fs::symlink_metadata(part).map_err(error)?;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(error("更新路径不能包含目录链接或重解析点"));
        }
    }
    Ok(())
}

pub(super) fn within(path: &Path, directory: &Path) -> bool {
    let normalize = |path: &Path| std::fs::canonicalize(path).ok()
        .map(|path| path.to_string_lossy().to_lowercase());
    match (normalize(path), normalize(directory)) {
        (Some(path), Some(directory)) => Path::new(&path).starts_with(directory),
        _ => false,
    }
}

/// 拒绝非管理员可修改的安装与升级目录；不通过修改整台机器的权限来强行放行。
pub(super) fn protected(path: &Path) -> Result<(), UpdateError> {
    no_links(path)?;
    let installer_text = wide(std::ffi::OsStr::new("S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"));
    let mut installer = PSID::default();
    unsafe { ConvertStringSidToSidW(PCWSTR(installer_text.as_ptr()), &mut installer) }.map_err(error)?;
    let result = (|| {
        check_acl(path, false, installer)?;
        for parent in path.ancestors().skip(1) { check_acl(parent, true, installer)?; }
        Ok(())
    })();
    unsafe { let _ = LocalFree(Some(HLOCAL(installer.0))); }
    result
}

fn check_acl(path: &Path, ancestor: bool, installer: PSID) -> Result<(), UpdateError> {
    let name = wide(path.as_os_str());
    let mut owner = PSID::default();
    let mut acl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let result = unsafe {
        GetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION, Some(&mut owner), None,
            Some(&mut acl), None, &mut descriptor)
    };
    result.ok().map_err(error)?;
    let _descriptor = Descriptor(descriptor);
    let trusted = |sid: PSID| !sid.0.is_null()
        && (trusted_sid(sid) || (ancestor && unsafe { EqualSid(sid, installer) }.is_ok()));
    if !trusted(owner) || acl.is_null() { return Err(error("目录所有者或访问权限不安全")); }
    // 父目录的 DELETE_CHILD 可以绕过子项 ACL；只忽略仅向后代继承的规则。
    const REPLACE_ACCESS: u32 = 0x1000_0000 | 0x0004_0000 | 0x0008_0000 | 0x0001_0000 | 0x0040;
    const WRITE_ACCESS: u32 = REPLACE_ACCESS | 0x4000_0000 | 0x0002 | 0x0004 | 0x0010 | 0x0100;
    let forbidden = if ancestor {
        if path.parent().is_none() { REPLACE_ACCESS & !0x0001_0000 } else { REPLACE_ACCESS }
    } else { WRITE_ACCESS };
    for index in 0..unsafe { (*acl).AceCount } {
        let mut raw = std::ptr::null_mut();
        unsafe { GetAce(acl, u32::from(index), &mut raw) }.map_err(error)?;
        let header = unsafe { &*raw.cast::<windows::Win32::Security::ACE_HEADER>() };
        if header.AceType == 1 || header.AceFlags & 0x08 != 0 { continue; }
        if header.AceType != 0 { return Err(error("更新目录使用了不受支持的权限规则")); }
        let ace = raw.cast::<ACCESS_ALLOWED_ACE>();
        let sid = PSID(unsafe { std::ptr::addr_of_mut!((*ace).SidStart).cast() });
        if unsafe { (*ace).Mask } & forbidden != 0 && !trusted(sid) {
            return Err(error("安装目录或其上级目录允许普通账号改写，不能安全执行一键更新"));
        }
    }
    Ok(())
}

fn trusted_sid(sid: PSID) -> bool {
    !sid.0.is_null() && unsafe {
        IsWellKnownSid(sid, WinLocalSystemSid).as_bool()
            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid).as_bool()
    }
}

pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| std::fs::canonicalize(path).ok()
        .map(|path| path.to_string_lossy().to_lowercase());
    match (normalize(left), normalize(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}
