use std::path::{Path, PathBuf};

pub(super) fn roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for name in ["Qoder", "Qoder CN"] {
        #[cfg(windows)] {
            roots.extend(registry(name));
            for variable in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(base) = std::env::var_os(variable).map(PathBuf::from).filter(|path| path.is_absolute()) {
                    roots.push(if variable == "LOCALAPPDATA" { base.join("Programs").join(name) } else { base.join(name) });
                }
            }
        }
        #[cfg(target_os = "macos")] {
            roots.push(PathBuf::from("/Applications").join(format!("{name}.app/Contents")));
            if let Ok(home) = super::super::desktop_credentials::home() {
                roots.push(home.join("Applications").join(format!("{name}.app/Contents")));
            }
        }
        #[cfg(target_os = "linux")] {
            let slug = name.to_ascii_lowercase().replace(' ', "-");
            roots.push(PathBuf::from("/opt").join(name));
            roots.push(PathBuf::from("/opt").join(&slug));
            roots.push(PathBuf::from("/usr/lib").join(&slug));
            roots.push(PathBuf::from("/snap").join(slug).join("current"));
        }
    }
    let mut resources = Vec::new();
    for root in roots {
        #[cfg(windows)] {
            if let Ok(entries) = std::fs::read_dir(root.join(".qoder-versions")) {
                let mut versions: Vec<_> = entries.take(64).flatten().filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .filter(|entry| entry.file_name().to_str().is_some_and(|name| name.len() <= 64 && name.split('.').all(|part|
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))).map(|entry| entry.path()).collect();
                versions.sort();
                resources.extend(versions.into_iter().rev().map(|version| version.join("resources")));
            }
        }
        resources.push(root.join("resources"));
        #[cfg(target_os = "macos")]
        resources.push(root.join("Resources"));
    }
    resources
}

#[cfg(windows)]
fn registry(name: &str) -> Vec<PathBuf> {
    use windows::{core::PCWSTR, Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ}};
    let path = format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{name}");
    let key: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
    let mut out = Vec::new();
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for field in ["InstallLocation", "UninstallString"] {
            let field_name: Vec<_> = field.encode_utf16().chain(Some(0)).collect();
            let mut data = [0u16; 2048];
            let mut size = (data.len() * 2) as u32;
            if unsafe { RegGetValueW(hive, PCWSTR(key.as_ptr()), PCWSTR(field_name.as_ptr()), RRF_RT_REG_SZ,
                None, Some(data.as_mut_ptr().cast()), Some(&mut size)) }.0 != 0 || size < 2 || size as usize > data.len() * 2 {
                continue;
            }
            let end = data.iter().position(|unit| *unit == 0).unwrap_or(data.len());
            let Ok(raw) = String::from_utf16(&data[..end]) else { continue; };
            let raw = raw.trim();
            let path = if field == "UninstallString" {
                raw.strip_prefix('"').and_then(|value| value.split_once('"').map(|(path, _)| path))
                    .or_else(|| raw.strip_suffix("uninstall.exe").map(|_| raw))
            } else { Some(raw.trim_matches('"')) };
            let Some(path) = path.map(PathBuf::from).filter(|path| path.is_absolute()) else { continue; };
            if field == "UninstallString" {
                if let Some(parent) = path.parent() { out.push(parent.to_path_buf()); }
            } else { out.push(path); }
        }
    }
    out
}

pub(super) fn official_files(resources: &Path) -> Vec<PathBuf> {
    vec![resources.join("umid").join(if cfg!(windows) { "runtime-info.exe" } else { "runtime-info" })]
}
