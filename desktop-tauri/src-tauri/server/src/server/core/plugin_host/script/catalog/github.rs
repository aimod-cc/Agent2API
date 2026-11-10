use std::collections::BTreeMap;
use futures::{stream, StreamExt};
use serde_json::Value;

use super::{cache, market, provider};
use super::network::{self, Failure};
use super::types::{self, Entry};

pub(super) const TOPIC: &str = "magpie-plugin";
const LIMIT: usize = 1000;
const MANIFEST_TTL: i64 = 6 * 3_600_000;

pub(super) struct Repositories {
    pub items: Vec<Value>,
    pub warning: Option<String>,
}

pub(super) async fn search() -> Result<Repositories, Failure> {
    let mut items = BTreeMap::new();
    let mut warning = None;
    for page in 1..=10 {
        let mut url = url::Url::parse("https://api.github.com/search/repositories").map_err(|error| Failure::from(error.to_string()))?;
        url.query_pairs_mut().append_pair("q", &format!("topic:{TOPIC} archived:false fork:false"))
            .append_pair("sort", "stars").append_pair("order", "desc")
            .append_pair("per_page", "100").append_pair("page", &page.to_string());
        let payload = network::github(url.as_str(), 8 * 1024 * 1024).await?
            .ok_or_else(|| Failure::from("GitHub 未返回插件标签搜索结果".to_string()))?;
        let rows = payload["items"].as_array().ok_or_else(|| Failure::from("GitHub 插件标签搜索结果格式无效".to_string()))?;
        let total = payload["total_count"].as_u64().ok_or_else(|| Failure::from("GitHub 插件标签搜索结果缺少总数".to_string()))?;
        if payload["incomplete_results"] == true { warning = Some("GitHub 返回的插件标签清单尚不完整，稍后会继续刷新".to_string()); }
        if total > LIMIT as u64 { warning = Some("GitHub 标签搜索最多返回 1000 个仓库，当前展示 Star 数最高的结果".to_string()); }
        for row in rows {
            if row["fork"] == true || row["archived"] == true { continue; }
            let Some(repository) = types::repository(&row["full_name"]) else { continue; };
            if repository.split('/').next().is_some_and(|owner| owner.eq_ignore_ascii_case("magpie-community")) { continue; }
            items.entry(repository.to_lowercase()).or_insert(row.clone());
        }
        if rows.len() < 100 || page * 100 >= total { break; }
    }
    let mut items: Vec<_> = items.into_values().collect();
    items.sort_by_key(|item| std::cmp::Reverse(item["stargazers_count"].as_u64().unwrap_or(0)));
    Ok(Repositories { items, warning })
}

pub(super) fn apply_repository(entry: &mut Entry, row: &Value) {
    if let Some(repository) = types::repository(&row["full_name"]) {
        entry.repository_url = Some(format!("https://github.com/{repository}"));
        entry.repository = Some(repository);
    }
    entry.stars = row["stargazers_count"].as_u64();
    entry.stars_at = crate::server::logging::now_ms();
    entry.pushed_at = types::text(&row["pushed_at"], 64);
}

pub(super) fn raw_url(repository: &str, reference: &str, path: &str) -> Result<String, Failure> {
    let mut url = url::Url::parse(&format!("https://raw.githubusercontent.com/{repository}/"))
        .map_err(|error| Failure::from(error.to_string()))?;
    let mut segments = url.path_segments_mut().map_err(|_| Failure::from("插件仓库路径无效".to_string()))?;
    segments.pop_if_empty().push(reference);
    for part in path.strip_prefix("./").unwrap_or(path).split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(['\\', '?', '#', ':', '%']) {
            return Err("插件入口路径无效".to_string().into());
        }
        segments.push(part);
    }
    drop(segments);
    Ok(url.to_string())
}

fn pick_entry(value: &Value) -> Option<String> {
    if let Some(path) = value.as_str().filter(|path| !path.trim().is_empty()) { return Some(path.to_string()); }
    [".", "import", "default", "node", "require", "bun"].into_iter()
        .find_map(|key| value.get(key).and_then(pick_entry))
}

fn has_entry(manifest: &Value) -> bool {
    manifest["main"].as_str().is_some_and(|value| !value.trim().is_empty())
        || pick_entry(&manifest["exports"]).is_some() || pick_entry(&manifest["exports"]["./server"]).is_some()
}

