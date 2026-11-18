//! 只调用固定 Known Folder API，不读 APPDATA/HOME，不创建目录或加载其他用户配置。
//! https://learn.microsoft.com/windows/win32/api/shlobj_core/nf-shlobj_core-shgetknownfolderpath
//! https://learn.microsoft.com/windows/win32/api/shlobj_core/ne-shlobj_core-known_folder_flag

use std::ffi::{c_void, OsString};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use ::windows::core::{Owned, GUID, HRESULT};
use ::windows::Win32::Foundation::{HANDLE, ERROR_NO_TOKEN};
use ::windows::Win32::Security::TOKEN_QUERY;
use ::windows::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

use super::ReadError;

const ROAMING: GUID = GUID::from_u128(0x3eb685db_65f9_4cf6_a03a_e3ef65729f3d);
const DONT_VERIFY: u32 = 0x0000_4000;
const DEFAULT_PATH: u32 = 0x0000_0400;
const MAX_PATH_UNITS: usize = 4096;

// Win32_UI_Shell / Win32_System_Com 未作为服务器直接特性声明；只绑定这两个公开、固定 API。
// 签名逐字核对 windows 0.61.3 生成绑定：REFKNOWNFOLDERID/DWORD/HANDLE/PWSTR* → HRESULT。
#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetKnownFolderPath(folder: *const GUID, flags: u32, token: *mut c_void, path: *mut *mut u16) -> i32;
}
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(memory: *const c_void);
}

struct ShellPath(*mut u16);
impl Drop for ShellPath {
    fn drop(&mut self) {
        // SAFETY: 指针只来自 SHGetKnownFolderPath；成功或失败均由本对象唯一释放，NULL 允许。
        unsafe { CoTaskMemFree(self.0.cast()) };
    }
}

/// DPAPI 与 Known Folder 必须针对同一当前进程用户；不在模拟其他用户的线程上运行。
pub(super) fn ensure_process_user() -> Result<(), ReadError> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentThread 返回当前线程伪句柄；token 指向有效出参。
    match unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token) } {
        Ok(()) => {
            // SAFETY: 成功返回的 token 为本次拥有的真实句柄，Owned 唯一关闭。
            let _token = unsafe { Owned::new(token) };
            Err(ReadError::UnsupportedLayout)
        }
        Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_TOKEN.0) => Ok(()),
        Err(_) => Err(ReadError::ReadFailed),
    }
}

pub(super) fn roaming() -> Result<PathBuf, ReadError> {
    ensure_process_user()?;
    // 不使用 CREATE/INIT；DONT_VERIFY 避免由 Shell 访问或创建目标目录。
    let current = known_folder(DONT_VERIFY)?;
    let default = known_folder(DONT_VERIFY | DEFAULT_PATH)?;
    // 只支持未做 Known Folder 重定向的当前用户默认布局。拒绝而不是改读默认的旧目录。
    if current != default { return Err(ReadError::UnsupportedLayout); }
    Ok(current)
}

fn known_folder(flags: u32) -> Result<PathBuf, ReadError> {
    let mut output = ShellPath(std::ptr::null_mut());
    // SAFETY: 固定有效 GUID，NULL hToken 表示当前用户；出参存活，ShellPath 保证释放。
    let result = unsafe { SHGetKnownFolderPath(&ROAMING, flags, std::ptr::null_mut(), &mut output.0) };
    if result < 0 || output.0.is_null() { return Err(ReadError::UnsupportedLayout); }
    let mut length = 0usize;
    while length < MAX_PATH_UNITS {
        // SAFETY: API 成功保证返回 NUL 结尾 UTF-16 字符串；遇到终止符即停，不跨过它读取。
        if unsafe { *output.0.add(length) } == 0 { break; }
        length += 1;
    }
    if length == 0 || length == MAX_PATH_UNITS { return Err(ReadError::UnsupportedLayout); }
    // SAFETY: 上面的逐字符遍历已确认此范围位于 API 返回的有效字符串内。
    let units = unsafe { std::slice::from_raw_parts(output.0, length) };
    if units.iter().any(|unit| *unit < 0x20) { return Err(ReadError::UnsupportedLayout); }
    Ok(PathBuf::from(OsString::from_wide(units)))
}
