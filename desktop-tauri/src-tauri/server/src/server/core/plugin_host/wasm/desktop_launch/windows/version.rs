use std::ffi::c_void;
use std::path::Path;

#[link(name = "version")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(filename: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(filename: *const u16, handle: u32, length: u32, data: *mut c_void) -> i32;
    fn VerQueryValueW(data: *const c_void, key: *const u16, output: *mut *mut c_void, length: *mut u32) -> i32;
}

#[repr(C)]
struct Fixed {
    signature: u32, structure: u32, file_ms: u32, file_ls: u32, product_ms: u32, product_ls: u32,
    flags_mask: u32, flags: u32, os: u32, kind: u32, subtype: u32, date_ms: u32, date_ls: u32,
}

fn query(data: &[u32], key: &str) -> Option<(*const u8, usize)> {
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let mut value = std::ptr::null_mut();
    let mut length = 0;
    if unsafe { VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut value, &mut length) } == 0 || value.is_null() { return None; }
    Some((value.cast(), length as usize))
}

pub fn product(path: &Path) -> Result<(String, [u16; 3]), String> {
    use std::os::windows::ffi::OsStrExt;
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut handle = 0;
    let length = unsafe { GetFileVersionInfoSizeW(name.as_ptr(), &mut handle) };
    if length == 0 || length > 1024 * 1024 { return Err("客户端程序身份信息不可读".into()); }
    let mut data = vec![0u32; length as usize / 4 + 1];
    if unsafe { GetFileVersionInfoW(name.as_ptr(), 0, length, data.as_mut_ptr().cast()) } == 0 {
        return Err("客户端程序身份信息不可读".into());
    }
    let (fixed, size) = query(&data, "\\").ok_or("客户端程序版本信息不可读")?;
    if size < std::mem::size_of::<Fixed>() { return Err("客户端程序版本格式无效".into()); }
    let fixed = unsafe { std::ptr::read_unaligned(fixed.cast::<Fixed>()) };
    if fixed.signature != 0xfeef04bd { return Err("客户端程序版本格式无效".into()); }
    let version = [(fixed.product_ms >> 16) as u16, fixed.product_ms as u16, (fixed.product_ls >> 16) as u16];
    let (translations, bytes) = query(&data, "\\VarFileInfo\\Translation").ok_or("客户端程序身份信息缺失")?;
    if bytes < 4 || bytes > 256 || bytes % 4 != 0 { return Err("客户端程序身份信息格式无效".into()); }
    for index in 0..bytes / 4 {
        let value = unsafe { std::ptr::read_unaligned(translations.add(index * 4).cast::<u32>()) };
        let key = format!("\\StringFileInfo\\{:04x}{:04x}\\ProductName", value & 0xffff, value >> 16);
        let Some((value, length)) = query(&data, &key) else { continue; };
        if length == 0 || length > 256 { continue; }
        let units = unsafe { std::slice::from_raw_parts(value.cast::<u16>(), length) };
        let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
        if let Ok(name) = String::from_utf16(&units[..end]) { return Ok((name, version)); }
    }
    Err("客户端程序身份信息缺失".into())
}