/// 单个仓库的读取结论：
/// Entry = 入库，Skip = 本次读出「不是提供商或清单缺失」（进忽略生命周期），
/// Skipped = 忽略快速路径命中（没有新信息，保留已有的未解析记录），
/// Unreadable = 清单不是合法 JSON（同样进忽略生命周期，原因进错误详情）
enum ReadOutcome {
    Entry(Entry),
    Skip,
    Skipped,
    Unreadable(String),
}

async fn read(row: &Value, previous: Option<&Entry>, force: bool) -> Result<ReadOutcome, Failure> {
    let repository = types::repository(&row["full_name"]).ok_or_else(|| Failure::from("GitHub 仓库标识无效".to_string()))?;
    let now = crate::server::logging::now_ms();
    if let Some(previous) = previous.filter(|entry| entry.is_provider() && !force && entry.pushed_at == row["pushed_at"].as_str().unwrap_or("")
        && entry.manifest_at > 0 && now.saturating_sub(entry.manifest_at) < MANIFEST_TTL) {
        let mut entry = previous.clone();
        apply_repository(&mut entry, row);
        return Ok(ReadOutcome::Entry(entry));
    }
    let reference = row["default_branch"].as_str().filter(|value| !value.is_empty()).unwrap_or("HEAD");
    let url = raw_url(&repository, reference, "package.json")?;
    let manifest = match network::github(&url, 2 * 1024 * 1024).await {
        Ok(Some(manifest)) => manifest,
        Ok(None) => return Ok(ReadOutcome::Skip),
        Err(failure) if failure.message == network::INVALID_MANIFEST => {
            return Ok(ReadOutcome::Unreadable(failure.detail
                .unwrap_or_else(|| "响应体不是合法 JSON".to_string())))
        }
        Err(failure) => return Err(failure),
    };
    let name = manifest["name"].as_str().unwrap_or("");
    if !super::super::packages::package_name(name) { return Ok(ReadOutcome::Skip); }
    if provider::excluded(&manifest) { return Ok(ReadOutcome::Skip); }
    let evidence = provider::inspect(&repository, reference, &manifest).await?;
    if !evidence.provider { return Ok(ReadOutcome::Skip); }
    let mut entry = Entry {
        name: name.to_string(), label: name.to_string(), publisher: "third-party".to_string(),
        source: format!("github:{repository}"), selector: reference.to_string(),
        kind: "provider".to_string(), installable: true, provider_verified: true,
        providers: evidence.providers, ..Entry::default()
    };
    market::apply_manifest(&mut entry, &manifest, true);
    let description = types::text(&row["description"], 8000);
    if entry.summary.is_empty() && !description.is_empty() { entry.summary.insert("en".to_string(), description); }
    apply_repository(&mut entry, row);
    let npm = network::npm(name).await?.filter(|npm| npm["name"].as_str() == Some(name)
        && npm["version"].as_str().is_some_and(|version| !version.is_empty() && super::super::source::valid_version(version))
        && types::repository(&npm["repository"]).is_some_and(|repo| repo.eq_ignore_ascii_case(&repository)));
    if let Some(npm) = &npm {
        if !provider::excluded(npm) && (has_entry(npm) || evidence.entry_present) {
            entry.source = name.to_string();
            entry.selector = npm["version"].as_str().unwrap_or("latest").to_string();
            market::apply_manifest(&mut entry, npm, true);
            apply_repository(&mut entry, row);
            return Ok(ReadOutcome::Entry(entry));
        }
    }
    if entry.version.is_empty() {
        entry.installable = false;
        entry.unavailable_reason = Some("插件尚未声明有效版本，暂时无法安装".to_string());
    } else if !evidence.entry_present {
        entry.installable = false;
        entry.unavailable_reason = Some("仓库没有已提交的可加载入口，请等待作者发布构建产物".to_string());
    }
    Ok(ReadOutcome::Entry(entry))
}

