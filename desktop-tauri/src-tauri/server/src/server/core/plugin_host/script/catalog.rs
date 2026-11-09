use serde_json::{json, Value};

use super::storage::{Package, PackageSource};

mod cache;
mod github;
mod market;
mod network;
mod provider;
mod refresh;
mod types;

pub(super) fn spawn() { refresh::spawn(); }
pub(super) fn wake(force: bool) { refresh::wake(force); }
pub(super) fn status() -> Value { cache::status() }

pub(super) fn recommendations() -> Value {
    Value::Array(cache::entries().iter().map(types::Entry::public).collect())
}

pub(super) fn icon_sources(item: &Value) -> Vec<String> {
    cache::entries().iter().find(|entry| item["name"].as_str() == Some(entry.name.as_str())
        && item["source"].as_str() == Some(entry.source.as_str()))
        .map(|entry| entry.icon_sources.clone()).unwrap_or_default()
}

pub(super) fn package_metadata(package: &Package) -> Value {
    let entries = cache::entries();
    if let Some(entry) = types::installed_entry(package, &entries) { return entry.public(); }
    let repository = match &package.source {
        Some(PackageSource::Github { repository, .. }) => Some(repository.clone()),
        _ => None,
    };
    json!({ "publisher": "third-party", "magpieOfficial": false, "magpieRecommended": false,
        "repositoryUrl": repository.as_ref().map(|repo| format!("https://github.com/{repo}")),
        "repository": repository, "stars": Value::Null, "starsAt": 0 })
}

pub(super) fn package_name(package: &Package, _names: &std::collections::BTreeMap<String, String>) -> String {
    let entries = cache::entries();
    types::installed_entry(package, &entries).map(|entry| entry.label.clone())
        .or_else(|| super::storage::directory(package).ok().and_then(|directory| super::icons::manifest_name(&directory)))
        .unwrap_or_else(|| package.name.clone())
}

pub(super) fn label(id: &str) -> Option<&'static str> {
    Some(match id {
        "agy-cli" => "Antigravity (local agy CLI)",
        "minimax-code" => "MiniMax Code 国内版",
        "minimax-code-global" => "MiniMax Code 国际版",
        "deepaicode" => "DeepAI Code",
        "cloudflare-workers-ai" => "Cloudflare Workers AI",
        "codar" => "Codar (科搭)",
        "joycode" => "JoyCode",
        "agnes-code" => "Agnes Code",
        "river" => "River",
        "agnes" => "Agnes AI",
        "warp" => "Warp",
        "intern-inkstone" => "书生·端砚",
        "gcp-ai" => "GCP AI",
        "sap-aicore" => "SAP AI Core",
        "mclaw" => "MClaw",
        "wps-lingxi" => "WPS 灵犀",
        "github-copilot" => "GitHub Copilot",
        "kimi-work" => "Kimi Work",
        "cursor" => "Cursor",
        "devin" => "Devin",
        "factory" => "Factory",
        "zed" => "Zed",
        "grok" => "Grok",
        "opendesign" => "Open Design",
        "nous-portal" => "Nous Portal",
        "mimo-app" => "Xiaomi MiMo",
        "bedrock-sso" => "AWS Bedrock SSO",
        _ => return None,
    })
}
