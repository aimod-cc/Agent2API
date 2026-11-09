use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::formats;

const RETRY_MS: i64 = 3_600_000;
const REFRESH_MS: i64 = 7 * 86_400_000;
const MAX_ENTRIES: usize = 512;
const MAX_DISK_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct Image {
    file: Option<String>,
    bytes: usize,
    checked_at: i64,
    retry_at: i64,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct Index {
    images: BTreeMap<String, Image>,
    cards: BTreeMap<String, String>,
}

#[derive(Default)]
struct Cache { root: PathBuf, index: Index }

pub(super) fn root() -> PathBuf { super::super::root().join("icons") }
pub(super) fn hash(value: &[u8]) -> String { format!("{:x}", Sha256::digest(value)) }

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

fn load(cache: &mut Cache) {
    let root = root();
    if cache.root == root { return; }
    let index = read_limited(&root.join("index.json"), 1024 * 1024)
        .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok()).unwrap_or_default();
    *cache = Cache { root, index };
    cache.index.images.retain(|key, image| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
        && image.file.as_deref().is_none_or(formats::filename));
    cache.index.cards.retain(|_, file| formats::filename(file));
}

pub(super) fn read_limited(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > limit as u64 { return None; }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= limit).then_some(bytes)
}

fn persist(cache: &mut Cache) {
    while cache.index.images.len() > MAX_ENTRIES {
        let Some(key) = cache.index.images.iter().min_by_key(|(_, image)| image.checked_at).map(|(key, _)| key.clone()) else { break };
        cache.index.images.remove(&key);
    }
    let mut files: HashMap<String, (usize, i64)> = HashMap::new();
    for image in cache.index.images.values() {
        if let Some(file) = &image.file {
            let entry = files.entry(file.clone()).or_default();
            entry.0 = image.bytes;
            entry.1 = entry.1.max(image.checked_at);
        }
    }
    let mut ordered: Vec<_> = files.into_iter().collect();
    ordered.sort_by_key(|(_, (_, at))| *at);
    let mut total: usize = ordered.iter().map(|(_, (bytes, _))| bytes).sum();
    for (file, (bytes, _)) in ordered {
        if total <= MAX_DISK_BYTES { break; }
        cache.index.images.retain(|_, image| image.file.as_deref() != Some(file.as_str()));
        cache.index.cards.retain(|_, value| value != &file);
        total = total.saturating_sub(bytes);
    }
    let referenced: std::collections::HashSet<_> = cache.index.images.values().filter_map(|image| image.file.as_ref()).collect();
    cache.index.cards.retain(|_, file| referenced.contains(file));
    if let Ok(entries) = std::fs::read_dir(&cache.root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if formats::filename(&name) && !referenced.contains(&name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    if let Ok(bytes) = serde_json::to_vec(&cache.index) {
        if let Err(error) = super::super::storage::atomic_write(&cache.root.join("index.json"), &bytes) {
            crate::server::logging::verbose("[PluginIcons]", &format!("保存图标缓存索引失败：{error}"));
        }
    }
}

pub(super) fn source(value: &str) -> Option<String> {
    let mut cache = cache().lock().ok()?;
    load(&mut cache);
    let file = cache.index.images.get(&hash(value.as_bytes()))?.file.as_deref()?;
    Some(format!("/plugins/script-icons/{file}"))
}

pub(super) fn card(key: &str) -> Option<String> {
    let mut cache = cache().lock().ok()?;
    load(&mut cache);
    let file = cache.index.cards.get(key)?;
    Some(format!("/plugins/script-icons/{file}"))
}

pub(super) fn bind(key: &str, file: Option<&str>) {
    let Ok(mut cache) = cache().lock() else { return };
    load(&mut cache);
    let cached = match file {
        Some(file) if formats::filename(file) => Some(file),
        _ => None,
    };
    if cache.index.cards.get(key).map(String::as_str) == cached { return; }
    match cached {
        Some(file) => { cache.index.cards.insert(key.into(), file.into()); }
        None => { cache.index.cards.remove(key); }
    }
    persist(&mut cache);
}

pub(super) fn retain_cards(keys: &std::collections::HashSet<String>) {
    let Ok(mut cache) = cache().lock() else { return };
    load(&mut cache);
    let count = cache.index.cards.len();
    cache.index.cards.retain(|key, _| keys.contains(key));
    if cache.index.cards.len() != count { persist(&mut cache); }
}

pub(super) async fn resolve(value: &str, force: bool) -> Option<String> {
    let key = hash(value.as_bytes());
    let inline = value.get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"));
    let now = crate::server::logging::now_ms();
    let previous = {
        let mut cache = cache().lock().ok()?;
        load(&mut cache);
        let mut previous = cache.index.images.get(&key).cloned().unwrap_or_default();
        if previous.file.as_ref().is_some_and(|file| !cache.root.join(file).is_file()) { previous.file = None; }
        if inline && previous.file.is_some() || !force && (previous.retry_at > now
            || previous.file.is_some() && now.saturating_sub(previous.checked_at) < REFRESH_MS) {
            return previous.file;
        }
        previous
    };
    let result = if inline {
        formats::inline(value).ok_or_else(|| "插件内嵌图标无效".to_string())
    } else { super::download::bytes(value, formats::MAX_BYTES).await };
    let image = result.and_then(|bytes| {
        let extension = formats::extension(&bytes).ok_or("插件图标不是支持的图片格式")?;
        let file = format!("{}.{extension}", hash(&bytes));
        let path = root().join(&file);
        if !path.is_file() { super::super::storage::atomic_write(&path, &bytes)?; }
        Ok(Image { file: Some(file), bytes: bytes.len(), checked_at: now, retry_at: 0 })
    });
    let image = match image {
        Ok(image) => image,
        Err(_) => Image { retry_at: now + RETRY_MS, ..previous },
    };
    let result = image.file.clone();
    let mut cache = cache().lock().ok()?;
    load(&mut cache);
    cache.index.images.insert(key, image);
    persist(&mut cache);
    result
}

pub(crate) fn read(file: &str) -> Option<(Vec<u8>, &'static str)> {
    if !formats::filename(file) { return None; }
    let root = root().canonicalize().ok()?;
    let path = root.join(file).canonicalize().ok()?;
    if !path.starts_with(&root) { return None; }
    let bytes = read_limited(&path, formats::MAX_BYTES)?;
    let (digest, extension) = file.split_once('.')?;
    if hash(&bytes) != digest || formats::extension(&bytes)? != extension { return None; }
    Some((bytes, formats::mime(extension)?))
}
