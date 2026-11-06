use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use super::files;

#[derive(Clone, Default)]
pub struct Installation {
    pub version: String,
    pub source: PathBuf,
    pub package_root: Option<PathBuf>,
}

pub fn modern(version: &str) -> Option<bool> {
    let version = version.trim().trim_start_matches('v');
    let mut parts = version.split(['.', '-', '+']);
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next()?.parse::<u32>().ok()?;
    Some((major, minor, patch) >= (0, 1, 7))
}

fn version(value: &Value) -> Option<String> {
    let value = value.get("version")?.as_str()?;
    modern(value)?;
    Some(value.to_string())
}

fn manifest(path: &Path) -> Option<Value> {
    let bytes = files::read_limited(path, 1024 * 1024).ok()??;
    serde_json::from_slice(&bytes).ok()
}

fn package(path: PathBuf, expected: &str) -> Option<Installation> {
    let value = manifest(&path)?;
    if value.get("name").and_then(Value::as_str) != Some(expected) {
        return None;
    }
    Some(Installation {
        version: version(&value)?,
        package_root: path.parent().map(PathBuf::from),
        source: path,
    })
}

pub fn cli(binary: Option<&Path>) -> Installation {
    let Some(binary) = binary else {
        return Installation::default();
    };
    let mut candidates = Vec::new();
    for program in [
        Some(binary.to_path_buf()),
        std::fs::canonicalize(binary).ok(),
    ]
    .into_iter()
    .flatten()
    {
        for parent in program.ancestors().skip(1).take(5) {
            candidates.push(parent.join("package.json"));
            candidates.push(parent.join("node_modules/@deepseek-ai/dsh/package.json"));
            candidates.push(parent.join("lib/node_modules/@deepseek-ai/dsh/package.json"));
        }
    }
    for path in candidates {
        if let Some(found) = package(path, "@deepseek-ai/dsh") {
            return found;
        }
    }
    Installation {
        version: command_version(binary).unwrap_or_default(),
        source: binary.into(),
        package_root: None,
    }
}

pub fn profile(dir: &Path) -> Installation {
    for parent in dir.ancestors().take(3) {
        for name in ["dsh", "dsh-base", "dsh-settings"] {
            let path = parent
                .join("node_modules/@deepseek-ai")
                .join(name)
                .join("package.json");
            if let Some(found) = package(path, &format!("@deepseek-ai/{name}")) {
                return found;
            }
        }
    }
    Installation::default()
}

pub fn package_file(installation: &Installation, name: &str) -> Option<PathBuf> {
    let root = installation.package_root.as_ref()?;
    for parent in root.ancestors().take(6) {
        for path in [
            parent.join("node_modules").join(name).join("package.json"),
            parent.join(name).join("package.json"),
        ] {
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

pub fn desktop() -> Installation {
    static CACHE: OnceLock<Mutex<Option<(Instant, Installation)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(cache) = cache.lock() {
        if let Some((time, value)) = cache.as_ref() {
            if time.elapsed() < Duration::from_secs(10) {
                return value.clone();
            }
        }
    }
    let found = find_desktop();
    if let Ok(mut cache) = cache.lock() {
        *cache = Some((Instant::now(), found.clone()));
    }
    found
}

fn find_desktop() -> Installation {
    for root in desktop_roots() {
        let resources = if root.extension().is_some_and(|ext| ext == "app") {
            root.join("Contents/Resources")
        } else {
            root.join("resources")
        };
        for path in [
            resources.join("app/dsh/package.json"),
            resources.join("app.asar.unpacked/dsh/package.json"),
            resources.join("runtime/dsh/package.json"),
        ] {
            if let Some(found) = package(path, "@deepseek-ai/dsh") {
                return found;
            }
        }
        let archive = resources.join("app.asar");
        for entry in [
            "dsh/node_modules/@deepseek-ai/dsh/package.json",
            "dsh/package.json",
            "package.json",
        ] {
            if let Some(value) = asar_json(&archive, entry) {
                let name = value.get("name").and_then(Value::as_str).unwrap_or("");
                if !matches!(name, "@deepseek-ai/dsh" | "@deepseek-ai/dsh-desktop") {
                    continue;
                }
                if let Some(version) = version(&value) {
                    return Installation {
                        version,
                        source: archive,
                        package_root: None,
                    };
                }
            }
        }
    }
    Installation::default()
}

fn desktop_roots() -> Vec<PathBuf> {
    let home = super::paths::home();
    let mut roots = Vec::new();
    #[cfg(windows)]
    roots.extend(super::dsh_windows::desktop_roots());
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            for parent in [
                PathBuf::from(&local),
                PathBuf::from(&local).join("Programs"),
            ] {
                for name in ["DeepSeek Harness", "deepseek-harness", "dsh-desktop"] {
                    roots.push(parent.join(name));
                }
            }
        }
        for key in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(key) {
                roots.push(PathBuf::from(dir).join("DeepSeek Harness"));
            }
        }
    } else if cfg!(target_os = "macos") {
        roots.extend([
            PathBuf::from("/Applications/DeepSeek Harness.app"),
            home.join("Applications/DeepSeek Harness.app"),
        ]);
    } else {
        roots.extend([
            PathBuf::from("/opt/DeepSeek Harness"),
            PathBuf::from("/opt/deepseek-harness"),
            home.join(".local/share/deepseek-harness"),
        ]);
    }
    roots
}

