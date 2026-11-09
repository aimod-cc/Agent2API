use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{cache, download, formats};
use super::super::storage::Package;

const MARKET: &str = "https://raw.githubusercontent.com/magpie-community/plugins/main/registry.json";
const DISTRIBUTION: &str = "https://raw.githubusercontent.com/magpie-community/plugins/dist/index.json";
const REFRESH_MS: i64 = 86_400_000;
const MAX_DOCUMENT: usize = 16 * 1024 * 1024;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct Document {
    checked_at: i64,
    retry_at: i64,
    entries: BTreeMap<String, Vec<String>>,
    listed: Vec<String>,
    names: BTreeMap<String, String>,
}

#[derive(Default)]
pub(super) struct Sources { loaded: HashMap<String, Arc<Document>> }

#[derive(Clone)]
pub(super) struct Card {
    pub key: String,
    pub name: String,
    pub version: String,
    pub providers: Vec<String>,
    pub source: String,
    pub selector: String,
    pub own: Vec<String>,
    pub installed: bool,
}

pub(super) fn installed_key(package: &Package) -> String {
    format!("installed:{}:{}", package.id, package.directory)
}

pub(super) fn recommendation_key(item: &Value) -> String {
    format!("recommended:{}:{}:{}:{}", item["name"].as_str().unwrap_or(""), item["version"].as_str().unwrap_or(""),
        item["source"].as_str().unwrap_or(""), item["selector"].as_str().unwrap_or(""))
}

pub(super) fn manifest_icon(directory: &std::path::Path) -> Option<String> {
    let bytes = cache::read_limited(&directory.join("package.json"), 1024 * 1024)?;
    let manifest: Value = serde_json::from_slice(&bytes).ok()?;
    formats::own(manifest["magpie"]["icon"].as_str()?)
}

pub(super) fn cards() -> Vec<Card> {
    let mut cards = Vec::new();
    if let Ok(registry) = super::super::storage::read() {
        for package in registry.packages {
            let mut own = Vec::new();
            if let Some(icon) = package.icon.as_deref().and_then(formats::own).or_else(||
                super::super::storage::directory(&package).ok().and_then(|directory| manifest_icon(&directory))) {
                own.push(icon);
            }
            for id in &package.providers {
                if let Some(icon) = package.provider_icons.get(id).and_then(|value| formats::own(value)) {
                    if !own.contains(&icon) { own.push(icon); }
                }
            }
            cards.push(Card { key: installed_key(&package), name: package.name, version: package.version,
                providers: package.providers, source: String::new(), selector: String::new(), own, installed: true });
        }
    }
    for item in super::super::catalog::recommendations().as_array().into_iter().flatten() {
        cards.push(Card { key: recommendation_key(item), name: item["name"].as_str().unwrap_or("").into(),
            version: item["version"].as_str().unwrap_or("").into(), providers: item["providers"].as_array().into_iter()
                .flatten().filter_map(Value::as_str).map(str::to_string).collect(),
            source: item["source"].as_str().unwrap_or("").into(), selector: item["selector"].as_str().unwrap_or("").into(),
            own: super::super::catalog::icon_sources(item), installed: false });
    }
    cards
}

fn insert(entries: &mut BTreeMap<String, Vec<String>>, key: String, values: Vec<String>) {
    if !values.is_empty() && entries.len() < 4096 { entries.entry(key).or_insert(values); }
}

