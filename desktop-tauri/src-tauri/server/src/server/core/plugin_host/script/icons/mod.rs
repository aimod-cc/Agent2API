mod cache;
mod download;
mod formats;
mod sources;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Notify;

use super::storage::Package;

struct Worker { wake: Notify, force: AtomicBool }

fn worker() -> &'static Worker {
    static WORKER: OnceLock<Worker> = OnceLock::new();
    WORKER.get_or_init(|| Worker { wake: Notify::new(), force: AtomicBool::new(false) })
}

pub(super) fn installed(package: &Package) -> Option<String> { cache::card(&sources::installed_key(package)) }
pub(super) fn provider(package: &Package, id: &str) -> Option<String> {
    package.provider_icons.get(id).and_then(|value| formats::own(value))
        .and_then(|value| cache::source(&value)).or_else(|| installed(package))
}
pub(super) fn recommended(item: &Value) -> Option<String> { cache::card(&sources::recommendation_key(item)) }
pub(super) fn names() -> std::collections::BTreeMap<String, String> { sources::names() }
pub(super) fn manifest_name(directory: &std::path::Path) -> Option<String> { sources::manifest_name(directory) }
pub(super) fn declaration(value: &str) -> Option<String> { formats::own(value) }
pub(super) fn market_sources(value: &str) -> Vec<String> { formats::market(value) }
pub(super) fn manifest(directory: &std::path::Path) -> Option<String> { sources::manifest_icon(directory) }
pub(crate) fn read(file: &str) -> Option<(Vec<u8>, &'static str)> { cache::read(file) }

pub fn wake(force: bool) {
    if force { worker().force.store(true, Ordering::Release); }
    worker().wake.notify_one();
}

async fn first(values: Vec<String>, force: bool, resolved: &mut std::collections::HashMap<String, Option<String>>) -> Option<String> {
    for value in values.into_iter().take(16) {
        let file = if let Some(file) = resolved.get(&value) { file.clone() } else {
            let file = cache::resolve(&value, force).await;
            resolved.insert(value, file.clone());
            file
        };
        if file.is_some() { return file; }
    }
    None
}

async fn refresh(force: bool) {
    let Ok(cards) = tokio::task::spawn_blocking(sources::cards).await else { return };
    let keys = cards.iter().map(|card| card.key.clone()).collect();
    cache::retain_cards(&keys);
    let mut sources = sources::Sources::default();
    sources.refresh_market(force).await;
    let mut resolved = std::collections::HashMap::new();
    for card in cards {
        let mut file = first(card.own.clone(), force, &mut resolved).await;
        if file.is_none() && !card.installed { file = first(sources.package(&card, force).await, force, &mut resolved).await; }
        if file.is_none() { file = first(sources.market(&card, force).await, force, &mut resolved).await; }
        if file.is_some() { cache::bind(&card.key, file.as_deref()); }
        if card.installed {
            for value in card.own.iter().take(33) {
                let _ = first(vec![value.clone()], force, &mut resolved).await;
            }
        }
    }
}

pub(super) fn spawn() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) { return; }
    crate::spawn_task(async {
        loop {
            let force = worker().force.swap(false, Ordering::AcqRel);
            refresh(force).await;
            tokio::select! {
                _ = worker().wake.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(3600)) => {},
            }
        }
    });
}
