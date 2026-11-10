use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use futures::{stream, StreamExt};
use tokio::sync::Notify;

use super::{cache, github, market, network, types};

const MARKET_TTL: i64 = 6 * 3_600_000;
const GITHUB_TTL: i64 = 10 * 60_000;

struct Worker { wake: Notify, force: AtomicBool, started: AtomicBool }
fn worker() -> &'static Worker {
    static WORKER: OnceLock<Worker> = OnceLock::new();
    WORKER.get_or_init(|| Worker { wake: Notify::new(), force: AtomicBool::new(false), started: AtomicBool::new(false) })
}

pub(super) fn wake(force: bool) {
    spawn();
    if force { worker().force.store(true, Ordering::Release); }
    worker().wake.notify_one();
}

pub(super) fn spawn() {
    if worker().started.swap(true, Ordering::AcqRel) { return; }
    crate::spawn_task(async {
        loop {
            let force = worker().force.swap(false, Ordering::AcqRel);
            refresh(force).await;
            cache::progress(false, 0, 0);
            tokio::select! {
                _ = worker().wake.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(60)) => {},
            }
        }
    });
}

async fn curated(force: bool) {
    let previous = cache::snapshot();
    let now = crate::server::logging::now_ms();
    if previous.market_retry > now || !force && previous.market_retry == 0
        && previous.market_at > 0 && now - previous.market_at < MARKET_TTL { return; }
    cache::progress(true, 0, 0);
    let result = network::github(market::MARKET_URL, 2 * 1024 * 1024).await
        .and_then(|value| value.ok_or_else(|| network::Failure::from("Magpie 官方名单不存在".to_string())))
        .and_then(|value| market::project(&value));
    let mut entries = match result {
        Ok(entries) => entries,
        Err(error) => {
            cache::update(|cache| {
                cache.market_error = Some(error.message);
                cache.market_retry = error.retry_at.max(now + 60_000);
            });
            cache::persist();
            return;
        }
    };
    for entry in &mut entries {
        if let Some(old) = previous.market.iter().find(|old| old.name == entry.name) { market::copy_metadata(entry, old); }
    }
    cache::update(|cache| { cache.market = entries.clone(); cache.market_at = now; cache.market_retry = 0; cache.market_error = None; });
    cache::persist();
    super::super::icons::wake(false);
    let distribution = market::distribution().await;
    let mut issue: Option<network::Failure> = None;
    let total = entries.len();
    let jobs: Vec<_> = entries.into_iter().enumerate().map(|(index, entry)| {
        let manifest = distribution.as_ref().ok().and_then(|value| market::distributed(value, &entry.name));
        (index, entry, manifest, distribution.as_ref().err().cloned())
    }).collect();
    let tasks = stream::iter(jobs.into_iter().map(|(index, mut entry, manifest, distribution_error)| async move {
        let result = if let Some(manifest) = manifest {
            market::apply_manifest(&mut entry, &manifest, false);
            entry.source = format!("magpie:{}", entry.name);
            entry.installable = !entry.version.is_empty();
            entry.unavailable_reason = None;
            Ok(())
        } else {
            match network::npm(&entry.name).await {
                Ok(Some(manifest)) if manifest["name"].as_str() == Some(entry.name.as_str()) => {
                    market::apply_manifest(&mut entry, &manifest, false);
                    entry.source = entry.name.clone();
                    entry.installable = !entry.version.is_empty();
                    entry.unavailable_reason = None;
                    Ok(())
                }
                Ok(Some(_)) => Err(network::Failure::from("npm 返回的插件包名不匹配".to_string())),
                Ok(None) => {
                    if let Some(error) = distribution_error { Err(error) }
                    else {
                        entry.installable = false;
                        entry.unavailable_reason = Some("插件尚未提供可下载的发布包".to_string());
                        Ok(())
                    }
                }
                Err(error) => Err(error),
            }
        };
        if let Err(error) = &result {
            if entry.version.is_empty() {
                entry.installable = false;
                entry.unavailable_reason = Some("插件发布信息读取失败，请稍后刷新".to_string());
            }
            crate::server::logging::verbose("[PluginCatalog]", &format!("{}：{}", entry.name, error.message));
        }
        (index, entry, result)
    })).buffer_unordered(6);
    tokio::pin!(tasks);
    let mut completed = 0;
    while let Some((index, entry, result)) = tasks.next().await {
        completed += 1;
        if let Err(error) = result {
            if issue.as_ref().is_none_or(|previous| previous.retry_at < error.retry_at) { issue = Some(error); }
        }
        cache::update(|cache| { if let Some(slot) = cache.market.get_mut(index) { *slot = entry; } });
        cache::progress(true, completed, total);
    }
    if let Some(error) = issue {
        cache::update(|cache| {
            cache.market_error = Some(format!("部分官方名单的版本或仓库信息未能更新：{}", error.message));
            cache.market_retry = error.retry_at.max(crate::server::logging::now_ms() + 60_000);
        });
    }
    cache::persist();
    super::super::icons::wake(false);
}

