use std::collections::BTreeMap;
use serde_json::Value;

use super::network::{self, Failure};
use super::types::{self, Entry};

pub(super) const MARKET_URL: &str = "https://raw.githubusercontent.com/magpie-community/plugins/main/registry.json";
const DISTRIBUTION_URL: &str = "https://raw.githubusercontent.com/magpie-community/plugins/dist/index.json";

pub(super) fn project(value: &Value) -> Result<Vec<Entry>, Failure> {
    if value["version"] != 1 { return Err("Magpie 官方名单版本暂不兼容".to_string().into()); }
    let plugins = value["plugins"].as_array().filter(|items| !items.is_empty() && items.len() <= 4096)
        .ok_or_else(|| Failure::from("Magpie 官方名单格式无效".to_string()))?;
    let mut result = Vec::new();
    for item in plugins {
        let name = item["package"].as_str().unwrap_or("");
        if !super::super::packages::package_name(name) { continue; }
        let providers: Vec<_> = item["providers"].as_array().into_iter().flatten().filter_map(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 160).take(128).map(str::to_string).collect();
        let kind = types::text(&item["kind"], 32);
        // 本宿主只承接提供商，不把原版的页面、工具或中间件列为可安装提供商。
        if !kind.is_empty() && kind != "provider" || providers.is_empty() { continue; }
        if result.iter().any(|entry: &Entry| entry.name == name) { continue; }
        let official = name.starts_with("@magpie-community/") && item["community"] == true;
        result.push(Entry {
            name: name.to_string(), label: types::text(&item["name"], 160), providers,
            publisher: if official { "official" } else { "recommended" }.to_string(),
            source: if official { format!("magpie:{name}") } else { name.to_string() },
            selector: "latest".to_string(), summary: types::summary(&item["summary"]),
            repository: official.then(|| "magpie-community/plugins".to_string()),
            repository_url: official.then(|| "https://github.com/magpie-community/plugins".to_string()),
            icon_sources: super::super::icons::market_sources(item["icon"].as_str().unwrap_or("")),
            kind: "provider".to_string(), installable: true, provider_verified: true, ..Entry::default()
        });
    }
    if result.is_empty() { return Err("Magpie 官方名单没有可识别的提供商条目".to_string().into()); }
    for entry in &mut result { if entry.label.is_empty() { entry.label = entry.name.clone(); } }
    Ok(result)
}

pub(super) async fn distribution() -> Result<Value, Failure> {
    let index = network::github(DISTRIBUTION_URL, 16 * 1024 * 1024).await?
        .ok_or_else(|| Failure::from("Magpie 分发索引不存在".to_string()))?;
    if index["version"] != 1 || !index["packages"].is_object() {
        return Err("Magpie 分发索引格式无效".to_string().into());
    }
    Ok(index)
}

pub(super) fn distributed(index: &Value, name: &str) -> Option<Value> {
    let package = index["packages"].get(name)?;
    let version = package["latest"].as_str().filter(|value| !value.is_empty() && super::super::source::valid_version(value))?;
    let manifest = package["versions"][version]["manifest"].clone();
    (manifest["name"].as_str() == Some(name) && manifest["version"].as_str() == Some(version)).then_some(manifest)
}

pub(super) fn apply_manifest(entry: &mut Entry, manifest: &Value, own_text: bool) {
    if manifest["name"].as_str() != Some(entry.name.as_str()) { return; }
    if let Some(version) = manifest["version"].as_str().filter(|value| !value.is_empty() && super::super::source::valid_version(value)) {
        entry.version = version.to_string();
    }
    if entry.publisher != "official" {
        if let Some(repo) = types::repository(&manifest["repository"]) {
            entry.repository_url = Some(format!("https://github.com/{repo}"));
            entry.repository = Some(repo);
        }
    }
    if let Some(icon) = manifest["magpie"]["icon"].as_str().and_then(super::super::icons::declaration) {
        entry.icon_sources.retain(|value| value != &icon);
        entry.icon_sources.insert(0, icon);
        entry.icon_sources.truncate(8);
    }
    if own_text {
        let label = types::text(&manifest["magpie"]["name"], 160);
        if !label.is_empty() { entry.label = label; }
        let summary = types::summary(&manifest["magpie"]["summary"]);
        if !summary.is_empty() { entry.summary = summary; }
        else if entry.summary.is_empty() {
            let description = types::text(&manifest["description"], 8000);
            if !description.is_empty() { entry.summary = BTreeMap::from([("en".to_string(), description)]); }
        }
    }
    entry.manifest_at = crate::server::logging::now_ms();
}

pub(super) fn copy_metadata(entry: &mut Entry, previous: &Entry) {
    entry.version = previous.version.clone();
    entry.source = previous.source.clone();
    entry.selector = previous.selector.clone();
    entry.installable = previous.installable;
    entry.unavailable_reason = previous.unavailable_reason.clone();
    if previous.repository.is_some() {
        entry.repository = previous.repository.clone();
        entry.repository_url = previous.repository_url.clone();
    }
    entry.stars = previous.stars;
    entry.stars_at = previous.stars_at;
    entry.manifest_at = previous.manifest_at;
    for icon in &previous.icon_sources {
        if !entry.icon_sources.contains(icon) && entry.icon_sources.len() < 8 { entry.icon_sources.push(icon.clone()); }
    }
}
