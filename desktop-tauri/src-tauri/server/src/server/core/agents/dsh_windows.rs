use std::path::PathBuf;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_EXPAND_SZ, REG_SZ,
    REG_VALUE_TYPE,
};

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn text(key: &Key, name: &str) -> Option<String> {
    let name = wide(name);
    let mut kind = REG_VALUE_TYPE::default();
    let mut bytes = 0u32;
    unsafe {
        if RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut bytes),
        )
        .0 != 0
            || !matches!(kind, REG_SZ | REG_EXPAND_SZ)
            || bytes > 65536
            || bytes % 2 != 0
        {
            return None;
        }
        let mut data = vec![0u16; bytes as usize / 2 + 1];
        if RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
        .0 != 0
        {
            return None;
        }
        let end = data
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(data.len());
        String::from_utf16(&data[..end]).ok()
    }
}

pub fn desktop_roots() -> Vec<PathBuf> {
    let uninstall = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
    let mut roots = Vec::new();
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let mut handle = HKEY::default();
            unsafe {
                if RegOpenKeyExW(
                    hive,
                    PCWSTR(uninstall.as_ptr()),
                    None,
                    KEY_READ | view,
                    &mut handle,
                )
                .0 != 0
                {
                    continue;
                }
            }
            let parent = Key(handle);
            for index in 0..4096 {
                let mut name = [0u16; 512];
                let mut length = name.len() as u32;
                let status = unsafe {
                    RegEnumKeyExW(
                        parent.0,
                        index,
                        Some(PWSTR(name.as_mut_ptr())),
                        &mut length,
                        None,
                        None,
                        None,
                        None,
                    )
                };
                if status.0 == 259 {
                    break;
                }
                if status.0 != 0 {
                    continue;
                }
                let mut handle = HKEY::default();
                unsafe {
                    if RegOpenKeyExW(
                        parent.0,
                        PCWSTR(name.as_ptr()),
                        None,
                        KEY_READ | view,
                        &mut handle,
                    )
                    .0 != 0
                    {
                        continue;
                    }
                }
                let key = Key(handle);
                if text(&key, "DisplayName").as_deref() != Some("DeepSeek Harness") {
                    continue;
                }
                if let Some(dir) = text(&key, "InstallLocation").filter(|dir| !dir.is_empty()) {
                    roots.push(PathBuf::from(dir));
                }
                if let Some(icon) = text(&key, "DisplayIcon") {
                    let icon = icon
                        .rsplit_once(',')
                        .filter(|(_, index)| index.trim().parse::<i32>().is_ok())
                        .map(|(path, _)| path)
                        .unwrap_or(&icon)
                        .trim()
                        .trim_matches('"');
                    let path = PathBuf::from(icon);
                    if path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
                    {
                        if let Some(dir) = path.parent() {
                            roots.push(dir.into());
                        }
                    }
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}
