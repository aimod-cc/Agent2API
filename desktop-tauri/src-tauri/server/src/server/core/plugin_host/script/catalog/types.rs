use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::source::InstallSource;
use super::super::storage::{Package, PackageSource};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Entry {
    pub name: String,
    pub label: String,
    pub version: String,
    pub providers: Vec<String>,
    pub source: String,
    pub selector: String,
    pub publisher: String,
    pub summary: BTreeMap<String, String>,
    pub repository: Option<String>,
    pub repository_url: Option<String>,
    pub stars: Option<u64>,
    pub stars_at: i64,
    pub icon_sources: Vec<String>,
    pub kind: String,
    pub installable: bool,
    pub unavailable_reason: Option<String>,
    pub pushed_at: String,
    pub manifest_at: i64,
    pub provider_verified: bool,
}

impl Entry {
    pub fn is_provider(&self) -> bool {
        self.kind == "provider" && (self.provider_verified
            || matches!(self.publisher.as_str(), "official" | "recommended") && !self.providers.is_empty())
    }

    pub fn public(&self) -> Value {
        json!({
            "name": self.name, "label": self.label, "version": self.version,
            "providers": self.providers, "source": self.source, "selector": self.selector,
            "publisher": self.publisher, "magpieOfficial": self.publisher == "official",
            "magpieRecommended": self.publisher == "recommended", "summary": self.summary,
            "repository": self.repository, "repositoryUrl": self.repository_url,
            "stars": self.stars, "starsAt": self.stars_at, "kind": self.kind,
            "installable": self.installable, "unavailableReason": self.unavailable_reason,
            "providerVerified": self.is_provider(),
        })
    }
}

pub(super) fn text(value: &Value, max: usize) -> String {
    value.as_str().unwrap_or("").trim().chars().filter(|ch| !ch.is_control()).take(max).collect()
}

pub(super) fn summary(value: &Value) -> BTreeMap<String, String> {
    value.as_object().into_iter().flatten().filter_map(|(locale, value)| {
        if locale.len() > 16 || !locale.bytes().all(|ch| ch.is_ascii_alphanumeric() || ch == b'-') { return None; }
        let value = text(value, 8000);
        (!value.is_empty()).then(|| (locale.clone(), value))
    }).take(12).collect()
}

pub(super) fn repository(value: &Value) -> Option<String> {
    let raw = value.as_str().or_else(|| value.get("url")?.as_str())?.trim();
    let raw = raw.strip_prefix("git+").unwrap_or(raw);
    let normalized = if let Some(path) = raw.strip_prefix("git@github.com:") {
        format!("https://github.com/{path}")
    } else if let Some(path) = raw.strip_prefix("ssh://git@github.com/") {
        format!("https://github.com/{path}")
    } else if let Some(path) = raw.strip_prefix("git://github.com/") {
        format!("https://github.com/{path}")
    } else if !raw.contains(':') && raw.split('/').count() == 2 {
        format!("github:{raw}")
    } else { raw.to_string() };
    match InstallSource::parse(&normalized, "") {
        Ok(InstallSource::Github { repository, .. }) => Some(repository),
        _ => None,
    }
}

pub(super) fn installed_entry<'a>(package: &Package, entries: &'a [Entry]) -> Option<&'a Entry> {
    let matching = |entry: &&Entry| match &package.source {
        Some(PackageSource::Local { .. }) => false,
        Some(PackageSource::Github { repository, .. }) => entry.repository.as_ref()
            .is_some_and(|known| known.eq_ignore_ascii_case(repository)),
        Some(PackageSource::Magpie { .. }) => entry.publisher != "third-party",
        None => !entry.source.starts_with("github:"),
    };
    entries.iter().filter(|entry| entry.name == package.name).find(matching)
}

pub(super) fn merge(curated: &[Entry], tagged: &[Entry]) -> Vec<Entry> {
    let mut entries: Vec<_> = curated.iter().filter(|entry| entry.is_provider()).cloned().collect();
    let names: BTreeSet<_> = entries.iter().map(|entry| entry.name.clone()).collect();
    for entry in tagged.iter().filter(|entry| entry.is_provider()) {
        if names.contains(entry.name.as_str()) && curated.iter().any(|known| known.name == entry.name
            && (known.repository.is_none() || known.repository.as_ref().zip(entry.repository.as_ref())
                .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right)))) {
            continue;
        }
        entries.push(entry.clone());
    }
    entries.sort_by(|left, right| {
        let rank = |entry: &Entry| match entry.publisher.as_str() { "official" => 0, "recommended" => 1, _ => 2 };
        rank(left).cmp(&rank(right)).then_with(|| right.stars.cmp(&left.stars)).then_with(|| left.label.cmp(&right.label))
    });
    entries
}
