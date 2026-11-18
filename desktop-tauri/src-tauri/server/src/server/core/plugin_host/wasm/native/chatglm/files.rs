//! 逐级打开并保留祖先句柄，不靠 exists/canonicalize 后另开文件猜同一来源。
//! https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-createfilew
//! https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew
//! FILE_SHARE_READ 排除正常写/删句柄；不声称阻止内核、设备或绕过文件共享的写入。

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle};
use std::path::{Component, Path, PathBuf, Prefix};

use ::windows::Win32::Foundation::HANDLE;
use ::windows::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetFileInformationByHandle, GetFileType, GetFinalPathNameByHandleW,
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OPEN_NO_RECALL,
    FILE_GENERIC_READ, FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
    FILE_TYPE_DISK, GETFINALPATHNAMEBYHANDLE_FLAGS, VOLUME_NAME_DOS,
};
use ::windows::core::PCWSTR;
use zeroize::Zeroizing;

use super::{folders, ReadError};

const MAX_LOCAL_STATE: u64 = 1024 * 1024;
const MAX_COOKIE_DB: u64 = 16 * 1024 * 1024;
const MAX_PATH_UNITS: usize = 4096;
const DRIVE_FIXED: u32 = 3; // Win32 GetDriveTypeW 的公开常量。

pub(super) struct Inputs {
    pub local_state: Zeroizing<Vec<u8>>,
    pub cookies: Zeroizing<Vec<u8>>,
    guard: Guard,
}

struct Guard {
    // 保留全部祖先；不允许逐级检查后关闭，避免后续父目录被换成链接。
    _directories: Vec<Pinned>,
    local_state: Pinned,
    cookies: Pinned,
    network: PathBuf,
}

struct Pinned {
    file: File,
    path: PathBuf,
    stamp: Stamp,
}

#[derive(PartialEq, Eq)]
struct Stamp {
    attributes: u32,
    volume: u32,
    index: u64,
    size: u64,
    links: u32,
    created: u64,
    written: u64,
}

impl Inputs {
    pub(super) fn check(&self) -> Result<(), ReadError> { self.guard.check() }
}

pub(super) fn read() -> Result<Inputs, ReadError> {
    let roaming = folders::roaming()?;
    let mut guard = Guard::open(&roaming)?;
    guard.check()?;
    let local_state = guard.local_state.read_bounded(MAX_LOCAL_STATE)?;
    let cookies = guard.cookies.read_bounded(MAX_COOKIE_DB)?;
    guard.check()?;
    Ok(Inputs { local_state, cookies, guard })
}

impl Guard {
    fn open(roaming: &Path) -> Result<Self, ReadError> {
        let mut directories = Vec::new();
        let mut current = PathBuf::new();
        let mut saw_drive = false;
        for component in roaming.components() {
            match component {
                Component::Prefix(prefix) => match prefix.kind() {
                    Prefix::Disk(letter) if letter.is_ascii_alphabetic() => {
                        current.push(prefix.as_os_str());
                        saw_drive = true;
                    }
                    _ => return Err(ReadError::UnsupportedLayout),
                },
                Component::RootDir if saw_drive => {
                    current.push(component.as_os_str());
                    let root = wide(&current)?;
                    // SAFETY: root 为有效 NUL 结尾 UTF-16 盘根，不探测其它盘或网络路径。
                    if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
                        return Err(ReadError::UnsupportedLayout);
                    }
                    directories.push(Pinned::open(&current, true, None)?);
                }
                Component::Normal(name) if !directories.is_empty() => {
                    safe_name(name)?;
                    current.push(name);
                    directories.push(Pinned::open(&current, true, None)?);
                }
                _ => return Err(ReadError::UnsupportedLayout),
            }
        }
        if directories.len() < 2 { return Err(ReadError::UnsupportedLayout); }
        // 已有原生证据唯一确证的目录；不尝试“智谱清言”/ZPHZ/HOME 等兜底。
        current.push("chatglm");
        directories.push(Pinned::open(&current, true, None)?);
        let local_state = Pinned::open(&current.join("Local State"), false, Some(MAX_LOCAL_STATE))?;
        current.push("Network");
        directories.push(Pinned::open(&current, true, None)?);
        let cookies = Pinned::open(&current.join("Cookies"), false, Some(MAX_COOKIE_DB))?;
        let guard = Self { _directories: directories, local_state, cookies, network: current };
        guard.check()?;
        Ok(guard)
    }

    fn check(&self) -> Result<(), ReadError> {
        for directory in &self._directories { directory.check()?; }
        self.local_state.check()?;
        self.cookies.check()?;
        // 不读 sidecar，不删、不恢复、不拷贝；只要存在就拒绝整个快照。
        for name in ["Cookies-wal", "Cookies-shm", "Cookies-journal"] {
            match Pinned::open(&self.network.join(name), false, Some(MAX_COOKIE_DB)) {
                Err(ReadError::NotFound) => {}
                Err(error) => return Err(error),
                Ok(_) => return Err(ReadError::UnsupportedLayout),
            }
        }
        Ok(())
    }
}