async fn tagged(force: bool) {
    let previous = cache::snapshot();
    let now = crate::server::logging::now_ms();
    if previous.github_retry > now || !force && previous.github_retry == 0
        && previous.github_at > 0 && now - previous.github_at < GITHUB_TTL { return; }
    cache::progress(true, 0, 0);
    let rows = match github::search().await {
        Ok(rows) => rows,
        Err(error) => {
            cache::update(|cache| { cache.github_error = Some(error.message); cache.github_retry = error.retry_at.max(now + 60_000); });
            cache::persist();
            return;
        }
    };
    let total = rows.items.len();
    let repositories: BTreeSet<_> = rows.items.iter().filter_map(|row| types::repository(&row["full_name"]))
        .map(|repository| repository.to_lowercase()).collect();
    cache::update(|cache| {
        cache.github.retain(|entry| entry.repository.as_ref().is_some_and(|repo| repositories.contains(&repo.to_lowercase())));
        cache.ignored.retain(|repo, _| repositories.contains(repo));
        cache.github_unreadable.retain(|entry| repositories.contains(&entry.repository));
        for entry in cache.market.iter_mut().chain(cache.github.iter_mut()) {
            if let Some(row) = rows.items.iter().find(|row| types::repository(&row["full_name"]).as_ref().zip(entry.repository.as_ref())
                .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))) {
                if entry.publisher == "third-party" {
                    entry.stars = row["stargazers_count"].as_u64();
                    entry.stars_at = now;
                } else { github::apply_repository(entry, row); }
            }
        }
    });
    cache::progress(true, 0, total);
    let (missing, failure) = github::resolve(rows.items, previous.github, force).await;
    cache::update(|cache| {
        cache.github_at = crate::server::logging::now_ms();
        cache.github_missing = missing;
        cache.github_error = failure.as_ref().map(|error| format!("{missing} 个插件仓库暂时未能读取：{}", error.message)).or(rows.warning);
        cache.github_retry = failure.as_ref().map(|error| error.retry_at.max(cache.github_at + 60_000)).unwrap_or(0);
    });
    cache::progress(true, total, total);
    cache::persist();
    super::super::icons::wake(false);
}

async fn stars(force: bool) {
    let now = crate::server::logging::now_ms();
    if cache::snapshot().stars_retry > now { return; }
    let entries = cache::entries();
    let mut repositories = BTreeMap::new();
    for entry in entries.iter().filter(|entry| (force && entry.publisher != "third-party")
        || entry.stars_at == 0 || now - entry.stars_at >= MARKET_TTL) {
        if let Some(repository) = &entry.repository { repositories.entry(repository.to_lowercase()).or_insert(repository.clone()); }
    }
    if repositories.is_empty() { return; }
    let tasks = stream::iter(repositories.into_values().map(|repository| async move {
        let result = github::stars(&repository).await;
        (repository, result)
    })).buffer_unordered(4);
    tokio::pin!(tasks);
    while let Some((repository, result)) = tasks.next().await {
        match result {
            Ok(stars) => cache::update(|cache| {
                for entry in cache.market.iter_mut().chain(cache.github.iter_mut()) {
                    if entry.repository.as_ref().is_some_and(|known| known.eq_ignore_ascii_case(&repository)) {
                        entry.stars = stars;
                        entry.stars_at = now;
                    }
                }
            }),
            Err(error) => {
                cache::update(|cache| { cache.stars_retry = error.retry_at.max(now + 5 * 60_000); });
                break;
            }
        }
    }
    cache::persist();
}

async fn refresh(force: bool) {
    curated(force).await;
    tagged(force).await;
    stars(force).await;
}