impl Sources {
    async fn document(&mut self, url: &str, kind: &str, force: bool) -> Arc<Document> {
        let key = format!("{kind}:{url}");
        if let Some(value) = self.loaded.get(&key) { return value.clone(); }
        let path = cache::root().join("metadata").join(format!("{}.json", cache::hash(key.as_bytes())));
        let mut document = cache::read_limited(&path, MAX_DOCUMENT)
            .and_then(|bytes| serde_json::from_slice::<Document>(&bytes).ok()).unwrap_or_default();
        let now = crate::server::logging::now_ms();
        if force || document.retry_at <= now && (now.saturating_sub(document.checked_at) >= REFRESH_MS
            || kind == "market" && document.names.is_empty()) {
            let fetched = download::bytes(url, MAX_DOCUMENT).await.ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|value| project(&value, kind).map(|(entries, listed)| (entries, listed, project_names(&value, kind))));
            if let Some((entries, listed, names)) = fetched {
                document = Document { checked_at: now, retry_at: 0, entries, listed, names };
            } else { document.retry_at = now + 3_600_000; }
            if let Ok(bytes) = serde_json::to_vec(&document) {
                if bytes.len() <= MAX_DOCUMENT { let _ = super::super::storage::atomic_write(&path, &bytes); }
            }
        }
        let document = Arc::new(document);
        self.loaded.insert(key, document.clone());
        document
    }

    pub async fn refresh_market(&mut self, force: bool) {
        let _ = self.document(MARKET, "market", force).await;
    }

    pub async fn market(&mut self, card: &Card, force: bool) -> Vec<String> {
        let document = self.document(MARKET, "market", force).await;
        let mut icons = document.entries.get(&format!("package:{}", card.name)).cloned().unwrap_or_default();
        for id in &card.providers {
            if let Some(values) = document.entries.get(&format!("provider:{id}")) {
                for value in values { if !icons.contains(value) { icons.push(value.clone()); } }
            }
        }
        icons
    }

    pub async fn package(&mut self, card: &Card, force: bool) -> Vec<String> {
        if card.installed { return Vec::new(); }
        if let Some(repository) = card.source.strip_prefix("github:") {
            if let Ok(super::super::source::InstallSource::Github { repository, reference }) =
                super::super::source::InstallSource::parse(&format!("github:{repository}"), &card.selector) {
                if let Ok(mut url) = url::Url::parse("https://raw.githubusercontent.com/") {
                    if let Ok(mut path) = url.path_segments_mut() {
                        path.extend(repository.split('/')).push(if reference.is_empty() { "HEAD" } else { &reference }).push("package.json");
                    }
                    return self.document(url.as_str(), "package", force).await.entries.get(&format!("package:{}", card.name)).cloned().unwrap_or_default();
                }
            }
            return Vec::new();
        }
        let distributed = self.document(DISTRIBUTION, "distribution", force).await;
        if let Some(values) = distributed.entries.get(&format!("{}@{}", card.name, card.version)) { return values.clone(); }
        if card.source.starts_with("magpie:") { return Vec::new(); }
        if !super::super::packages::package_name(&card.name) || !super::super::source::valid_version(&card.version) { return Vec::new(); }
        let url = format!("https://registry.npmjs.org/{}/{}", card.name.replace('/', "%2f"), card.version);
        let document = self.document(&url, "package", force).await;
        document.entries.get(&format!("{}@{}", card.name, card.version)).cloned().unwrap_or_default()
    }
}

pub(super) fn display_name(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 160 && !value.chars().any(char::is_control)).then(|| value.to_string())
}

pub(super) fn names() -> BTreeMap<String, String> {
    let key = format!("market:{MARKET}");
    let path = cache::root().join("metadata").join(format!("{}.json", cache::hash(key.as_bytes())));
    cache::read_limited(&path, MAX_DOCUMENT)
        .and_then(|bytes| serde_json::from_slice::<Document>(&bytes).ok())
        .map(|document| document.names.into_iter().filter_map(|(package, name)| display_name(&name).map(|name| (package, name))).collect())
        .unwrap_or_default()
}

pub(super) fn manifest_name(directory: &std::path::Path) -> Option<String> {
    let bytes = cache::read_limited(&directory.join("package.json"), 1024 * 1024)?;
    let manifest: Value = serde_json::from_slice(&bytes).ok()?;
    display_name(manifest["magpie"]["name"].as_str()?)
}

fn project_names(value: &Value, kind: &str) -> BTreeMap<String, String> {
    if kind != "market" || value["version"] != 1 { return BTreeMap::new(); }
    value["plugins"].as_array().into_iter().flatten().filter_map(|item| {
        let package = item["package"].as_str().filter(|name| super::super::packages::package_name(name))?;
        Some((package.to_string(), display_name(item["name"].as_str()?)?))
    }).take(4096).collect()
}

fn project(value: &Value, kind: &str) -> Option<(BTreeMap<String, Vec<String>>, Vec<String>)> {
    let mut entries = BTreeMap::new();
    let mut listed = Vec::new();
    match kind {
        "market" => {
            if value["version"] != 1 { return None; }
            for item in value["plugins"].as_array()? {
                let Some(name) = item["package"].as_str() else { continue };
                // 名录以 package 字段为准：有图标没图标都算被收录（图标是另一件事）。
                if !listed.iter().any(|seen| seen == name) { listed.push(name.to_string()); }
                let Some(icon) = item["icon"].as_str() else { continue };
                let icons = formats::market(icon);
                insert(&mut entries, format!("package:{name}"), icons.clone());
                if item["community"] == true {
                    for provider in item["providers"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                        insert(&mut entries, format!("provider:{provider}"), icons.clone());
                    }
                }
            }
        }
        "distribution" => {
            if value["version"] != 1 { return None; }
            for (name, package) in value["packages"].as_object()? {
                for (version, release) in package["versions"].as_object().into_iter().flatten() {
                    let manifest = &release["manifest"];
                    if manifest["name"].as_str() != Some(name) || manifest["version"].as_str() != Some(version) { continue; }
                    if let Some(icon) = manifest["magpie"]["icon"].as_str().and_then(formats::own) {
                        insert(&mut entries, format!("{name}@{version}"), vec![icon]);
                    }
                }
            }
        }
        "package" => {
            let name = value["name"].as_str()?;
            if !super::super::packages::package_name(name) { return None; }
            if let Some(icon) = value["magpie"]["icon"].as_str().and_then(formats::own) {
                insert(&mut entries, format!("package:{name}"), vec![icon.clone()]);
                if let Some(version) = value["version"].as_str() {
                    insert(&mut entries, format!("{name}@{version}"), vec![icon]);
                }
            }
        }
        _ => return None,
    }
    Some((entries, listed))
}