impl Pinned {
    fn open(path: &Path, directory: bool, maximum: Option<u64>) -> Result<Self, ReadError> {
        if path.as_os_str().encode_wide().count() >= MAX_PATH_UNITS { return Err(ReadError::UnsupportedLayout); }
        // 目录也需数据读取位才能使共享限制真正参与检查；仅 FILE_READ_ATTRIBUTES 不够。
        // FILE_LIST_DIRECTORY 只用于固定祖先的句柄锁定，绝不枚举或向调用者交出句柄。
        let file = OpenOptions::new()
            .access_mode(if directory { (FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY).0 } else { FILE_GENERIC_READ.0 })
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags((FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_NO_RECALL).0)
            .open(path).map_err(io_error)?;
        let stamp = inspect(&file, path, directory)?;
        if maximum.is_some_and(|maximum| stamp.size > maximum) { return Err(ReadError::TooLarge); }
        if !directory && stamp.size == 0 { return Err(ReadError::Malformed); }
        Ok(Self { file, path: path.to_path_buf(), stamp })
    }

    fn check(&self) -> Result<(), ReadError> {
        let directory = self.stamp.attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
        if inspect(&self.file, &self.path, directory)? != self.stamp {
            return Err(ReadError::Busy);
        }
        Ok(())
    }

    fn read_bounded(&mut self, maximum: u64) -> Result<Zeroizing<Vec<u8>>, ReadError> {
        self.check()?;
        if self.stamp.size > maximum { return Err(ReadError::TooLarge); }
        let length = usize::try_from(self.stamp.size).map_err(|_| ReadError::TooLarge)?;
        let mut bytes = Zeroizing::new(Vec::new());
        bytes.try_reserve_exact(length + 1).map_err(|_| ReadError::TooLarge)?;
        Read::by_ref(&mut self.file).take(self.stamp.size + 1).read_to_end(&mut bytes).map_err(io_error)?;
        if bytes.len() != length { return Err(ReadError::Busy); }
        self.check()?;
        Ok(bytes)
    }
}

fn inspect(file: &File, expected: &Path, directory: bool) -> Result<Stamp, ReadError> {
    let handle = HANDLE(file.as_raw_handle());
    // SAFETY: 句柄借自仍存活的 File；两个查询不取得所有权也不写文件。
    if unsafe { GetFileType(handle) } != FILE_TYPE_DISK { return Err(ReadError::UnsupportedLayout); }
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(handle, &mut info) }.map_err(|_| ReadError::ReadFailed)?;
    let forbidden = FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_DEVICE.0 | FILE_ATTRIBUTE_OFFLINE.0
        | FILE_ATTRIBUTE_RECALL_ON_OPEN.0 | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0;
    if info.dwFileAttributes & forbidden != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0) != directory
        || !directory && info.nNumberOfLinks != 1
    { return Err(ReadError::UnsupportedLayout); }
    check_final_path(handle, expected)?;
    Ok(Stamp {
        attributes: info.dwFileAttributes,
        volume: info.dwVolumeSerialNumber,
        index: pair(info.nFileIndexHigh, info.nFileIndexLow),
        size: pair(info.nFileSizeHigh, info.nFileSizeLow),
        links: info.nNumberOfLinks,
        created: pair(info.ftCreationTime.dwHighDateTime, info.ftCreationTime.dwLowDateTime),
        written: pair(info.ftLastWriteTime.dwHighDateTime, info.ftLastWriteTime.dwLowDateTime),
    })
}

fn check_final_path(handle: HANDLE, expected: &Path) -> Result<(), ReadError> {
    let mut actual = vec![0u16; MAX_PATH_UNITS];
    // SAFETY: 输出切片存活，句柄由调用者 File 保持，路径仅作固定位置核对。
    let flags = GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0);
    let count = unsafe { GetFinalPathNameByHandleW(handle, &mut actual, flags) } as usize;
    if count == 0 || count >= actual.len() { return Err(ReadError::UnsupportedLayout); }
    actual.truncate(count);
    let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
    let actual = actual.strip_prefix(prefix.as_slice()).ok_or(ReadError::UnsupportedLayout)?;
    let expected: Vec<u16> = expected.as_os_str().encode_wide().collect();
    if actual.len() != expected.len() || !actual.iter().zip(expected).all(|(a, b)| fold(*a) == fold(b)) {
        return Err(ReadError::UnsupportedLayout);
    }
    Ok(())
}

fn fold(value: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&value) { value + 32 } else { value }
}
fn pair(high: u32, low: u32) -> u64 { (u64::from(high) << 32) | u64::from(low) }

fn safe_name(name: &OsStr) -> Result<(), ReadError> {
    let units: Vec<u16> = name.encode_wide().collect();
    if units.is_empty() || units.iter().any(|unit| *unit < 0x20 || *unit == u16::from(b':'))
        || units.last().is_some_and(|unit| matches!(*unit, 0x20 | 0x2e))
    { return Err(ReadError::UnsupportedLayout); }
    Ok(())
}
fn wide(path: &Path) -> Result<Vec<u16>, ReadError> {
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    if value.len() >= MAX_PATH_UNITS || value.contains(&0) { return Err(ReadError::UnsupportedLayout); }
    value.push(0);
    Ok(value)
}
fn io_error(error: std::io::Error) -> ReadError {
    match error.raw_os_error() {
        Some(2 | 3) => ReadError::NotFound,
        Some(32 | 33) => ReadError::Busy,
        _ => ReadError::ReadFailed,
    }
}
