use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
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

static DESKTOP_CACHE: OnceLock<Mutex<Option<(Instant, Installation)>>> = OnceLock::new();
static CMD_VERSION_CACHE: OnceLock<Mutex<HashMap<PathBuf, (Option<SystemTime>, Instant, Option<String>)>>> = OnceLock::new();
static ASAR_CACHE: OnceLock<Mutex<HashMap<PathBuf, (Option<SystemTime>, u64, Arc<AsarIndex>)>>> = OnceLock::new();
static BUNDLED_CACHE: OnceLock<Mutex<HashMap<(PathBuf, Option<SystemTime>, u64, String), Option<Vec<(PathBuf, String)>>>>> = OnceLock::new();

#[derive(Clone)]
enum AsarLocation {
    Packed { offset: u64, size: u64 },
    Unpacked,
}

struct AsarIndex {
    data_offset: u64,
    file_len: u64,
    entries: HashMap<String, AsarLocation>,
}

pub fn invalidate_cache() {
    if let Some(cache) = DESKTOP_CACHE.get() {
        if let Ok(mut guard) = cache.lock() {
            *guard = None;
        }
    }
    if let Some(cache) = CMD_VERSION_CACHE.get() {
        if let Ok(mut guard) = cache.lock() {
            guard.clear();
        }
    }
    if let Some(cache) = ASAR_CACHE.get() {
        if let Ok(mut guard) = cache.lock() {
            guard.clear();
        }
    }
    if let Some(cache) = BUNDLED_CACHE.get() {
        if let Ok(mut guard) = cache.lock() {
            guard.clear();
        }
    }
}

pub fn cli(binary: Option<&Path>) -> Installation {
    cli_with_options(binary, true)
}

pub fn cli_with_options(binary: Option<&Path>, allow_spawn: bool) -> Installation {
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
        version: command_version(binary, allow_spawn).unwrap_or_default(),
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
    let cache = DESKTOP_CACHE.get_or_init(Default::default);
    if let Ok(cache) = cache.lock() {
        if let Some((time, value)) = cache.as_ref() {
            if time.elapsed() < Duration::from_secs(180) {
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

fn collect_asar_entries(
    node: &Value,
    prefix: &str,
    entries: &mut HashMap<String, AsarLocation>,
    filter_top: bool,
) {
    let Some(children) = node.get("files").and_then(Value::as_object) else {
        return;
    };
    for (name, child) in children {
        if filter_top && name != "dsh" && name != "package.json" {
            continue;
        }
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if child.get("link").is_some() {
            continue;
        }
        if child.get("files").is_some() {
            collect_asar_entries(child, &path, entries, false);
            continue;
        }
        if child.get("unpacked").and_then(Value::as_bool) == Some(true) {
            entries.insert(path, AsarLocation::Unpacked);
            continue;
        }
        let Some(size) = child.get("size").and_then(Value::as_u64) else {
            continue;
        };
        if size > 1024 * 1024 {
            continue;
        }
        let Some(offset) = child
            .get("offset")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        entries.insert(path, AsarLocation::Packed { offset, size });
    }
}

fn load_asar_index(path: &Path) -> Option<(Option<SystemTime>, u64, Arc<AsarIndex>)> {
    let meta = path.metadata().ok()?;
    let modified = meta.modified().ok();
    let file_len = meta.len();
    let cache = ASAR_CACHE.get_or_init(Default::default);
    if let Ok(guard) = cache.lock() {
        if let Some((cached_mod, cached_len, index)) = guard.get(path) {
            if *cached_mod == modified && *cached_len == file_len {
                return Some((modified, file_len, Arc::clone(index)));
            }
        }
    }
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
    drop(bytes);
    let mut entries = HashMap::new();
    collect_asar_entries(&header, "", &mut entries, json_size > 256 * 1024);
    drop(header);
    let index = Arc::new(AsarIndex {
        data_offset: 8u64.checked_add(header_size)?,
        file_len,
        entries,
    });
    if let Ok(mut guard) = cache.lock() {
        guard.insert(path.to_path_buf(), (modified, file_len, Arc::clone(&index)));
    }
    Some((modified, file_len, index))
}

fn asar_bytes(path: &Path, entry: &str) -> Option<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    let (_, _, index) = load_asar_index(path)?;
    let location = index.entries.get(entry)?;
    match *location {
        AsarLocation::Unpacked => files::read_limited(
            &path.with_file_name("app.asar.unpacked").join(entry),
            1024 * 1024,
        )
        .ok()?,
        AsarLocation::Packed { offset, size } => {
            let start = index.data_offset.checked_add(offset)?;
            if start.checked_add(size)? > index.file_len {
                return None;
            }
            let mut file = std::fs::File::open(path).ok()?;
            file.seek(SeekFrom::Start(start)).ok()?;
            let mut bytes = vec![0; size as usize];
            file.read_exact(&mut bytes).ok()?;
            Some(bytes)
        }
    }
}

fn asar_json(path: &Path, entry: &str) -> Option<Value> {
    serde_json::from_slice(&asar_bytes(path, entry)?).ok()
}

pub fn bundled_files(installation: &Installation, name: &str) -> Option<Vec<(PathBuf, String)>> {
    if installation.source.file_name()?.to_str()? != "app.asar" {
        return None;
    }
    let meta = installation.source.metadata().ok()?;
    let modified = meta.modified().ok();
    let file_len = meta.len();
    let cache_key = (
        installation.source.clone(),
        modified,
        file_len,
        name.to_string(),
    );
    let cache = BUNDLED_CACHE.get_or_init(Default::default);
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.get(&cache_key) {
            return cached.clone();
        }
    }
    let result = read_bundled_files(&installation.source, name);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(cache_key, result.clone());
    }
    result
}

fn read_bundled_files(source: &Path, name: &str) -> Option<Vec<(PathBuf, String)>> {
    let root = format!("dsh/node_modules/{name}");
    let package = asar_json(source, &format!("{root}/package.json"))?;
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
        let text = String::from_utf8(asar_bytes(source, &entry)?).ok()?;
        files.push((source.join(entry), text));
    }
    Some(files)
}

fn command_version(binary: &Path, allow_spawn: bool) -> Option<String> {
    let modified = binary.metadata().ok().and_then(|meta| meta.modified().ok());
    let cache = CMD_VERSION_CACHE.get_or_init(Default::default);
    if let Ok(cache) = cache.lock() {
        if let Some((previous, checked, value)) = cache.get(binary) {
            if *previous == modified && checked.elapsed() < Duration::from_secs(300) {
                return value.clone();
            }
        }
    }
    if !allow_spawn {
        let binary_buf = binary.to_path_buf();
        std::thread::spawn(move || {
            let _ = command_version(&binary_buf, true);
        });
        return None;
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