pub(super) async fn resolve(rows: Vec<Value>, previous: Vec<Entry>, force: bool) -> (usize, Option<Failure>) {
    let before: BTreeMap<_, _> = previous.into_iter().filter_map(|entry| {
        Some((entry.repository.as_ref()?.to_lowercase(), entry))
    }).collect();
    let ignored = cache::snapshot().ignored;
    let now = crate::server::logging::now_ms();
    let total = rows.len();
    let tasks: Vec<_> = rows.into_iter().map(|row| {
        let repository = types::repository(&row["full_name"]).unwrap_or_default().to_lowercase();
        let before = before.get(&repository).cloned();
        let skip = !force && ignored.get(&repository).is_some_and(|(pushed, at)| row["pushed_at"].as_str() == Some(pushed)
            && now.saturating_sub(*at) < MANIFEST_TTL);
        (row, repository, before, skip)
    }).collect();
    let results = stream::iter(tasks.into_iter().map(|(row, repository, before, skip)| async move {
        let result = if skip { Ok(ReadOutcome::Skipped) } else {
            tokio::time::timeout(std::time::Duration::from_secs(90), read(&row, before.as_ref(), force)).await
                .unwrap_or_else(|_| Err(Failure::from("插件仓库信息读取超时".to_string())))
        };
        (row, repository, before, result)
    })).buffer_unordered(8);
    tokio::pin!(results);
    let mut completed = 0;
    let mut missing = 0;
    let mut failure: Option<Failure> = None;
    let mut failures: Vec<cache::CatalogFailure> = Vec::new();
    while let Some((row, repository, before, result)) = results.next().await {
        completed += 1;
        let entry = match result {
            Ok(ReadOutcome::Entry(entry)) => {
                cache::update(|cache| {
                    cache.ignored.remove(&repository);
                    cache.github_unreadable.retain(|entry| entry.repository != repository);
                });
                Some(entry)
            }
            Ok(ReadOutcome::Skip) => {
                cache::update(|cache| {
                    cache.ignored.insert(repository.clone(), (types::text(&row["pushed_at"], 64), now));
                    cache.github.retain(|entry| entry.repository.as_ref().is_none_or(|known| !known.eq_ignore_ascii_case(&repository)));
                    cache.github_unreadable.retain(|entry| entry.repository != repository);
                });
                None
            }
            Ok(ReadOutcome::Skipped) => {
                // 快速路径没有新信息，只刷新忽略时间戳，未解析记录原样保留
                cache::update(|cache| {
                    cache.ignored.insert(repository.clone(), (types::text(&row["pushed_at"], 64), now));
                });
                None
            }
            Ok(ReadOutcome::Unreadable(detail)) => {
                cache::update(|cache| {
                    cache.ignored.insert(repository.clone(), (types::text(&row["pushed_at"], 64), now));
                    cache.github.retain(|entry| entry.repository.as_ref().is_none_or(|known| !known.eq_ignore_ascii_case(&repository)));
                    if let Some(slot) = cache.github_unreadable.iter_mut().find(|entry| entry.repository == repository) {
                        slot.detail = detail.clone();
                        slot.at = now;
                    } else {
                        cache.github_unreadable.push(cache::UnreadableManifest {
                            repository: repository.clone(), detail, at: now,
                        });
                        if cache.github_unreadable.len() > 16 {
                            if let Some(oldest) = cache.github_unreadable.iter()
                                .min_by_key(|entry| entry.at).map(|entry| entry.repository.clone()) {
                                cache.github_unreadable.retain(|entry| entry.repository != oldest);
                            }
                        }
                    }
                });
                None
            }
            Err(error) => {
                missing += 1;
                failures.push(cache::CatalogFailure {
                    repository: repository.clone(), message: error.message.clone(), retry_at: error.retry_at,
                });
                let error = error.with_repository(&repository);
                if failure.as_ref().is_none_or(|previous| previous.retry_at < error.retry_at) { failure = Some(error); }
                before.filter(Entry::is_provider).map(|mut entry| { apply_repository(&mut entry, &row); entry.manifest_at = 0; entry })
            }
        };
        if let Some(entry) = entry {
            cache::update(|cache| {
                if let Some(slot) = cache.github.iter_mut().find(|known| known.repository.as_ref()
                    .is_some_and(|known| known.eq_ignore_ascii_case(&repository))) { *slot = entry; }
                else { cache.github.push(entry); }
            });
        }
        cache::progress(true, completed, total);
        if completed == 1 || completed % 16 == 0 {
            cache::persist();
            super::super::icons::wake(false);
        }
    }
    failures.truncate(32);
    cache::update(|cache| { cache.github_failures = failures; });
    (missing, failure)
}

pub(super) async fn stars(repository: &str) -> Result<Option<u64>, Failure> {
    let value = network::github(&format!("https://api.github.com/repos/{repository}"), 512 * 1024).await?;
    Ok(value.as_ref().and_then(|value| value["stargazers_count"].as_u64()))
}