fn asar_bytes(path: &Path, entry: &str) -> Option<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut prefix = [0u8; 16];
    file.read_exact(&mut prefix).ok()?;
    let size = |start| {
        u32::from_le_bytes(prefix[start..start + 4].try_into().ok().unwrap_or([0; 4])) as u64
    };
    let header_size = size(4);
    let json_size = size(12);
    if size(0) != 4 || !(8..=32 * 1024 * 1024).contains(&header_size) || json_size > header_size - 8
    {
        return None;
    }
    let mut bytes = vec![0; json_size as usize];
    file.read_exact(&mut bytes).ok()?;
    let header: Value = serde_json::from_slice(&bytes).ok()?;
    let mut node = &header;
    for part in entry.split('/') {
        node = node.get("files")?.get(part)?;
    }
    if node.get("link").is_some() {
        return None;
    }
    if node.get("unpacked").and_then(Value::as_bool) == Some(true) {
        return files::read_limited(
            &path.with_file_name("app.asar.unpacked").join(entry),
            1024 * 1024,
        )
        .ok()?;
    }
    let length = node.get("size")?.as_u64()?;
    if length > 1024 * 1024 {
        return None;
    }
    let offset = node.get("offset")?.as_str()?.parse::<u64>().ok()?;
    let start = 8u64.checked_add(header_size)?.checked_add(offset)?;
    if start.checked_add(length)? > file.metadata().ok()?.len() {
        return None;
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = vec![0; length as usize];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

fn asar_json(path: &Path, entry: &str) -> Option<Value> {
    serde_json::from_slice(&asar_bytes(path, entry)?).ok()
}

pub fn bundled_files(installation: &Installation, name: &str) -> Option<Vec<(PathBuf, String)>> {
    if installation.source.file_name()?.to_str()? != "app.asar" {
        return None;
    }
    let root = format!("dsh/node_modules/{name}");
    let package = asar_json(&installation.source, &format!("{root}/package.json"))?;
    let patches = package.pointer("/dsh/bundle/patch")?;
    let paths = if let Some(path) = patches.as_str() {
        vec![path]
    } else {
        patches
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()?
    };
    let mut files = Vec::new();
    for path in paths {
        if path.contains("..") || path.contains('\\') || path.starts_with('/') {
            return None;
        }
        let entry = format!("{root}/{}", path.trim_start_matches("./"));
        let text = String::from_utf8(asar_bytes(&installation.source, &entry)?).ok()?;
        files.push((installation.source.join(entry), text));
    }
    Some(files)
}

fn command_version(binary: &Path) -> Option<String> {
    type Cached = (Option<SystemTime>, Instant, Option<String>);
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
    let modified = binary.metadata().ok().and_then(|meta| meta.modified().ok());
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(cache) = cache.lock() {
        if let Some((previous, checked, value)) = cache.get(binary) {
            if *previous == modified && checked.elapsed() < Duration::from_secs(30) {
                return value.clone();
            }
        }
    }
    let value = run_version(binary);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(binary.into(), (modified, Instant::now(), value.clone()));
    }
    value
}

fn run_version(binary: &Path) -> Option<String> {
    let mut program = binary.to_path_buf();
    if cfg!(windows) && program.extension().is_none() && program.with_extension("cmd").is_file() {
        program.set_extension("cmd");
    }
    let mut command = Command::new(program);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(4097).read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !success {
        return None;
    }
    let bytes = receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()?;
    if bytes.len() > 4096 {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    text.split_whitespace()
        .find(|word| modern(word).is_some())
        .map(|word| word.trim_start_matches('v').to_string())
}
